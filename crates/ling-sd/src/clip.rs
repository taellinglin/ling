//! CLIP ViT-L/14's text transformer (SD 1.x's text encoder): 12 pre-norm layers, 12 heads, causal mask, quick GELU, final layer
//! norm; plus the WebUI's prompt handling around it (75-token chunks, emphasis weights, clip skip), as the host engine does it.
use crate::act::Act;
use crate::ops::{Eng, Lin, Loader, Norm, W16};
use crate::prompt::{self, Piece};
use crate::tok::{Tokenizer, BOS, EOS};
use crate::Error;
use alloc::vec::Vec;

struct Layer {
    ln1: Norm,
    q: Lin,
    k: Lin,
    v: Lin,
    o: Lin,
    ln2: Norm,
    fc1: Lin,
    fc2: Lin,
}

pub struct Clip {
    tok: Vec<W16>,
    dim: usize,
    pos: Vec<f32>,
    layers: Vec<Layer>,
    final_ln: Norm,
}

pub const CTX: usize = 77;

impl Clip {
    /// `l.prefix` = the text model's prefix (e.g. `cond_stage_model.transformer.`).
    pub fn load(l: &mut Loader) -> Result<Clip, Error> {
        let tm = if l.has("text_model.embeddings.token_embedding.weight") { "text_model." } else { "" };
        let (tok, _rows, dim) = l.rows_f16(&alloc::format!("{tm}embeddings.token_embedding.weight"))?;
        let pos = l.vecf(&alloc::format!("{tm}embeddings.position_embedding.weight"))?;
        let mut layers = Vec::new();
        let mut i = 0;
        while l.has(&alloc::format!("{tm}encoder.layers.{i}.layer_norm1.weight")) {
            let b = alloc::format!("{tm}encoder.layers.{i}.");
            let f = |s: &str| alloc::format!("{b}{s}");
            layers.push(Layer {
                ln1: l.norm(&f("layer_norm1"))?,
                q: l.lin(&f("self_attn.q_proj"))?,
                k: l.lin(&f("self_attn.k_proj"))?,
                v: l.lin(&f("self_attn.v_proj"))?,
                o: l.lin(&f("self_attn.out_proj"))?,
                ln2: l.norm(&f("layer_norm2"))?,
                fc1: l.lin(&f("mlp.fc1"))?,
                fc2: l.lin(&f("mlp.fc2"))?,
            });
            i += 1;
        }
        if layers.is_empty() {
            return Err("the checkpoint's text encoder has no layers".into());
        }
        let final_ln = l.norm(&alloc::format!("{tm}final_layer_norm"))?;
        Ok(Clip { tok, dim, pos, layers, final_ln })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    fn embed_row(&self, id: u32, out: &mut [f32]) {
        let (part, r) = (id as usize / 16384, id as usize % 16384);
        let row = &self.tok[part.min(self.tok.len() - 1)].slice()[r * self.dim..(r + 1) * self.dim];
        for (o, &h) in out.iter_mut().zip(row.iter()) {
            *o = crate::f16::to_f32(h);
        }
    }

    /// 77 ids -> 77 x 768 hidden states. `clip_skip` 1 = the last layer, 2 = the one before it (then the final norm either way).
    pub fn forward(&self, e: &mut Eng, ids: &[u32], clip_skip: usize) -> Act {
        let d = self.dim;
        let mut x = Act::seq(ids.len(), d);
        for (t, &id) in ids.iter().enumerate() {
            let row = x.tok_mut(t);
            self.embed_row(id, row);
            for (v, p) in row.iter_mut().zip(self.pos[t * d..(t + 1) * d].iter()) {
                *v += *p;
            }
        }
        let n = self.layers.len();
        let last = n - clip_skip.clamp(1, n);
        let heads = d / 64;
        for (li, ly) in self.layers.iter().enumerate() {
            if li > last {
                break;
            }
            let h = e.layer_norm(&x, &ly.ln1, 1e-5);
            let q = e.linear(&ly.q, &h, None);
            let k = e.linear(&ly.k, &h, None);
            let v = e.linear(&ly.v, &h, None);
            drop(h);
            let a = e.attention(&q, &k, &v, heads, true);
            x = e.linear(&ly.o, &a, Some(x));
            let h = e.layer_norm(&x, &ly.ln2, 1e-5);
            let mut m = e.linear(&ly.fc1, &h, None);
            e.quick_gelu(&mut m);
            x = e.linear(&ly.fc2, &m, Some(x));
        }
        e.layer_norm(&x, &self.final_ln, 1e-5)
    }
}

type Chunk = (Vec<u32>, Vec<f32>);

/// The WebUI's 75-token chunking (a comma near the end carries what follows it over).
pub fn chunk_prompt(tok: &Tokenizer, text: &str) -> Vec<Chunk> {
    let comma = tok.encode(",").first().copied().unwrap_or(267);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut ids: Vec<u32> = Vec::new();
    let mut ws: Vec<f32> = Vec::new();
    let mut last_comma: Option<usize> = None;
    for piece in prompt::parse_attention(&prompt::strip_networks(text)) {
        match piece {
            Piece::Break => {
                chunks.push((core::mem::take(&mut ids), core::mem::take(&mut ws)));
                last_comma = None;
            },
            Piece::Text(t, w) => {
                for id in tok.encode(&t) {
                    if ids.len() == 75 {
                        match last_comma {
                            Some(c) if 75 - c <= 20 => {
                                let ri = ids.split_off(c + 1);
                                let rw = ws.split_off(c + 1);
                                chunks.push((core::mem::replace(&mut ids, ri), core::mem::replace(&mut ws, rw)));
                            },
                            _ => chunks.push((core::mem::take(&mut ids), core::mem::take(&mut ws))),
                        }
                        last_comma = None;
                    }
                    ids.push(id);
                    ws.push(w);
                    if id == comma {
                        last_comma = Some(ids.len() - 1);
                    }
                }
            },
        }
    }
    if !ids.is_empty() || chunks.is_empty() {
        chunks.push((ids, ws));
    }
    chunks
}

pub fn padded(c: &Chunk) -> (Vec<u32>, Vec<f32>) {
    let mut ids = alloc::vec![BOS];
    ids.extend(&c.0);
    ids.push(EOS);
    let mut ws = alloc::vec![1.0];
    ws.extend(&c.1);
    ws.push(1.0);
    ids.truncate(CTX);
    ws.truncate(CTX);
    while ids.len() < CTX {
        ids.push(EOS);
        ws.push(1.0);
    }
    (ids, ws)
}

/// Encode chunks and join them: (77 * chunks) x 768. Emphasis: z * w, rescaled to keep the mean (the WebUI's way).
pub fn encode_chunks(clip: &Clip, e: &mut Eng, chunks: &[Chunk], clip_skip: usize) -> Act {
    let d = clip.dim();
    let out = Act::seq(CTX * chunks.len(), d);
    for (ci, c) in chunks.iter().enumerate() {
        let (ids, ws) = padded(c);
        let mut z = clip.forward(e, &ids, clip_skip);
        if ws.iter().any(|w| (*w - 1.0).abs() > 1e-6) {
            let mean = |z: &Act| {
                let mut s = 0f64;
                for t in 0..z.tokens() {
                    s += z.tok(t).iter().map(|&v| v as f64).sum::<f64>();
                }
                s / (z.tokens() * d) as f64
            };
            let before = mean(&z);
            for (t, w) in ws.iter().enumerate() {
                for v in z.tok_mut(t).iter_mut() {
                    *v *= *w;
                }
            }
            let after = mean(&z);
            if after.abs() > 1e-12 {
                let k = (before / after) as f32;
                for t in 0..z.tokens() {
                    for v in z.tok_mut(t).iter_mut() {
                        *v *= k;
                    }
                }
            }
        }
        for t in 0..CTX {
            unsafe { core::ptr::copy_nonoverlapping(z.px(t), out.px(ci * CTX + t), d) };
        }
    }
    out
}

/// (cond, uncond) of the same length.
pub fn encode_pair(clip: &Clip, tok: &Tokenizer, e: &mut Eng, prompt: &str, negative: &str, clip_skip: usize) -> (Act, Act) {
    let mut pc = chunk_prompt(tok, prompt);
    let mut nc = chunk_prompt(tok, negative);
    let n = pc.len().max(nc.len());
    while pc.len() < n {
        pc.push((Vec::new(), Vec::new()));
    }
    while nc.len() < n {
        nc.push((Vec::new(), Vec::new()));
    }
    let c = encode_chunks(clip, e, &pc, clip_skip);
    let u = encode_chunks(clip, e, &nc, clip_skip);
    (c, u)
}
