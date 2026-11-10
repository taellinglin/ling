//! Single-file (LDM / AUTOMATIC1111) checkpoint names -> the diffusers names candle's modules load.
//! Port of diffusers' convert_from_ckpt.py for the UNet, VAE and the two CLIP text encoders.
//!
//!   model.diffusion_model.*                        -> UNet
//!   first_stage_model.*                            -> VAE
//!   cond_stage_model.transformer.*                 -> CLIP ViT-L   (SD 1.x)
//!   conditioner.embedders.0.transformer.*          -> CLIP ViT-L   (SDXL)
//!   conditioner.embedders.1.model.*                -> OpenCLIP ViT-bigG (SDXL), fused qkv split in three

pub const UNET: &str = "model.diffusion_model.";
pub const VAE: &str = "first_stage_model.";
pub const CLIP_SD1: &str = "cond_stage_model.transformer.";
pub const CLIP_SD1_ALT: &str = "cond_stage_model.model.transformer."; // a few merges carry this spelling
pub const CLIP_L_XL: &str = "conditioner.embedders.0.transformer.";
pub const CLIP_G_XL: &str = "conditioner.embedders.1.model.";

fn split_idx(s: &str) -> Option<(usize, &str)> {
    let dot = s.find('.')?;
    Some((s[..dot].parse().ok()?, &s[dot + 1..]))
}

fn unet_resnet(rest: &str) -> String {
    for (from, to) in [
        ("in_layers.0.", "norm1."),
        ("in_layers.2.", "conv1."),
        ("out_layers.0.", "norm2."),
        ("out_layers.3.", "conv2."),
        ("emb_layers.1.", "time_emb_proj."),
        ("skip_connection.", "conv_shortcut."),
    ] {
        if let Some(r) = rest.strip_prefix(from) {
            return format!("{to}{r}");
        }
    }
    rest.to_string()
}

/// `k` without the `model.diffusion_model.` prefix. `lpb` = layers per block (2 for SD1 and SDXL).
pub fn unet(k: &str, lpb: usize) -> Option<String> {
    for (from, to) in [
        ("time_embed.0.", "time_embedding.linear_1."),
        ("time_embed.2.", "time_embedding.linear_2."),
        ("label_emb.0.0.", "add_embedding.linear_1."),
        ("label_emb.0.2.", "add_embedding.linear_2."),
        ("input_blocks.0.0.", "conv_in."),
        ("out.0.", "conv_norm_out."),
        ("out.2.", "conv_out."),
    ] {
        if let Some(r) = k.strip_prefix(from) {
            return Some(format!("{to}{r}"));
        }
    }
    if let Some(r) = k.strip_prefix("input_blocks.") {
        let (i, r) = split_idx(r)?;
        let (j, rest) = split_idx(r)?;
        if i == 0 {
            return None;
        }
        let b = (i - 1) / (lpb + 1);
        let l = (i - 1) % (lpb + 1);
        return Some(match j {
            0 => match rest.strip_prefix("op.") {
                Some(op) => format!("down_blocks.{b}.downsamplers.0.conv.{op}"),
                None => format!("down_blocks.{b}.resnets.{l}.{}", unet_resnet(rest)),
            },
            _ => format!("down_blocks.{b}.attentions.{l}.{rest}"),
        });
    }
    if let Some(r) = k.strip_prefix("middle_block.") {
        let (j, rest) = split_idx(r)?;
        return Some(match j {
            0 => format!("mid_block.resnets.0.{}", unet_resnet(rest)),
            1 => format!("mid_block.attentions.0.{rest}"),
            _ => format!("mid_block.resnets.1.{}", unet_resnet(rest)),
        });
    }
    if let Some(r) = k.strip_prefix("output_blocks.") {
        let (i, r) = split_idx(r)?;
        let (j, rest) = split_idx(r)?;
        let b = i / (lpb + 1);
        let l = i % (lpb + 1);
        if j == 0 {
            return Some(format!("up_blocks.{b}.resnets.{l}.{}", unet_resnet(rest)));
        }
        if let Some(c) = rest.strip_prefix("conv.") {
            return Some(format!("up_blocks.{b}.upsamplers.0.conv.{c}"));
        }
        return Some(format!("up_blocks.{b}.attentions.{l}.{rest}"));
    }
    None
}

fn vae_resnet(rest: &str) -> String {
    match rest.strip_prefix("nin_shortcut.") {
        Some(r) => format!("conv_shortcut.{r}"),
        None => rest.to_string(),
    }
}

fn vae_mid(rest: &str) -> Option<String> {
    if let Some(r) = rest.strip_prefix("block_1.") {
        return Some(format!("mid_block.resnets.0.{}", vae_resnet(r)));
    }
    if let Some(r) = rest.strip_prefix("block_2.") {
        return Some(format!("mid_block.resnets.1.{}", vae_resnet(r)));
    }
    if let Some(r) = rest.strip_prefix("attn_1.") {
        for (from, to) in [("norm.", "group_norm."), ("q.", "query."), ("k.", "key."), ("v.", "value."), ("proj_out.", "proj_attn.")] {
            if let Some(x) = r.strip_prefix(from) {
                return Some(format!("mid_block.attentions.0.{to}{x}"));
            }
        }
    }
    None
}

