//! `lingfu seed` — a reconstructable, encrypted integrity fingerprint of the
//! project: every git-tracked file's own BLAKE3 checksum, rolled up into a
//! Merkle root (a `log2(n)`-depth tree — the "logarithmic symmetry"), the
//! whole manifest encrypted with a key deterministically derived (HKDF-SHA3,
//! no salt — the same keyfile always regenerates the same key, nothing else
//! is ever stored) from a keyfile, and packaged as a playable/inspectable
//! `.wav` file rather than an opaque blob.
//!
//! `--check` recomputes the manifest and diffs it against a saved seed to
//! catch drift — including silent, small modifications — while another
//! process runs, if invoked from a hook/watcher.
//!
//! A keyfile-derived, unsalted key means the keyfile *is* the entire root of
//! trust: lose it and there is no way to ever regenerate the same key again.
//! `--recover-init` deliberately trades a little of that starkness for
//! recoverability by also splitting the derived key into Shamir shares
//! (2-of-3): any two reconstruct it without the keyfile at all.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const INFO_LABEL: &[u8] = b"lingfu-seed-v1";
const DEFAULT_SEED_FILE: &str = "seed.wav";
const WAV_SAMPLE_RATE: u32 = 22_050;

/// relative path (forward-slashed) -> BLAKE3 hash, kept sorted so the
/// manifest and its Merkle root are identical across runs/machines.
type Manifest = BTreeMap<String, [u8; 32]>;

// ─── file discovery ─────────────────────────────────────────────────────────

/// Git-tracked files only: already respects `.gitignore`, needs no
/// hand-maintained include/exclude list, and is exactly what "essential"
/// means for a versioned project — build artefacts and scratch files were
/// never tracked in the first place. `ling.ignore` (see `crate::ignore`) is
/// applied on top, for anything tracked-but-still-shouldn't-be-hashed —
/// e.g. a `seed` keyfile or recovery share that later got `git add`ed by
/// mistake, which would otherwise end up hashed into the very manifest it
/// derives the encryption key for.
fn essential_files(root: &Path) -> Result<Vec<PathBuf>> {
    let out = std::process::Command::new("git")
        .arg("ls-files")
        .current_dir(root)
        .output()
        .context("running `git ls-files` (is this a git repository?)")?;
    if !out.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let ignore_patterns = crate::ignore::load(root);
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .filter(|l| !crate::ignore::is_ignored(&ignore_patterns, l))
        .map(PathBuf::from)
        .collect())
}

fn build_manifest(root: &Path, files: &[PathBuf]) -> Result<Manifest> {
    let mut m = Manifest::new();
    for rel in files {
        let full = root.join(rel);
        let bytes = match std::fs::read(&full) {
            Ok(b) => b,
            // A tracked path that's a submodule/symlink-to-nowhere/etc.
            // shouldn't abort the whole manifest.
            Err(_) => continue,
        };
        let hash = ling_crypto::hash::Blake3::hash(&bytes);
        m.insert(rel.to_string_lossy().replace('\\', "/"), hash);
    }
    Ok(m)
}

// ─── Merkle root ────────────────────────────────────────────────────────────

/// Pairwise-BLAKE3 binary Merkle tree over `manifest`'s hashes in sorted
/// (path) order; an odd node out is carried up unchanged rather than
/// duplicated, so a manifest of one file's root is just that file's hash.
fn merkle_root(manifest: &Manifest) -> [u8; 32] {
    let mut layer: Vec<[u8; 32]> = manifest.values().copied().collect();
    if layer.is_empty() {
        return [0u8; 32];
    }
    while layer.len() > 1 {
        let mut next = Vec::with_capacity(layer.len().div_ceil(2));
        for pair in layer.chunks(2) {
            let h = if pair.len() == 2 {
                let mut buf = [0u8; 64];
                buf[..32].copy_from_slice(&pair[0]);
                buf[32..].copy_from_slice(&pair[1]);
                ling_crypto::hash::Blake3::hash(&buf)
            } else {
                pair[0]
            };
            next.push(h);
        }
        layer = next;
    }
    layer[0]
}

// ─── manifest <-> text (the thing that actually gets encrypted) ───────────

