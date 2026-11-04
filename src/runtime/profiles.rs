//! Install profiles (Cargo features `core`, `web`, `privacy`, `game`, `ai`,
//! `audio`, `os`; `full` = all, the default). A build without a profile has
//! that profile's builtins compiled out; calling one then fails with a hint
//! naming the feature to reinstall with, instead of a bare "unknown function".
//! (`crypto`, `svg` and `ui` are building blocks the profiles share; the hint
//! names a profile that brings them.)

/// `(name prefix, key)` for every builtin family that lives behind a profile.
/// Checked only for names no builtin table answered, first match wins (so the
/// more specific prefixes come first).
const FAMILIES: &[(&str, &str)] = &[
    // web
    ("http_", "web"),
    ("db_", "web"),
    ("oauth_", "web"),
    ("totp_secret", "web"),
    ("totp_now", "web"),
    ("totp_check", "web"),
    ("zip_", "web"),
    ("tar_gz_", "web"),
    ("icon_convert", "web"),
    ("code_normalize", "web"),
    ("video_", "video"),
    ("proc_", "proc"),
    ("run_process", "proc"),
    // the crypto building block (web and privacy both carry it)
    ("file_hash", "crypto"),
    ("hash_hex", "crypto"),
    ("crypto_", "crypto"),
    ("knot_", "crypto"),
    ("holo_points", "crypto"),
    ("holo_fragment_count", "crypto"),
    ("shake_hex", "crypto"),
    ("ed25519_", "crypto"),
    ("mldsa", "crypto"),
    ("password_", "crypto"),
    // the svg building block (web and game)
    ("file_fetch", "svg"),
    ("file_job", "svg"),
    ("image_fetch", "svg"),
    ("image_job", "svg"),
    ("image_svg", "svg"),
    ("image_resized", "svg"),
    ("tm_glyph", "svg"),
    // privacy
    ("lingtp_", "privacy"),
    ("ssh_", "privacy"),
    ("p2p_", "privacy"),
    ("lw_", "privacy"),
    // audio (before the ui_ widgets: ui_sound is a sound)
    ("ui_sound", "audio"),
    ("audio_", "audio"),
    ("music_", "audio"),
    ("mic_", "audio"),
    ("fft_", "audio"),
    ("morph_note", "audio"),
    // ai (before game: these dialog_* are the learned chat models)
    ("nn_", "ai"),
    ("bt_", "ai"),
    ("stt_", "ai"),
    ("dialog_new", "ai"),
    ("dialog_learn", "ai"),
    ("dialog_load", "ai"),
    ("dialog_train", "ai"),
    ("dialog_say", "ai"),
    ("dialog_save", "ai"),
    ("llm_", "llm"),
    ("vision_", "vision"),
    // the ui building block (game and privacy)
    ("ui_", "ui"),
    ("ease", "ui"),
    // game
    ("dialog_", "game"),
    ("tween_ease", "game"),
    ("soft_", "game"),
    ("rb_", "game"),
    ("liquid_", "game"),
    ("gltf_", "game"),
    ("draw_mesh", "game"),
    ("pad_", "game"),
    ("gamepad_", "game"),
    ("gpu_", "game"),
    ("body_", "game"),
    ("breathe", "game"),
    ("wobble", "game"),
    ("gait_", "game"),
    ("spring_to", "game"),
    ("ik2", "game"),
    ("gear_", "game"),
    ("cam_lift", "game"),
    ("piston", "game"),
    ("rack", "game"),
    // emulation (opt-in, never in `full`)
    ("emu_", "emu"),
];

/// Whether `key` is compiled into this build, and the install feature that brings it.
fn status(key: &str) -> (bool, &'static str) {
    match key {
        "web" => (cfg!(feature = "web"), "web"),
        "video" => (cfg!(feature = "video"), "web"),
        "proc" => (cfg!(feature = "proc"), "web"),
        "crypto" => (cfg!(feature = "crypto"), "web"),
        "svg" => (cfg!(feature = "svg"), "web"),
        "ui" => (cfg!(feature = "ui"), "game"),
        "privacy" => (cfg!(feature = "privacy"), "privacy"),
        "game" => (cfg!(feature = "game"), "game"),
        "ai" => (cfg!(feature = "ai"), "ai"),
        "llm" => (cfg!(feature = "llm"), "llm"),
        "vision" => (cfg!(feature = "vision"), "vision"),
        "audio" => (cfg!(feature = "audio"), "audio"),
        "emu" => (cfg!(feature = "emu"), "emu"),
        _ => (true, ""),
    }
}

/// The error for `name` when it belongs to a profile this build left out.
pub(crate) fn missing_profile(name: &str) -> Option<String> {
    let (_, key) = FAMILIES.iter().find(|(p, _)| name.starts_with(p))?;
    let (present, feature) = status(key);
    if present {
        return None;
    }
    Some(format!(
        "'{name}' needs the `{feature}` profile, which this build of ling leaves out \
         (reinstall with: cargo install ling-lang --features {feature}, or the default full install)"
    ))
}

