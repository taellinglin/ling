//! The SD UNet, vendored from candle-transformers' `unet_2d.rs` (Apache-2.0 / MIT) with the one thing
//! it leaves out: SDXL's `add_embedding` (a.k.a. `label_emb`), which folds the pooled OpenCLIP text
//! embedding and the size / crop conditioning into the timestep embedding. Without it an SDXL
//! checkpoint still runs but ignores half of its conditioning. Down / mid / up blocks are candle's.

use candle_core::{DType, Result, Tensor};
use candle_nn as nn;
use candle_nn::Module;
use candle_transformers::models::stable_diffusion::embeddings::{TimestepEmbedding, Timesteps};
use crate::sd::blocks::*;

#[derive(Debug, Clone, Copy)]
pub struct BlockConfig {
    pub out_channels: usize,
    /// None = no cross-attention; Some(n) = n transformer layers per attention
    pub use_cross_attn: Option<usize>,
    pub attention_head_dim: usize,
}

#[derive(Debug, Clone)]
pub struct UNetConfig {
    pub blocks: Vec<BlockConfig>,
    pub layers_per_block: usize,
    pub cross_attention_dim: usize,
    pub use_linear_projection: bool,
    pub sliced_attention_size: Option<usize>,
    /// SDXL: width of the vector fed to add_embedding (1280 pooled + 6 * 256 size ids = 2816)
    pub addition_embed_dim: Option<usize>,
}

impl UNetConfig {
    pub fn sd15(sliced: Option<usize>) -> Self {
        let bc = |out_channels, use_cross_attn, attention_head_dim| BlockConfig { out_channels, use_cross_attn, attention_head_dim };
        Self {
            blocks: vec![bc(320, Some(1), 8), bc(640, Some(1), 8), bc(1280, Some(1), 8), bc(1280, None, 8)],
            layers_per_block: 2,
            cross_attention_dim: 768,
            use_linear_projection: false,
            sliced_attention_size: sliced,
            addition_embed_dim: None,
        }
    }
    pub fn sdxl(sliced: Option<usize>) -> Self {
        let bc = |out_channels, use_cross_attn, attention_head_dim| BlockConfig { out_channels, use_cross_attn, attention_head_dim };
        Self {
            blocks: vec![bc(320, None, 5), bc(640, Some(2), 10), bc(1280, Some(10), 20)],
            layers_per_block: 2,
            cross_attention_dim: 2048,
            use_linear_projection: true,
            sliced_attention_size: sliced,
            addition_embed_dim: Some(2816),
        }
    }
}

enum Down {
    Basic(DownBlock2D),
    Cross(CrossAttnDownBlock2D),
}
enum Up {
    Basic(UpBlock2D),
    Cross(CrossAttnUpBlock2D),
}

pub struct UNet {
    conv_in: nn::Conv2d,
    time_proj: Timesteps,
    time_embedding: TimestepEmbedding,
    add_time_proj: Option<Timesteps>,
    add_embedding: Option<TimestepEmbedding>,
    down: Vec<Down>,
    mid: UNetMidBlock2DCrossAttn,
    up: Vec<Up>,
    conv_norm_out: nn::GroupNorm,
    conv_out: nn::Conv2d,
    n_blocks: usize,
    pub in_channels: usize,
}

