//! The SSH shell's commands for working on the machine: files (ls, cat, cd, stat, chmod, rm, mkdir,
//! touch, mv, cp), the machine (info, df, running), the desktop (open, close, lock) and the login
//! message (motd). Each returns the text to show, so none of it depends on the SSH session code.
//!
//! The files are the same view SFTP gives: `home/`, the SD card and removable media, through `vfs`,
//! with the `chmod` modes (modes.rs) honoured. Commands the Pi console already has (apps, ps, date,
//! wifi, lingfu, miao, ling run, reboot, ...) are not repeated here: the SSH shell hands those to
//! the console. Pipes and redirects are not supported.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Split a command's arguments on spaces; "double" and 'single' quotes keep spaces inside one argument.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(c);
                any = true;
            },
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(core::mem::take(&mut cur));
                    any = false;
                }
            },
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A path argument as a vfs path: "/x" from the root, "~" the home folder, anything else under `cwd`.
fn vpath(cwd: &str, user: &str, a: &str) -> String {
    if let Some(abs) = a.strip_prefix('/') {
        crate::vfs::norm(abs)
    } else if a == "~" {
        format!("home/{user}")
    } else if let Some(rest) = a.strip_prefix("~/") {
        crate::vfs::join(&format!("home/{user}"), rest)
    } else {
        crate::vfs::join(cwd, a)
    }
}

fn in_home(p: &str) -> bool {
    p == "home" || p.starts_with("home/")
}

/// `Some((is_dir, size))` for an existing path.
fn stat_of(p: &str) -> Option<(bool, usize)> {
    if p.is_empty() {
        return Some((true, 0));
    }
    if crate::vfs::is_dir(p) {
        return Some((true, 0));
    }
    let parent = crate::vfs::parent(p);
    let base = crate::vfs::basename(p);
    if let Some((_, _, size)) = crate::vfs::list(&parent).into_iter().find(|(n, _, _)| *n == base) {
        return Some((false, size));
    }
    if crate::vfs::exists(p) {
        return Some((false, crate::vfs::read(p).map(|v| v.len()).unwrap_or(0)));
    }
    None
}

fn mode_of(p: &str, is_dir: bool) -> u32 {
    if in_home(p) {
        crate::modes::get(p, is_dir)
    } else if is_dir {
        crate::modes::DEFAULT_DIR
    } else {
        crate::modes::DEFAULT_FILE
    }
}

