//! The SD 1.x UNet in its original (LDM / AUTOMATIC1111) layout: `input_blocks`, `middle_block`, `output_blocks`, each a list of
//! ResNet blocks, spatial transformers (self-attention, cross-attention on the prompt, GEGLU feed-forward), and down / up
//! sampling convolutions. The structure is read from the checkpoint's names, the widths from its shapes.
use crate::act::Act;
use crate::ops::{Aff, Conv, Eng, In, Lin, Loader, Norm, Res};
use crate::Error;
use alloc::vec::Vec;

struct Tb {
    n1: Norm,
    q1: Lin,
    k1: Lin,
    v1: Lin,
    o1: Lin,
    n2: Norm,
    q2: Lin,
    k2: Lin,
    v2: Lin,
    o2: Lin,
    n3: Norm,
    ff1: Lin,
    ff2: Lin,
}

struct St {
    norm: Norm,
    proj_in: Lin,
    blocks: Vec<Tb>,
    proj_out: Lin,
}

enum Step {
    Res(Res),
    Attn(St),
    Down(Conv),
    Up(Conv),
}

pub struct UNet {
    t1: Lin,
    t2: Lin,
    conv_in: Conv,
    input: Vec<Vec<Step>>,
    mid: Vec<Step>,
    output: Vec<Vec<Step>>,
    out_norm: Norm,
    conv_out: Conv,
    model_ch: usize,
    heads: usize,
}

fn load_st(l: &mut Loader, base: &str) -> Result<St, Error> {
    let f = |s: &str| alloc::format!("{base}.{s}");
    let mut blocks = Vec::new();
    let mut i = 0;
    while l.has(&f(&alloc::format!("transformer_blocks.{i}.norm1.weight"))) {
        let b = alloc::format!("{base}.transformer_blocks.{i}.");
        let g = |s: &str| alloc::format!("{b}{s}");
        blocks.push(Tb {
            n1: l.norm(&g("norm1"))?,
            q1: l.lin(&g("attn1.to_q"))?,
            k1: l.lin(&g("attn1.to_k"))?,
            v1: l.lin(&g("attn1.to_v"))?,
            o1: l.lin(&g("attn1.to_out.0"))?,
            n2: l.norm(&g("norm2"))?,
            q2: l.lin(&g("attn2.to_q"))?,
            k2: l.lin(&g("attn2.to_k"))?,
            v2: l.lin(&g("attn2.to_v"))?,
            o2: l.lin(&g("attn2.to_out.0"))?,
            n3: l.norm(&g("norm3"))?,
            ff1: l.lin(&g("ff.net.0.proj"))?,
            ff2: l.lin(&g("ff.net.2"))?,
        });
        i += 1;
    }
    Ok(St { norm: l.norm(&f("norm"))?, proj_in: l.lin(&f("proj_in"))?, blocks, proj_out: l.lin(&f("proj_out"))? })
}

fn load_steps(l: &mut Loader, base: &str) -> Result<Vec<Step>, Error> {
    let mut steps = Vec::new();
    let mut j = 0;
    loop {
        let b = alloc::format!("{base}.{j}");
        if l.has(&alloc::format!("{b}.in_layers.0.weight")) {
            steps.push(Step::Res(Res::load(l, &b, false)?));
        } else if l.has(&alloc::format!("{b}.proj_in.weight")) {
            steps.push(Step::Attn(load_st(l, &b)?));
        } else if l.has(&alloc::format!("{b}.op.weight")) {
            steps.push(Step::Down(l.conv(&alloc::format!("{b}.op"))?));
        } else if l.has(&alloc::format!("{b}.conv.weight")) {
            steps.push(Step::Up(l.conv(&alloc::format!("{b}.conv"))?));
        } else {
            break;
        }
        j += 1;
    }
    Ok(steps)
}

impl UNet {
    /// `l.prefix` = `model.diffusion_model.`
    pub fn load(l: &mut Loader) -> Result<UNet, Error> {
        let t1 = l.lin("time_embed.0")?;
        let t2 = l.lin("time_embed.2")?;
        let conv_in = l.conv("input_blocks.0.0")?;
        if conv_in.cin != 4 {
            return Err(alloc::format!("inpainting / {}-channel checkpoints are not supported", conv_in.cin));
        }
        let model_ch = conv_in.n;
        let mut input = Vec::new();
        let mut i = 1;
        while l.has(&alloc::format!("input_blocks.{i}.0.in_layers.0.weight")) || l.has(&alloc::format!("input_blocks.{i}.0.op.weight")) {
            input.push(load_steps(l, &alloc::format!("input_blocks.{i}"))?);
            i += 1;
        }
        let mid = load_steps(l, "middle_block")?;
        let mut output = Vec::new();
        let mut i = 0;
        while l.has(&alloc::format!("output_blocks.{i}.0.in_layers.0.weight")) {
            output.push(load_steps(l, &alloc::format!("output_blocks.{i}"))?);
            i += 1;
        }
        let out_norm = l.norm("out.0")?;
        let conv_out = l.conv("out.2")?;
        Ok(UNet { t1, t2, conv_in, input, mid, output, out_norm, conv_out, model_ch, heads: 8 })
    }

