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

/// Quote non-ASCII bare words so the strict, spec-compliant `toml` crate
/// accepts manifests written with CJK/Thai/Korean keys (`[灵符]`,
/// `名 = "..."`, `灵林 = { 邮 = "..." }`) the way `ling`'s own build-time
/// manifest reader (a tolerant hand-rolled line parser, see
/// `parse_lingfu_toml` in `ling/src/main.rs`) already does — TOML restricts
/// bare keys to `[A-Za-z0-9_-]+`, so `[灵符]` is technically invalid TOML
/// even though every non-English `lingfu new`/`init` scaffold (and the
/// project's own `hello_world/灵符.toml` example) generates exactly that.
///
/// Valid TOML has no non-ASCII bare *values* either (the only unquoted
/// value types — bool/number/date — are all ASCII), so any non-ASCII run
/// found outside a quoted string or comment must be an unquoted key, and is
/// always safe to quote: a single left-to-right pass that tracks whether
/// it's inside a string (so value text, including inside `"..."`, is never
/// touched) rather than trying to special-case table headers vs. `key =
/// value` lines vs. inline-table keys separately.
fn quote_non_ascii_keys(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars().peekable();
    let mut in_string: Option<char> = None;
    let mut word = String::new();

    macro_rules! flush_word {
        () => {
            if !word.is_empty() {
                if word.is_ascii() {
                    out.push_str(&word);
                } else {
                    out.push_str(&format!("{word:?}"));
                }
                word.clear();
            }
        };
    }

    while let Some(c) = chars.next() {
        if let Some(q) = in_string {
            out.push(c);
            if c == '\\' && q == '"' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
                continue;
            }
            if c == q {
                in_string = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                flush_word!();
                in_string = Some(c);
                out.push(c);
            },
            '#' => {
                flush_word!();
                out.push(c);
                for c2 in chars.by_ref() {
                    out.push(c2);
                    if c2 == '\n' {
                        break;
                    }
                }
            },
            '=' | '.' | ',' | '{' | '}' | '[' | ']' | ' ' | '\t' | '\r' | '\n' => {
                flush_word!();
                out.push(c);
            },
            _ => word.push(c),
        }
    }
    flush_word!();
    out
}

/// Read and parse a manifest as TOML, tolerating non-ASCII bare keys.
fn parse_manifest(manifest: &Path) -> Result<toml::Value> {
    let text = std::fs::read_to_string(manifest)
        .with_context(|| format!("reading manifest {}", manifest.display()))?;
    quote_non_ascii_keys(&text).parse().context("parsing manifest TOML")
}