impl UNet {
    pub fn new(vs: nn::VarBuilder, in_channels: usize, cfg: &UNetConfig) -> Result<Self> {
        let n = cfg.blocks.len();
        let b0 = cfg.blocks[0].out_channels;
        let bl = cfg.blocks[n - 1].out_channels;
        let bl_heads = cfg.blocks[n - 1].attention_head_dim;
        let temb = b0 * 4;
        let conv_cfg = nn::Conv2dConfig { padding: 1, ..Default::default() };
        let conv_in = nn::conv2d(in_channels, b0, 3, conv_cfg, vs.pp("conv_in"))?;
        let time_proj = Timesteps::new(b0, true, 0.);
        let time_embedding = TimestepEmbedding::new(vs.pp("time_embedding"), b0, temb)?;
        let (add_time_proj, add_embedding) = match cfg.addition_embed_dim {
            Some(d) => (Some(Timesteps::new(256, true, 0.)), Some(TimestepEmbedding::new(vs.pp("add_embedding"), d, temb)?)),
            None => (None, None),
        };
        let sliced = |head_dim: usize| match cfg.sliced_attention_size {
            Some(0) => Some(head_dim / 2),
            s => s,
        };

        let vs_db = vs.pp("down_blocks");
        let mut down = Vec::new();
        for i in 0..n {
            let BlockConfig { out_channels, use_cross_attn, attention_head_dim } = cfg.blocks[i];
            let in_ch = if i > 0 { cfg.blocks[i - 1].out_channels } else { b0 };
            let db = DownBlock2DConfig {
                num_layers: cfg.layers_per_block,
                resnet_eps: 1e-5,
                resnet_groups: 32,
                add_downsample: i < n - 1,
                downsample_padding: 1,
                ..Default::default()
            };
            down.push(match use_cross_attn {
                Some(layers) => Down::Cross(CrossAttnDownBlock2D::new(
                    vs_db.pp(i.to_string()),
                    in_ch,
                    out_channels,
                    Some(temb),
                    false,
                    CrossAttnDownBlock2DConfig {
                        downblock: db,
                        attn_num_head_channels: attention_head_dim,
                        cross_attention_dim: cfg.cross_attention_dim,
                        sliced_attention_size: sliced(attention_head_dim),
                        use_linear_projection: cfg.use_linear_projection,
                        transformer_layers_per_block: layers,
                    },
                )?),
                None => Down::Basic(DownBlock2D::new(vs_db.pp(i.to_string()), in_ch, out_channels, Some(temb), db)?),
            });
        }

        let mid = UNetMidBlock2DCrossAttn::new(
            vs.pp("mid_block"),
            bl,
            Some(temb),
            false,
            UNetMidBlock2DCrossAttnConfig {
                resnet_eps: 1e-5,
                output_scale_factor: 1.,
                cross_attn_dim: cfg.cross_attention_dim,
                attn_num_head_channels: bl_heads,
                resnet_groups: Some(32),
                use_linear_projection: cfg.use_linear_projection,
                transformer_layers_per_block: cfg.blocks[n - 1].use_cross_attn.unwrap_or(1),
                sliced_attention_size: sliced(bl_heads),
                ..Default::default()
            },
        )?;

        let vs_ub = vs.pp("up_blocks");
        let mut up = Vec::new();
        for i in 0..n {
            let BlockConfig { out_channels, use_cross_attn, attention_head_dim } = cfg.blocks[n - 1 - i];
            let prev_out = if i > 0 { cfg.blocks[n - i].out_channels } else { bl };
            let in_ch = cfg.blocks[if i == n - 1 { 0 } else { n - i - 2 }].out_channels;
            let ub = UpBlock2DConfig {
                num_layers: cfg.layers_per_block + 1,
                resnet_eps: 1e-5,
                resnet_groups: 32,
                add_upsample: i < n - 1,
                ..Default::default()
            };
            up.push(match use_cross_attn {
                Some(layers) => Up::Cross(CrossAttnUpBlock2D::new(
                    vs_ub.pp(i.to_string()),
                    in_ch,
                    prev_out,
                    out_channels,
                    Some(temb),
                    false,
                    CrossAttnUpBlock2DConfig {
                        upblock: ub,
                        attn_num_head_channels: attention_head_dim,
                        cross_attention_dim: cfg.cross_attention_dim,
                        sliced_attention_size: sliced(attention_head_dim),
                        use_linear_projection: cfg.use_linear_projection,
                        transformer_layers_per_block: layers,
                    },
                )?),
                None => Up::Basic(UpBlock2D::new(vs_ub.pp(i.to_string()), in_ch, prev_out, out_channels, Some(temb), ub)?),
            });
        }
        let conv_norm_out = nn::group_norm(32, b0, 1e-5, vs.pp("conv_norm_out"))?;
        let conv_out = nn::conv2d(b0, 4, 3, conv_cfg, vs.pp("conv_out"))?;
        Ok(Self {
            conv_in,
            time_proj,
            time_embedding,
            add_time_proj,
            add_embedding,
            down,
            mid,
            up,
            conv_norm_out,
            conv_out,
            n_blocks: n,
            in_channels,
        })
    }

    /// `added`: SDXL only, (pooled text [B,1280], size ids [B,6]).
    pub fn forward(&self, xs: &Tensor, timestep: f64, ctx: &Tensor, added: Option<(&Tensor, &Tensor)>) -> Result<Tensor> {
        let (bsize, _c, height, width) = xs.dims4()?;
        let dev = xs.device();
        let dtype = xs.dtype();
        let up_factor = 2usize.pow((self.n_blocks - 1) as u32);
        let forward_upsample_size = height % up_factor != 0 || width % up_factor != 0;

        // timestep embedding computed in f32 (999.37 is not representable in f16), then cast
        let t = Tensor::full(timestep as f32, bsize, dev)?;
        let emb = self.time_proj.forward(&t)?.to_dtype(dtype)?;
        let mut emb = self.time_embedding.forward(&emb)?;
        if let (Some(proj), Some(add), Some((pooled, ids))) = (&self.add_time_proj, &self.add_embedding, added) {
            let ids_emb = proj.forward(&ids.flatten_all()?.to_dtype(DType::F32)?)?; // [B*6, 256]
            let ids_emb = ids_emb.reshape((bsize, ()))?.to_dtype(dtype)?;
            let v = Tensor::cat(&[&pooled.to_dtype(dtype)?, &ids_emb], 1)?;
            emb = (emb + add.forward(&v)?)?;
        }

        let xs = self.conv_in.forward(xs)?;
        let mut res = vec![xs.clone()];
        let mut xs = xs;
        for b in self.down.iter() {
            let (x, r) = match b {
                Down::Basic(b) => b.forward(&xs, Some(&emb))?,
                Down::Cross(b) => b.forward(&xs, Some(&emb), Some(ctx))?,
            };
            res.extend(r);
            xs = x;
        }
        let mut xs = self.mid.forward(&xs, Some(&emb), Some(ctx))?;
        let mut upsample_size = None;
        for (i, b) in self.up.iter().enumerate() {
            let n_res = match b {
                Up::Basic(b) => b.resnets.len(),
                Up::Cross(b) => b.upblock.resnets.len(),
            };
            let r = res.split_off(res.len() - n_res);
            if i < self.n_blocks - 1 && forward_upsample_size {
                let (_, _, h, w) = res.last().unwrap().dims4()?;
                upsample_size = Some((h, w));
            }
            xs = match b {
                Up::Basic(b) => b.forward(&xs, &r, Some(&emb), upsample_size)?,
                Up::Cross(b) => b.forward(&xs, &r, Some(&emb), upsample_size, Some(ctx))?,
            };
        }
        let xs = self.conv_norm_out.forward(&xs)?;
        let xs = nn::ops::silu(&xs)?;
        self.conv_out.forward(&xs)
    }
}