    fn st(&self, e: &mut Eng, s: &St, x: Act, ctx: &Act) -> Act {
        let a = e.gn_affine(&x, &s.norm, 32, 1e-6);
        let hn = e.apply(&x, &a, false);
        let mut h = e.linear(&s.proj_in, &hn, None);
        drop(hn);
        for b in &s.blocks {
            let n = e.layer_norm(&h, &b.n1, 1e-5);
            let q = e.linear(&b.q1, &n, None);
            let k = e.linear(&b.k1, &n, None);
            let v = e.linear(&b.v1, &n, None);
            drop(n);
            let att = e.attention(&q, &k, &v, self.heads, false);
            drop((q, k, v));
            h = e.linear(&b.o1, &att, Some(h));
            drop(att);
            let n = e.layer_norm(&h, &b.n2, 1e-5);
            let q = e.linear(&b.q2, &n, None);
            drop(n);
            let k = e.linear(&b.k2, ctx, None);
            let v = e.linear(&b.v2, ctx, None);
            let att = e.attention(&q, &k, &v, self.heads, false);
            drop((q, k, v));
            h = e.linear(&b.o2, &att, Some(h));
            drop(att);
            let n = e.layer_norm(&h, &b.n3, 1e-5);
            let f = e.linear(&b.ff1, &n, None);
            drop(n);
            let g = e.geglu(&f);
            drop(f);
            h = e.linear(&b.ff2, &g, Some(h));
        }
        e.linear(&s.proj_out, &h, Some(x))
    }

    fn run(&self, e: &mut Eng, steps: &[Step], mut h: Act, emb: &Act, ctx: &Act) -> Act {
        for s in steps {
            h = match s {
                Step::Res(r) => e.res(r, h, Some(emb)),
                Step::Attn(st) => self.st(e, st, h, ctx),
                Step::Down(c) => e.conv3(c, In { x: &h, up: false, aff: None, silu: false }, 2, None, None),
                Step::Up(c) => e.conv3(c, In { x: &h, up: true, aff: None, silu: false }, 1, None, None),
            };
            crate::kern::pump();
        }
        h
    }

    /// The noise prediction for latent `x` (h x w x 4) at timestep `t` with the prompt context `ctx` (S x 768).
    pub fn forward(&self, e: &mut Eng, x: &Act, t: f64, ctx: &Act) -> Act {
        // sinusoidal timestep embedding, cos first (flip_sin_to_cos), then the MLP; ResNets apply SiLU before their projection
        let half = self.model_ch / 2;
        let mut te = Act::seq(1, self.model_ch);
        {
            let row = te.tok_mut(0);
            for i in 0..half {
                let f = libm::exp(-libm::log(10000.0) * i as f64 / half as f64);
                let a = t * f;
                row[i] = libm::cos(a) as f32;
                row[half + i] = libm::sin(a) as f32;
            }
        }
        let mut emb = e.linear(&self.t1, &te, None);
        e.silu(&mut emb);
        let mut emb = e.linear(&self.t2, &emb, None);
        e.silu(&mut emb);

        let mut h = e.conv3(&self.conv_in, In { x, up: false, aff: None, silu: false }, 1, None, None);
        let mut hs: Vec<Act> = Vec::new();
        hs.push(h.copy());
        for blk in &self.input {
            h = self.run(e, blk, h, &emb, ctx);
            hs.push(h.copy());
        }
        h = self.run(e, &self.mid, h, &emb, ctx);
        for blk in &self.output {
            let skip = hs.pop().expect("UNet skip connections");
            let cat = e.concat(&h, &skip);
            drop((h, skip));
            h = self.run(e, blk, cat, &emb, ctx);
        }
        let a: Aff = e.gn_affine(&h, &self.out_norm, 32, 1e-5);
        e.conv3(&self.conv_out, In { x: &h, up: false, aff: Some(&a), silu: true }, 1, None, None)
    }
}
