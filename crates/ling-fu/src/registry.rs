//! Registry client for `lingfu publish` / `login` / `logout`.
//!
//! Talks to a Ling package registry (fu.ling-lang.org by default) over the
//! same HTTP API the `.ling` server implements: an API-key bearer token
//! authorizes a multipart-free form POST carrying the built, gzip-tarred
//! project as base64. Credentials live in `~/.lingfu/credentials.toml`,
//! mirroring how cargo stores registry tokens.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

const DEFAULT_REGISTRY: &str = "https://fu.ling-lang.org";

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Credentials {
    registry: Option<String>,
    token: Option<String>,
}

fn credentials_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot locate home directory")?;
    Ok(home.join(".lingfu").join("credentials.toml"))
}

fn load_credentials() -> Credentials {
    credentials_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_credentials(c: &Credentials) -> Result<()> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, toml::to_string_pretty(c)?)?;
    Ok(())
}

/// The registry base URL: `LINGFU_REGISTRY` env override, else the saved
/// value, else the public default.
fn registry_url() -> String {
    if let Ok(url) = std::env::var("LINGFU_REGISTRY") {
        return url.trim_end_matches('/').to_string();
    }
    load_credentials()
        .registry
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// `lingfu login <api-key>` — save an API key (created in the web dev portal
/// at /me/keys). With no argument, reads the key from stdin.
pub fn login(args: &[String]) -> Result<()> {
    let token = match args.iter().find(|a| !a.starts_with('-')) {
        Some(t) => t.clone(),
        None => {
            print!("paste your lingfu API key: ");
            std::io::stdout().flush().ok();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.trim().to_string()
        },
    };
    if token.is_empty() {
        bail!("no API key provided");
    }
    let registry = args
        .iter()
        .position(|a| a == "--registry")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let mut creds = load_credentials();
    creds.token = Some(token);
    if let Some(r) = registry {
        creds.registry = Some(r);
    } else if creds.registry.is_none() {
        creds.registry = Some(DEFAULT_REGISTRY.to_string());
    }
    save_credentials(&creds)?;
    Ok(())
}

/// `lingfu logout` — forget the saved API key.
pub fn logout() -> Result<()> {
    let mut creds = load_credentials();
    creds.token = None;
    save_credentials(&creds)?;
    Ok(())
}

/// Reads `(name, version, description)` from the project manifest, trying
/// every language's section/key spelling.
fn manifest_meta(manifest: &Path) -> Result<(String, String, String)> {
    let text = std::fs::read_to_string(manifest)
        .with_context(|| format!("reading manifest {}", manifest.display()))?;
    let value: toml::Value = text.parse().context("parsing manifest TOML")?;

    // Section names that hold package metadata across the lexicons.
    let sections = ["spirit", "package", "灵符", "霊符", "영부", "ลิงฟู"];
    let name_keys = ["name", "名", "이름", "ชื่อ"];
    let version_keys = ["version", "版", "버전", "รุ่น"];
    let desc_keys = ["description", "desc", "描述", "説明", "설명", "คำอธิบาย"];

    let find = |keys: &[&str]| -> Option<String> {
        for sec in &sections {
            if let Some(tbl) = value.get(sec).and_then(|v| v.as_table()) {
                for k in keys {
                    if let Some(s) = tbl.get(*k).and_then(|v| v.as_str()) {
                        return Some(s.to_string());
                    }
                }
            }
        }
        None
    };

    let name = find(&name_keys).context("manifest is missing a package name")?;
    let version = find(&version_keys).unwrap_or_else(|| "0.0.0".to_string());
    let description = find(&desc_keys).unwrap_or_default();
    Ok((name, version, description))
}

/// Gzip-tars `dir` (skipping build/VCS dirs) into memory, returns the bytes.
fn build_tarball(dir: &Path) -> Result<Vec<u8>> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let gz = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = tar::Builder::new(gz);

    for entry in walkdir::WalkDir::new(dir).into_iter().filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        // Skip VCS, build output, and the shared target dir.
        !matches!(
            name.as_ref(),
            ".git" | ".ling-build" | "target" | ".ling-shared-target" | "dist"
        )
    }) {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let rel = path.strip_prefix(dir).unwrap_or(path);
            tar.append_path_with_name(path, rel)?;
        }
    }

    let gz = tar.into_inner()?;
    let bytes = gz.finish()?;
    Ok(bytes)
}

/// `lingfu publish` — build a tarball of the current project and upload it to
/// the registry, authenticated with the saved API key.
pub fn publish(args: &[String]) -> Result<()> {
    let creds = load_credentials();
    let token = creds
        .token
        .clone()
        .context("not logged in — run `lingfu login <api-key>` first (create a key at <registry>/me/keys)")?;
    let registry = registry_url();

    // Locate the manifest (walk up), and use its directory as the project root.
    let manifest = crate::find_manifest().context(
        "no Ling manifest found (Ling.toml / 灵符.toml / …) — run this inside a project",
    )?;
    let root = manifest
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let (name, mut version, description) = manifest_meta(&manifest)?;
    // Allow `lingfu publish --version x.y.z` to override the manifest version.
    if let Some(v) = args
        .iter()
        .position(|a| a == "--version")
        .and_then(|i| args.get(i + 1))
    {
        version = v.clone();
    }

    let tarball = build_tarball(&root)?;
    if tarball.len() < 3 || tarball[0] != 0x1f || tarball[1] != 0x8b {
        bail!("failed to build a valid gzip artifact");
    }
    use base64::Engine as _;
    let artifact_b64 = base64::engine::general_purpose::STANDARD.encode(&tarball);

    let url = format!("{registry}/api/v1/packages/publish");
    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .send_form(&[
            ("name", name.as_str()),
            ("version", version.as_str()),
            ("description", description.as_str()),
            ("artifact_b64", artifact_b64.as_str()),
        ]);

    match resp {
        Ok(r) => {
            let body = r.into_string().unwrap_or_default();
            println!("{body}");
            Ok(())
        },
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            bail!("registry rejected publish ({code}): {body}");
        },
        Err(e) => bail!("could not reach registry {registry}: {e}"),
    }
}

/// The active registry URL, for status messages.
pub fn active_registry() -> String {
    registry_url()
}
