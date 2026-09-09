//! Trust-on-first-use host key store, `.lingtp/known_hosts` — same model as
//! this workspace's SSH client. One line per host: `"{host}:{port} {pubkey_hex}"`.
//! A host whose recorded key doesn't match the one just presented is refused
//! outright (never silently overwritten) — that's what catches a MITM or a
//! server that quietly rotated keys without telling anyone.

use std::io::Write;
use std::path::PathBuf;

pub fn known_hosts_path() -> PathBuf {
    PathBuf::from(".lingtp/known_hosts")
}

/// `Ok(true)` — known and matches. `Ok(false)` — first time seeing this host.
/// `Err(())` — known, but the key changed; caller must refuse the connection.
pub fn check_known_host(host_port: &str, pubkey_hex: &str) -> Result<bool, ()> {
    let Ok(text) = std::fs::read_to_string(known_hosts_path()) else {
        return Ok(false);
    };
    for line in text.lines() {
        if let Some((h, k)) = line.split_once(' ') {
            if h == host_port {
                return if k == pubkey_hex { Ok(true) } else { Err(()) };
            }
        }
    }
    Ok(false)
}

pub fn learn_known_host(host_port: &str, pubkey_hex: &str) {
    let path = known_hosts_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{host_port} {pubkey_hex}");
    }
}