/// Every line is `<hex hash>  <name>`, hash first, so parsing doesn't need
/// to special-case the root line's shape — only its name, the literal
/// `ROOT`, which can't collide with a real (git-tracked, forward-slashed)
/// path.
fn manifest_to_text(manifest: &Manifest, root: &[u8; 32]) -> String {
    let mut out = String::new();
    for (path, hash) in manifest {
        out.push_str(&hex::encode(hash));
        out.push_str("  ");
        out.push_str(path);
        out.push('\n');
    }
    out.push_str(&hex::encode(root));
    out.push_str("  ROOT\n");
    out
}

fn text_to_manifest(text: &str) -> Result<(Manifest, [u8; 32])> {
    let mut m = Manifest::new();
    let mut root = None;
    for line in text.lines() {
        let Some((hash_hex, name)) = line.split_once("  ") else { continue };
        let hash = hex::decode(hash_hex).context("corrupt manifest: bad hash hex")?;
        let hash: [u8; 32] = hash
            .try_into()
            .map_err(|_| anyhow::anyhow!("corrupt manifest: hash not 32 bytes"))?;
        if name == "ROOT" {
            root = Some(hash);
        } else {
            m.insert(name.to_string(), hash);
        }
    }
    let root = root.context("corrupt manifest: no ROOT line")?;
    Ok((m, root))
}

// ─── tiny hex + WAV helpers (no extra dependencies) ────────────────────────

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        const T: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push(T[(b >> 4) as usize] as char);
            s.push(T[(b & 0xf) as usize] as char);
        }
        s
    }

    pub fn decode(s: &str) -> anyhow::Result<Vec<u8>> {
        if s.len() % 2 != 0 {
            anyhow::bail!("odd-length hex string");
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow::anyhow!(e)))
            .collect()
    }
}

/// Wrap arbitrary bytes as 8-bit unsigned mono PCM — a real, playable
/// (if noisy) `.wav` file, and byte-for-byte reversible: sample N *is*
/// input byte N, no bit-packing or resampling to undo.
fn write_wav(path: &Path, data: &[u8]) -> Result<()> {
    let mut out = Vec::with_capacity(44 + data.len());
    let data_len = data.len() as u32;
    let byte_rate = WAV_SAMPLE_RATE; // 1 channel * 1 byte/sample * sample_rate
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&WAV_SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // block align (1 byte/sample)
    out.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(data);
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

fn read_wav(path: &Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("{}: not a WAV file this tool wrote", path.display());
    }
    // Walk chunks rather than assume `fmt ` is exactly 16 bytes / `data`
    // starts at byte 44 — still simple, just not fragile to a stray chunk.
    let mut i = 12usize;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        let start = i + 8;
        if id == b"data" {
            let end = (start + len).min(bytes.len());
            return Ok(bytes[start..end].to_vec());
        }
        i = start + len + (len % 2); // chunks are word-aligned
    }
    bail!("{}: no data chunk found", path.display());
}

// ─── key derivation + AEAD ──────────────────────────────────────────────────

fn derive_key(keyfile: &Path) -> Result<[u8; 32]> {
    let ikm =
        std::fs::read(keyfile).with_context(|| format!("reading keyfile {}", keyfile.display()))?;
    let derived = ling_crypto::kdf::hkdf_sha3(&ikm, &[], INFO_LABEL, 32)
        .map_err(|e| anyhow::anyhow!("hkdf: {e}"))?;
    let mut key = [0u8; 32];
    key.copy_from_slice(&derived);
    Ok(key)
}