/// Run `name arg`; None when it is not one of these commands (the caller tries the console next).
pub fn run(name: &str, arg: &str, user: &str, cwd: &mut String) -> Option<String> {
    let args = split_args(arg);
    let flags: Vec<&str> = args.iter().filter(|s| s.starts_with('-') && s.len() > 1).map(|s| s.as_str()).collect();
    let plain: Vec<&String> = args.iter().filter(|s| !(s.starts_with('-') && s.len() > 1)).collect();
    let has = |f: char| flags.iter().any(|x| x.trim_start_matches('-').contains(f));
    Some(match name {
        "prof" => crate::prof::command(arg),
        "cores" => crate::par::command(arg),
        "bench3d" => crate::drivers::g3d::bench(arg),
        #[cfg(target_arch = "x86_64")]
        "gpu" => crate::drivers::nvgpu::command(arg),
        #[cfg(target_arch = "x86_64")]
        "nvme" => crate::drivers::nvme::command(arg),
        #[cfg(target_arch = "x86_64")]
        "report" => report(),
        #[cfg(target_arch = "x86_64")]
        "wifi-intel" => crate::drivers::iwlwifi::command(arg),
        #[cfg(target_arch = "x86_64")]
        "cpu" => crate::arch::cpudrv::command(arg),
        #[cfg(target_arch = "x86_64")]
        "audio" => crate::drivers::mixer::command(arg),
        #[cfg(target_arch = "x86_64")]
        "update-boot" => {
            let mut s = String::new();
            for (_, l) in crate::drivers::installer::update_boot() {
                s.push_str(&l);
                s.push('\n');
            }
            s
        },
        // rename-user <new> <password>: renames the account this session signed in as
        "rename-user" => match (plain.first(), plain.get(1)) {
            (Some(new), Some(pw)) if plain.len() == 2 => match crate::fs::users::rename(user, new, pw) {
                Ok(m) => {
                    if let Some(rest) = cwd.strip_prefix(&format!("home/{user}")) {
                        *cwd = format!("home/{new}{rest}");
                    }
                    format!("{m}\n")
                },
                Err(e) => format!("rename-user: {e}\n"),
            },
            _ => String::from("usage: rename-user <new name> <your password>\n"),
        },
        "help" => help(),
        "pwd" => format!("/{cwd}\n"),
        "cd" => {
            let target = match plain.first() {
                None => format!("home/{user}"),
                Some(p) => vpath(cwd, user, p),
            };
            if target.is_empty() || crate::vfs::is_dir(&target) {
                *cwd = target;
                String::new()
            } else {
                format!("cd: {}: no such directory\n", plain[0])
            }
        },
        "ls" | "dir" => ls(cwd, user, &plain, has('l')),
        "cat" => {
            let Some(p) = plain.first() else { return Some(String::from("usage: cat <file>\n")) };
            let path = vpath(cwd, user, p);
            match crate::vfs::read(&path) {
                Some(b) if !crate::vfs::is_dir(&path) => {
                    let cut = b.len() > 256 * 1024;
                    let mut s = String::from_utf8_lossy(&b[..b.len().min(256 * 1024)]).into_owned();
                    if cut {
                        s.push_str("\n[cat: stopped at 256 KiB -- download the file with sftp for the rest]\n");
                    }
                    s
                },
                _ => format!("cat: {p}: no such file\n"),
            }
        },
        "stat" => {
            let Some(p) = plain.first() else { return Some(String::from("usage: stat <path>\n")) };
            let path = vpath(cwd, user, p);
            match stat_of(&path) {
                Some((d, size)) => {
                    let m = mode_of(&path, d);
                    format!(
                        "  Path: /{path}\n  Type: {}\n  Size: {size} bytes\n  Mode: {m:04o} ({}){}\n",
                        if d { "folder" } else { "file" },
                        crate::modes::symbolic(m, d),
                        if in_home(&path) && !crate::modes::writable(&path) { "  read-only" } else { "" }
                    )
                },
                None => format!("stat: {p}: no such file or directory\n"),
            }
        },
        "chmod" => chmod(cwd, user, &plain, has('R')),
        "rm" | "del" => rm(cwd, user, &plain, has('r') || has('R'), has('f'), user),
        "mkdir" => {
            if plain.is_empty() {
                return Some(String::from("usage: mkdir [-p] <folder>...\n"));
            }
            let mut out = String::new();
            for p in &plain {
                let path = vpath(cwd, user, p);
                if crate::vfs::exists(&path) {
                    if !has('p') {
                        out.push_str(&format!("mkdir: {p}: already exists\n"));
                    }
                } else if !has('p') && !crate::vfs::parent(&path).is_empty() && !crate::vfs::is_dir(&crate::vfs::parent(&path)) {
                    out.push_str(&format!("mkdir: {p}: the folder above it does not exist (-p makes it)\n"));
                } else if !crate::vfs::mkdir(&path) {
                    out.push_str(&format!("mkdir: {p}: could not create it\n"));
                }
            }
            out
        },
        "touch" => {
            let mut out = String::new();
            for p in &plain {
                let path = vpath(cwd, user, p);
                if !crate::vfs::exists(&path) && !crate::vfs::write(&path, b"") {
                    out.push_str(&format!("touch: {p}: could not create it\n"));
                }
            }
            if plain.is_empty() { String::from("usage: touch <file>...\n") } else { out }
        },
        "mv" | "cp" => {
            if plain.len() != 2 {
                return Some(format!("usage: {name} {}<from> <to>\n", if name == "cp" { "[-r] " } else { "" }));
            }
            let from = vpath(cwd, user, plain[0]);
            let Some((is_dir, _)) = stat_of(&from) else { return Some(format!("{name}: {}: no such file or directory\n", plain[0])) };
            let to_arg = vpath(cwd, user, plain[1]);
            let to = crate::vfs::target_for(&from, &to_arg);
            if is_dir && (to == from || to.starts_with(&format!("{from}/"))) {
                return Some(format!("{name}: cannot put a folder inside itself\n"));
            }
            if name == "cp" {
                if is_dir && !(has('r') || has('R')) {
                    return Some(format!("cp: {} is a folder (use -r)\n", plain[0]));
                }
                if crate::vfs::copy(&from, &to) { String::new() } else { format!("cp: could not copy to {} (read-only, or no room)\n", plain[1]) }
            } else if crate::vfs::rename(&from, &to) {
                String::new()
            } else {
                format!("mv: could not move to {} (a read-only file in the way, or no room)\n", plain[1])
            }
        },
        "df" => df(),
        "sha256sum" => sha256sum(cwd, user, &plain),
        // LingDiffusion's checkpoint scan (the .safetensors files it would offer)
        #[cfg(target_arch = "x86_64")]
        "models" => {
            let m = crate::drivers::diffusion::find_models();
            if m.is_empty() { String::from("no checkpoints found\n") } else { m.iter().map(|p| format!("/{p}\n")).collect() }
        },
        "info" | "sysinfo" | "neofetch" => info(user),
        "running" => running(),
        "open" | "launch" | "start" => desktop_cmd("open", &plain.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")),
        "close" | "quit-app" => desktop_cmd("close", &plain.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")),
        "lock" => desktop_cmd("lock", "screen"),
        "motd" => crate::motd::run(arg, &|f| vpath(cwd, user, f)),
        _ => return None,
    })
}

/// `sha256sum <file> [offset [length]]`: files on media volumes are read in
/// pieces (any size, e.g. a 6 GB checkpoint on exFAT), others whole.
fn sha256sum(cwd: &str, user: &str, plain: &[&String]) -> String {
    use sha2::Digest;
    let Some(p) = plain.first() else { return String::from("usage: sha256sum <file> [offset [length]]\n") };
    let path = vpath(cwd, user, p);
    let num = |i: usize| plain.get(i).and_then(|s| s.parse::<u64>().ok());
    let mut h = sha2::Sha256::new();
    let (off, len);
    if let Some(r) = crate::fs::media::open_range(&path) {
        off = num(1).unwrap_or(0).min(r.size());
        len = num(2).unwrap_or(r.size() - off).min(r.size() - off);
        let mut buf = alloc::vec![0u8; 1 << 20];
        let mut at = off;
        while at < off + len {
            let n = ((off + len - at) as usize).min(buf.len());
            if !r.read_at(at, &mut buf[..n]) {
                return format!("sha256sum: {p}: read failed at byte {at}\n");
            }
            h.update(&buf[..n]);
            at += n as u64;
        }
    } else {
        let Some(b) = crate::vfs::read(&path) else { return format!("sha256sum: {p}: no such file\n") };
        off = num(1).unwrap_or(0).min(b.len() as u64);
        len = num(2).unwrap_or(b.len() as u64 - off).min(b.len() as u64 - off);
        h.update(&b[off as usize..(off + len) as usize]);
    }
    let d: [u8; 32] = h.finalize().into();
    let hex: String = d.iter().map(|x| format!("{x:02x}")).collect();
    format!("{hex}  {p}  (bytes {off}..{})\n", off + len)
}

fn help() -> String {
    String::from(
        "files:    ls [-l] [dir]   cat <file>   cd <dir>   pwd   stat <path>\n\
         \x20         mkdir [-p] <dir>   touch <file>   cp [-r] <a> <b>   mv <a> <b>\n\
         \x20         rm [-r] [-f] <path>   chmod [-R] <mode> <path>   (644, 755, u+w, a-w ...)\n\
         \x20         sha256sum <file> [offset [length]]   models (LingDiffusion checkpoints)\n\
         machine:  info   df   running   uptime   free   hostname   date   board   net\n\
         desktop:  open <app>   close <app>   lock\n\
         login:    motd [show | set <file> | clear]\n\
         audio:    audio   audio out <0|1|auto>   audio tone [hz] [ms]   (x86: onboard / USB output)
         profile:  prof [frames] | prof show | prof stop   (frame profiler for the running program)\n\
         other:    apps   ps   wifi   lingfu   miao <rite>   ling run <file>   reboot   poweroff\n\
         \x20         echo   whoami   uname   get <url>   clear   help   exit\n\
         upload and download: sftp / scp (an upload replaces the file; chmod 444 makes one read-only)\n",
    )
}

fn ls(cwd: &str, user: &str, plain: &[&String], long: bool) -> String {
    let target = match plain.first() {
        Some(p) => vpath(cwd, user, p),
        None => cwd.to_string(),
    };
    let mut out = String::new();
    let line = |out: &mut String, name: &str, full: &str, d: bool, size: usize| {
        if long {
            let m = mode_of(full, d);
            out.push_str(&format!("{} {:<8} {:>10} {}{}\n", crate::modes::symbolic(m, d), user, size, name, if d { "/" } else { "" }));
        } else {
            out.push_str(name);
            if d {
                out.push('/');
            }
            out.push('\n');
        }
    };
    if !target.is_empty() && !crate::vfs::is_dir(&target) {
        return match stat_of(&target) {
            Some((d, size)) => {
                line(&mut out, &crate::vfs::basename(&target), &target, d, size);
                out
            },
            None => format!("ls: {}: no such file or directory\n", plain.first().map(|s| s.as_str()).unwrap_or("")),
        };
    }
    let entries = crate::vfs::list(&target);
    if entries.is_empty() {
        return String::from("(empty)\n");
    }
    for (name, d, size) in entries.into_iter().take(1000) {
        let full = crate::vfs::join(&target, &name);
        line(&mut out, &name, &full, d, size);
    }
    out
}

fn chmod(cwd: &str, user: &str, plain: &[&String], recursive: bool) -> String {
    if plain.len() < 2 {
        return String::from("usage: chmod [-R] <mode> <path>...   (mode: 644, 755, u+w, a-w, go-rwx ...)\n");
    }
    let spec = plain[0].as_str();
    let mut out = String::new();
    for p in &plain[1..] {
        let path = vpath(cwd, user, p);
        let Some((d, _)) = stat_of(&path) else {
            out.push_str(&format!("chmod: {p}: no such file or directory\n"));
            continue;
        };
        if !in_home(&path) || path == "home" {
            out.push_str(&format!("chmod: {p}: only files under home/ have permissions\n"));
            continue;
        }
        chmod_one(&path, d, spec, recursive, &mut out, p);
    }
    out
}

fn chmod_one(path: &str, d: bool, spec: &str, recursive: bool, out: &mut String, shown: &str) {
    let cur = crate::modes::get(path, d);
    let Some(mode) = crate::modes::parse_chmod(spec, cur) else {
        out.push_str(&format!("chmod: invalid mode '{spec}'\n"));
        return;
    };
    if !crate::modes::set(path, mode, d) {
        out.push_str(&format!("chmod: {shown}: could not keep the permissions\n"));
        return;
    }
    if d && recursive {
        for (name, sub_dir, _) in crate::vfs::list(path) {
            chmod_one(&crate::vfs::join(path, &name), sub_dir, spec, true, out, &name);
        }
    }
}

fn rm(cwd: &str, user: &str, plain: &[&String], recursive: bool, force: bool, owner: &str) -> String {
    if plain.is_empty() {
        return String::from("usage: rm [-r] [-f] <path>...\n");
    }
    let mut out = String::new();
    for p in plain {
        let path = vpath(cwd, user, p);
        // never the roots people stand on
        if path.is_empty() || path == "home" || path == format!("home/{owner}") || path == "media" || path.starts_with("media") && !path.contains('/') {
            out.push_str(&format!("rm: refusing to remove {p}\n"));
            continue;
        }
        let Some((is_dir, _)) = stat_of(&path) else {
            if !force {
                out.push_str(&format!("rm: {p}: no such file or directory\n"));
            }
            continue;
        };
        if is_dir && !recursive {
            if !crate::vfs::list(&path).is_empty() {
                out.push_str(&format!("rm: {p} is a folder with files in it (use -r)\n"));
                continue;
            }
        }
        let ok = if is_dir && recursive { crate::vfs::delete_tree(&path) } else { crate::vfs::delete(&path) };
        if !ok && crate::vfs::exists(&path) {
            if in_home(&path) && !crate::modes::writable(&path) {
                out.push_str(&format!("rm: {p}: read-only (chmod u+w {p} first)\n"));
            } else {
                out.push_str(&format!("rm: {p}: could not remove it (the image's built-in files stay)\n"));
            }
        }
    }
    out
}

/// `report`: everything needed to debug this machine from another one --
/// the hardware commands' output plus the whole boot log -- written to the
/// first FAT drive (the USB stick LingOS booted from, normally) as
/// `lingos-report.txt`. Screens cut long lines off; this file doesn't.
#[cfg(target_arch = "x86_64")]
fn report() -> String {
    let mut r = String::new();
    let sec = |r: &mut String, title: &str, body: String| {
        r.push_str(&format!("===== {title}\n{body}\n"));
    };
    sec(&mut r, "cpu", crate::arch::cpudrv::command(""));
    sec(&mut r, "cores", crate::par::command(""));
    sec(&mut r, "nvme", crate::drivers::nvme::command(""));
    sec(&mut r, "nic", crate::drivers::netdev::status_line());
    sec(&mut r, "wifi-intel", crate::drivers::iwlwifi::command(""));
    sec(&mut r, "wifi-intel regs", crate::drivers::iwlwifi::command("regs"));
    sec(&mut r, "gpu", crate::drivers::nvgpu::command(""));
    sec(&mut r, "df", df());
    let log = crate::drivers::serial::klog();
    sec(&mut r, "boot log", String::from_utf8_lossy(&log).into_owned());
    let names = crate::fs::media::names();
    let Some(drive) = names.first() else {
        return String::from("report: no FAT drive (USB stick) mounted to write to\n");
    };
    let path = format!("media/{drive}/lingos-report.txt");
    if crate::vfs::write(&path, r.as_bytes()) {
        format!("report: wrote {} KB to {path} -- plug the stick into the other PC\n", r.len() / 1024)
    } else {
        format!("report: could not write {path} (stick write-protected or full?)\n")
    }
}

fn df() -> String {
    let mut s = String::new();
    let free = crate::mm::frame::free_frame_count() * 4 / 1024;
    s.push_str(&format!("memory   {free} MiB free\n"));
    #[cfg(target_arch = "aarch64")]
    {
        let sectors = crate::arch::sdcard::sectors();
        if sectors > 0 {
            s.push_str(&format!("sd card  {} MiB\n", sectors / 2048));
        }
    }
    for n in crate::fs::media::names() {
        s.push_str(&format!("media/{n}\n"));
    }
    s
}

fn secs_words(mut t: u64) -> String {
    let d = t / 86400;
    t %= 86400;
    let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
    if d > 0 { format!("{d}d {h}h {m}m {s}s") } else if h > 0 { format!("{h}h {m}m {s}s") } else { format!("{m}m {s}s") }
}

fn info(user: &str) -> String {
    let mut s = String::new();
    let host = crate::vfs::read("/hostname").map(|b| String::from_utf8_lossy(&b).trim().to_string()).filter(|h| !h.is_empty()).unwrap_or_else(|| String::from("lingos"));
    s.push_str(&format!("host      {host}  (user {user})\n"));
    #[cfg(target_arch = "aarch64")]
    {
        let board = match crate::arch::mmio::board() {
            crate::arch::mmio::RpiBoard::Pi3 => "Raspberry Pi 3 (BCM2837, Cortex-A53)",
            crate::arch::mmio::RpiBoard::Pi4 => "Raspberry Pi 4 (BCM2711, Cortex-A72)",
            crate::arch::mmio::RpiBoard::Pi5 => "Raspberry Pi 5 (BCM2712, Cortex-A76)",
            crate::arch::mmio::RpiBoard::Unknown => "Raspberry Pi (unrecognized SoC)",
        };
        s.push_str(&format!("system    LingOS aarch64 -- {board}\n"));
    }
    #[cfg(target_arch = "x86_64")]
    s.push_str("system    LingOS x86_64\n");
    s.push_str(&format!("uptime    {}\n", secs_words(crate::arch::timer::now_ms() / 1000)));
    let free = crate::mm::frame::free_frame_count() * 4 / 1024;
    let total = crate::mm::ram_end() / (1024 * 1024);
    s.push_str(&format!("memory    {free} MiB free{}\n", if total > 0 { format!(" of {total} MiB address range") } else { String::new() }));
    let ip = crate::drivers::netstack::self_ip();
    let gw = crate::drivers::netstack::gateway_ip();
    s.push_str(&format!("network   {}.{}.{}.{}  gateway {}.{}.{}.{}\n", ip[0], ip[1], ip[2], ip[3], gw[0], gw[1], gw[2], gw[3]));
    if crate::drivers::framebuffer::available() {
        s.push_str(&format!(
            "display   {} x {}, 32-bit colour, {} Hz frame cap{}\n",
            crate::drivers::framebuffer::width(),
            crate::drivers::framebuffer::height(),
            crate::drivers::framebuffer::present_hz(),
            match crate::drivers::framebuffer::edid_summary() {
                Some(e) => format!("  (monitor native {e})"),
                None => String::from("  (no EDID from the firmware)"),
            }
        ));
    }
    #[cfg(target_arch = "aarch64")]
    {
        let sectors = crate::arch::sdcard::sectors();
        if sectors > 0 {
            s.push_str(&format!("storage   SD card {} MiB\n", sectors / 2048));
        }
    }
    let media = crate::fs::media::names();
    if !media.is_empty() {
        s.push_str(&format!("media     {}\n", media.iter().map(|n| format!("media/{n}")).collect::<Vec<_>>().join("  ")));
    }
    s.push_str(&format!(
        "login     SSH on port {}, two-factor {} ({} over SSH)\n",
        crate::drivers::sshd::port(),
        if crate::fs::twofa::enabled(user) { "on" } else { "off" },
        if crate::drivers::sshd::twofa_required() { "asked for" } else { "not asked for" }
    ));
    let up = running_list();
    s.push_str(&format!("apps      {}\n", if up.is_empty() { String::from("none running") } else { up.join(", ") }));
    s
}

fn running_list() -> Vec<String> {
    let mut v = crate::lingapp::apps_running();
    let front = crate::lingapp::active_app_name();
    if front != "desktop" && !v.contains(&front) {
        v.insert(0, front);
    }
    v
}

fn running() -> String {
    let up = running_list();
    if up.is_empty() {
        String::from("no apps running (the desktop only)\n")
    } else {
        format!("{}\n", up.join("\n"))
    }
}

/// Ask the desktop to open / close an app, or lock the screen.
fn desktop_cmd(verb: &str, what: &str) -> String {
    let what = what.trim();
    if verb != "lock" && what.is_empty() {
        return format!("usage: {verb} <app>   (apps lists the built-in ones; also files, settings, console, terminal)\n");
    }
    if !what.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'.' | b'-' | b'_')) {
        return String::from("that is not an app name\n");
    }
    let cmd = format!("{verb}:{}", what.to_ascii_lowercase());
    if !crate::remote::desktop_is_polling() {
        return String::from("no desktop is running to do that (it is on the console, or still starting)\n");
    }
    if crate::remote::push(&cmd) {
        match verb {
            "open" => format!("asked the desktop to open {what}\n"),
            "close" => format!("asked the desktop to close {what}\n"),
            _ => String::from("asked the desktop to lock the screen\n"),
        }
    } else {
        String::from("the desktop is busy with earlier requests; try again\n")
    }
}