/// Reads `(name, version, description)` from the project manifest, trying
/// every language's section/key spelling.
fn manifest_meta(manifest: &Path) -> Result<(String, String, String)> {
    let value = parse_manifest(manifest)?;

    // Section names that hold package metadata across the lexicons.
    let sections = ["spirit", "package", "灵符", "霊符", "영부", "ลิงฟู"];
    let name_keys = ["name", "名", "이름", "ชื่อ"];
    // "เวอร์ชัน" is what the Thai scaffolder actually emits; "รุ่น" is the
    // native synonym — accept both. "版本" covers the zh scaffold too.
    let version_keys = ["version", "版", "版本", "버전", "รุ่น", "เวอร์ชัน"];
    let desc_keys = [
        "description", "desc", "描述", "説明", "설명", "คำอธิบาย", "รายละเอียด",
    ];

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

/// Optional package metadata read from the manifest: keywords, homepage,
/// repository, license, authors. All default to empty strings when absent.
struct Extra {
    keywords: String,
    homepage: String,
    repository: String,
    license: String,
    authors: String,
}

fn manifest_extra(manifest: &Path) -> Extra {
    let empty = || Extra {
        keywords: String::new(),
        homepage: String::new(),
        repository: String::new(),
        license: String::new(),
        authors: String::new(),
    };
    let Ok(value) = parse_manifest(manifest) else { return empty() };

    let pkg_sections = ["spirit", "package", "灵符", "霊符", "영부", "ลิงฟู"];
    let author_sections = ["author", "authors", "灵者", "作者", "저자", "ผู้สร้าง"];

    // Look up a key (any of `keys`) across the package sections; a value may be
    // a string or an array of strings (joined by ", ").
    let find = |keys: &[&str]| -> String {
        for sec in &pkg_sections {
            if let Some(tbl) = value.get(sec).and_then(|v| v.as_table()) {
                for k in keys {
                    match tbl.get(*k) {
                        Some(toml::Value::String(s)) => return s.clone(),
                        Some(toml::Value::Array(a)) => {
                            let parts: Vec<String> = a
                                .iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect();
                            if !parts.is_empty() {
                                return parts.join(", ");
                            }
                        },
                        _ => {},
                    }
                }
            }
        }
        String::new()
    };

    // Authors: an explicit key, else the display-name keys of an author section.
    let mut authors = find(&["authors", "author", "ผู้เขียน"]);
    if authors.is_empty() {
        for sec in &author_sections {
            if let Some(tbl) = value.get(sec).and_then(|v| v.as_table()) {
                let names: Vec<String> = tbl.keys().cloned().collect();
                if !names.is_empty() {
                    authors = names.join(", ");
                    break;
                }
            }
        }
    }

    Extra {
        keywords: find(&["keywords", "คำสำคัญ", "คีย์เวิร์ด", "关键词"]),
        homepage: find(&["homepage", "หน้าหลัก", "โฮมเพจ", "主页"]),
        repository: find(&["repository", "repo", "ที่เก็บ", "仓库"]),
        license: find(&["license", "ใบอนุญาต", "สัญญาอนุญาต", "许可证"]),
        authors,
    }
}

/// Reads a README from the project root (first of a few common names),
/// truncated to a sane size for a form field.
fn read_readme(root: &Path) -> String {
    for name in ["README.md", "README.ling", "README.txt", "README", "อ่านฉัน.md"] {
        if let Ok(s) = std::fs::read_to_string(root.join(name)) {
            return s.chars().take(20_000).collect();
        }
    }
    String::new()
}

/// Gzip-tars `dir` (skipping build/VCS dirs and anything matched by
/// `ling.ignore`) into memory, returns the bytes.
///
/// Unlike `git ls-files`-backed operations, this walks the real filesystem —
/// it has no `.gitignore` awareness at all, so a `ling.ignore` at the
/// project root is the only way to keep something (an oversized local-only
/// asset, a keyfile that ended up sitting in the project directory) out of
/// an uploaded package short of physically moving it first.
fn build_tarball(dir: &Path) -> Result<Vec<u8>> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let ignore_patterns = crate::ignore::load(dir);
    let gz = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = tar::Builder::new(gz);

    for entry in walkdir::WalkDir::new(dir).into_iter().filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        // Skip VCS, build output, and the shared target dir.
        if matches!(
            name.as_ref(),
            ".git" | ".ling-build" | "target" | ".ling-shared-target" | "dist"
        ) {
            return false;
        }
        let Ok(rel) = e.path().strip_prefix(dir) else { return true };
        let rel = rel.to_string_lossy().replace('\\', "/");
        rel.is_empty() || !crate::ignore::is_ignored(&ignore_patterns, &rel)
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

    let extra = manifest_extra(&manifest);
    let readme = read_readme(&root);

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
            ("keywords", extra.keywords.as_str()),
            ("homepage", extra.homepage.as_str()),
            ("repository", extra.repository.as_str()),
            ("license", extra.license.as_str()),
            ("authors", extra.authors.as_str()),
            ("readme", readme.as_str()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_cjk_table_header() {
        let out = quote_non_ascii_keys("[灵符]\n");
        assert_eq!(out, "[\"灵符\"]\n");
        out.parse::<toml::Value>().expect("should now parse");
    }

    #[test]
    fn quotes_cjk_key_value() {
        let out = quote_non_ascii_keys("名 = \"tensormap\"\n");
        assert_eq!(out, "\"名\" = \"tensormap\"\n");
    }

    #[test]
    fn quotes_nested_inline_table_key() {
        let out = quote_non_ascii_keys("灵林 = { 邮 = \"a@b.com\" }\n");
        assert_eq!(out, "\"灵林\" = { \"邮\" = \"a@b.com\" }\n");
        out.parse::<toml::Value>().expect("should now parse");
    }

    #[test]
    fn leaves_ascii_manifest_untouched() {
        let text = "[spirit]\nname = \"x\"\nversion = \"1.0.0\"\n";
        assert_eq!(quote_non_ascii_keys(text), text);
    }

    #[test]
    fn leaves_string_values_untouched_even_with_non_ascii_content() {
        let out = quote_non_ascii_keys("描述 = \"你好 = world\"\n");
        assert_eq!(out, "\"描述\" = \"你好 = world\"\n");
    }

    #[test]
    fn full_hello_world_style_manifest_parses() {
        let text = "[灵符]\n名 = \"hello_world\"\n型 = \"独行\"\n版 = \"2030.0.0\"\n\n[灵者]\n灵林 = { 邮 = \"taellinglin@gmail.com\" }\n";
        let quoted = quote_non_ascii_keys(text);
        let value: toml::Value = quoted.parse().expect("should parse");
        assert_eq!(
            value["灵符"]["名"].as_str(),
            Some("hello_world")
        );
    }
}