fn encrypt(key: [u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    ling_crypto::symmetric::XChaCha20::new(key)
        .encrypt(plaintext)
        .map_err(|e| anyhow::anyhow!("encrypt: {e}"))
}

fn decrypt(key: [u8; 32], nonce_and_ct: &[u8]) -> Result<Vec<u8>> {
    ling_crypto::symmetric::XChaCha20::new(key)
        .decrypt(nonce_and_ct)
        .map_err(|e| {
            anyhow::anyhow!("decrypt (wrong keyfile/shares, or the seed was tampered with): {e}")
        })
}

// ─── Shamir recovery ────────────────────────────────────────────────────────

fn recover_init(key: [u8; 32], out_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let shares = ling_crypto::shamir::split_secret(&key, 2, 3);
    for share in &shares {
        let path = out_dir.join(format!("seed-recovery-{}.share", share.x));
        let content = format!("{}\n{}\n", share.x, hex::encode(&share.y));
        std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        println!("  wrote {}", path.display());
    }
    println!(
        "Any 2 of these 3 files reconstruct the seed key without the keyfile. Store them apart from each other and from the keyfile."
    );
    Ok(())
}

fn recover_key(share_paths: &[String]) -> Result<[u8; 32]> {
    if share_paths.len() < 2 {
        bail!("--recover needs at least 2 share files");
    }
    let mut shares = Vec::new();
    for p in share_paths {
        let text = std::fs::read_to_string(p).with_context(|| format!("reading share {p}"))?;
        let mut lines = text.lines();
        let x: u8 = lines
            .next()
            .context("empty share file")?
            .trim()
            .parse()
            .context("share file: bad x-coordinate")?;
        let y = hex::decode(lines.next().context("share file: missing y")?.trim())?;
        shares.push(ling_crypto::shamir::Share { x, y });
    }
    let secret = ling_crypto::shamir::reconstruct_secret(&shares);
    let key: [u8; 32] = secret.try_into().map_err(|_| {
        anyhow::anyhow!("reconstructed secret is not 32 bytes — wrong/mismatched shares")
    })?;
    Ok(key)
}

// ─── diff report ────────────────────────────────────────────────────────────

fn report_diff(old: &Manifest, new: &Manifest) -> bool {
    let mut clean = true;
    for (path, new_hash) in new {
        match old.get(path) {
            None => {
                println!("  + added     {path}");
                clean = false;
            },
            Some(old_hash) if old_hash != new_hash => {
                println!("  ~ modified  {path}");
                clean = false;
            },
            _ => {},
        }
    }
    for path in old.keys() {
        if !new.contains_key(path) {
            println!("  - removed   {path}");
            clean = false;
        }
    }
    clean
}

// ─── CLI ────────────────────────────────────────────────────────────────────

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

pub fn run(args: &[String]) -> Result<()> {
    let root = flag_value(args, "--path")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let check = args.iter().any(|a| a == "--check");
    let recover_init_flag = args.iter().any(|a| a == "--recover-init");
    let recover_shares: Vec<String> = {
        let mut v = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--recover" {
                for s in it.by_ref() {
                    if s.starts_with("--") {
                        break;
                    }
                    v.push(s.clone());
                }
                break;
            }
        }
        v
    };
    let seed_path = flag_value(args, "--in")
        .or_else(|| flag_value(args, "--out"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SEED_FILE));
    let keyfile = flag_value(args, "--keyfile").map(PathBuf::from);

    let key = if !recover_shares.is_empty() {
        recover_key(&recover_shares)?
    } else {
        let keyfile = keyfile.context("need --keyfile <path> (or --recover <share> <share>)")?;
        derive_key(&keyfile)?
    };

    if recover_init_flag {
        let out_dir = flag_value(args, "--out-dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return recover_init(key, &out_dir);
    }

    let files = essential_files(&root)?;
    println!(
        "lingfu seed: {} tracked files under {}",
        files.len(),
        root.display()
    );
    let manifest = build_manifest(&root, &files)?;
    let root_hash = merkle_root(&manifest);
    println!("  root: {}", hex::encode(&root_hash));

    if check {
        let wav_bytes = read_wav(&seed_path)?;
        let plaintext = decrypt(key, &wav_bytes)?;
        let text = String::from_utf8(plaintext).context("decrypted seed is not valid UTF-8")?;
        let (old_manifest, old_root) = text_to_manifest(&text)?;
        let clean = report_diff(&old_manifest, &manifest);
        if clean && old_root == root_hash {
            println!("clean: matches {}", seed_path.display());
            Ok(())
        } else {
            bail!("drift detected against {}", seed_path.display());
        }
    } else {
        let text = manifest_to_text(&manifest, &root_hash);
        let ciphertext = encrypt(key, text.as_bytes())?;
        write_wav(&seed_path, &ciphertext)?;
        println!(
            "wrote {} ({} files, {} bytes encrypted)",
            seed_path.display(),
            manifest.len(),
            ciphertext.len()
        );
        Ok(())
    }
}