/// `k` without the `first_stage_model.` prefix (or a stand-alone LDM VAE file's key).
pub fn vae(k: &str) -> Option<String> {
    if k.starts_with("quant_conv.") || k.starts_with("post_quant_conv.") {
        return Some(k.to_string());
    }
    for side in ["encoder.", "decoder."] {
        let Some(r) = k.strip_prefix(side) else { continue };
        if r.starts_with("conv_in.") || r.starts_with("conv_out.") {
            return Some(k.to_string());
        }
        if let Some(x) = r.strip_prefix("norm_out.") {
            return Some(format!("{side}conv_norm_out.{x}"));
        }
        if let Some(x) = r.strip_prefix("mid.") {
            return vae_mid(x).map(|m| format!("{side}{m}"));
        }
        if let Some(x) = r.strip_prefix("down.") {
            let (i, x) = split_idx(x)?;
            if let Some(y) = x.strip_prefix("block.") {
                let (j, y) = split_idx(y)?;
                return Some(format!("encoder.down_blocks.{i}.resnets.{j}.{}", vae_resnet(y)));
            }
            if let Some(y) = x.strip_prefix("downsample.conv.") {
                return Some(format!("encoder.down_blocks.{i}.downsamplers.0.conv.{y}"));
            }
        }
        if let Some(x) = r.strip_prefix("up.") {
            let (i, x) = split_idx(x)?;
            let bi = 3usize.checked_sub(i)?; // decoder blocks run in reverse
            if let Some(y) = x.strip_prefix("block.") {
                let (j, y) = split_idx(y)?;
                return Some(format!("decoder.up_blocks.{bi}.resnets.{j}.{}", vae_resnet(y)));
            }
            if let Some(y) = x.strip_prefix("upsample.conv.") {
                return Some(format!("decoder.up_blocks.{bi}.upsamplers.0.conv.{y}"));
            }
        }
    }
    None
}

/// CLIP ViT-L keys after their prefix: they already use HF names, only `text_model.` may be missing.
pub fn clip_l(k: &str) -> Option<String> {
    if k.ends_with("position_ids") {
        return None;
    }
    Some(if k.starts_with("text_model.") { k.to_string() } else { format!("text_model.{k}") })
}

/// What one OpenCLIP key becomes: a straight rename, or a third of the fused qkv tensor.
pub enum GKey {
    Plain(String),
    /// (q, k, v) target names; the tensor is split in three along dim 0
    Qkv([String; 3]),
    Projection,
}

pub fn clip_g(k: &str) -> Option<GKey> {
    let p = |s: String| Some(GKey::Plain(s));
    match k {
        "token_embedding.weight" => return p("text_model.embeddings.token_embedding.weight".into()),
        "positional_embedding" => return p("text_model.embeddings.position_embedding.weight".into()),
        "text_projection" => return Some(GKey::Projection),
        _ => {}
    }
    if let Some(r) = k.strip_prefix("ln_final.") {
        return p(format!("text_model.final_layer_norm.{r}"));
    }
    let r = k.strip_prefix("transformer.resblocks.")?;
    let (i, r) = split_idx(r)?;
    let base = format!("text_model.encoder.layers.{i}.");
    for (from, to) in [
        ("ln_1.", "layer_norm1."),
        ("ln_2.", "layer_norm2."),
        ("mlp.c_fc.", "mlp.fc1."),
        ("mlp.c_proj.", "mlp.fc2."),
        ("attn.out_proj.", "self_attn.out_proj."),
    ] {
        if let Some(x) = r.strip_prefix(from) {
            return p(format!("{base}{to}{x}"));
        }
    }
    let suffix = match r {
        "attn.in_proj_weight" => "weight",
        "attn.in_proj_bias" => "bias",
        _ => return None,
    };
    Some(GKey::Qkv([
        format!("{base}self_attn.q_proj.{suffix}"),
        format!("{base}self_attn.k_proj.{suffix}"),
        format!("{base}self_attn.v_proj.{suffix}"),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unet_names() {
        assert_eq!(unet("input_blocks.1.0.in_layers.0.weight", 2).unwrap(), "down_blocks.0.resnets.0.norm1.weight");
        assert_eq!(unet("input_blocks.2.1.proj_in.weight", 2).unwrap(), "down_blocks.0.attentions.1.proj_in.weight");
        assert_eq!(unet("input_blocks.3.0.op.weight", 2).unwrap(), "down_blocks.0.downsamplers.0.conv.weight");
        assert_eq!(unet("output_blocks.2.1.conv.bias", 2).unwrap(), "up_blocks.0.upsamplers.0.conv.bias");
        assert_eq!(unet("output_blocks.5.2.conv.bias", 2).unwrap(), "up_blocks.1.upsamplers.0.conv.bias");
        assert_eq!(unet("output_blocks.4.1.norm.weight", 2).unwrap(), "up_blocks.1.attentions.1.norm.weight");
        assert_eq!(unet("middle_block.2.skip_connection.weight", 2).unwrap(), "mid_block.resnets.1.conv_shortcut.weight");
    }
    #[test]
    fn vae_names() {
        assert_eq!(vae("decoder.up.3.block.0.conv1.weight").unwrap(), "decoder.up_blocks.0.resnets.0.conv1.weight");
        assert_eq!(vae("encoder.mid.attn_1.q.weight").unwrap(), "encoder.mid_block.attentions.0.query.weight");
        assert_eq!(vae("encoder.down.1.block.0.nin_shortcut.bias").unwrap(), "encoder.down_blocks.1.resnets.0.conv_shortcut.bias");
    }
}
