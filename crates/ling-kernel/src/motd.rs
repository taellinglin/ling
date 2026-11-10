//! The message of the day: a note shown when someone logs in over SSH (`motd set <file>` / `motd clear`).
//!
//! Kept in lingfs as `/motd` (system-wide, like `/hostname`), at most 4 KiB of text. Control and escape
//! characters are dropped when it is set, so a note can't send terminal escape sequences to whoever logs in.

use alloc::string::String;
use alloc::vec::Vec;

const PATH: &str = "motd";
const MAX_BYTES: usize = 4096;

/// The current message, if one is set.
pub fn get() -> Option<Vec<u8>> {
    crate::vfs::read(PATH).filter(|b| !b.is_empty())
}

/// Keep printable text, newlines and tabs (a `\r` goes; the SSH side adds its own).
fn clean(b: &[u8]) -> Vec<u8> {
    b.iter().copied().filter(|&c| c == b'\n' || c == b'\t' || (0x20..0x7f).contains(&c) || c >= 0x80).collect()
}

/// `motd [show | set <file> | clear]`; `file_path` turns the argument into a path (the caller's working
/// directory rules). Returns what to say to the user.
pub fn run(arg: &str, file_path: &dyn Fn(&str) -> String) -> String {
    let arg = arg.trim();
    let (sub, rest) = match arg.split_once(char::is_whitespace) {
        Some((s, r)) => (s, r.trim()),
        None => (arg, ""),
    };
    match sub {
        "" | "show" => match get() {
            Some(b) => {
                let mut s = String::from_utf8_lossy(&b).into_owned();
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s
            },
            None => String::from("motd: none set (motd set <file> to set one)\n"),
        },
        "set" => {
            if rest.is_empty() {
                return String::from("usage: motd set <file>\n");
            }
            let path = file_path(rest);
            let Some(data) = crate::vfs::read(&path) else {
                return alloc::format!("motd: cannot read {rest}\n");
            };
            if crate::vfs::is_dir(&path) {
                return alloc::format!("motd: {rest} is a folder\n");
            }
            let text = clean(&data);
            if text.iter().all(|b| b.is_ascii_whitespace()) {
                return String::from("motd: that file has no text in it (use motd clear to remove the message)\n");
            }
            if text.len() > MAX_BYTES {
                return alloc::format!("motd: {rest} is {} bytes; the most is {MAX_BYTES}\n", text.len());
            }
            if crate::vfs::write("/motd", &text) {
                alloc::format!("motd: set from {rest} ({} bytes); it is shown at SSH login\n", text.len())
            } else {
                String::from("motd: could not store it (is storage mounted?)\n")
            }
        },
        "clear" | "off" | "remove" => {
            if get().is_none() {
                return String::from("motd: none was set\n");
            }
            if crate::vfs::delete(PATH) {
                String::from("motd: cleared\n")
            } else {
                String::from("motd: could not clear it\n")
            }
        },
        _ => String::from("usage: motd [show | set <file> | clear]\n"),
    }
}
