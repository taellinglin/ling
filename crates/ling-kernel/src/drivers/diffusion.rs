//! LingDiffusion on LingOS: Stable Diffusion txt2img for the bundled `lingdiffusion` app, two ways.
//!
//! * **GPU server** (the default when one answers): any LingDiffusion or AUTOMATIC1111 WebUI on the LAN, through its
//!   `/sdapi/v1` API over plain HTTP -- `txt2img` (base64 PNG back), `progress` (step, ETA, live preview) while it runs,
//!   `options` / `/api/engine` for what it renders on. LingOS cannot drive an NVIDIA card itself, so this is how a GPU is
//!   used: the server's. The address comes from Settings; otherwise one is looked for once in the background (the gateway,
//!   then the gateway's /24 on ports 7860 / 7861).
//! * **This PC's CPU**, fully offline: the `ling-sd` engine (SD 1.5: CLIP, UNet, VAE, Euler a / DPM++ 2M Karras) on every core
//!   (`par.rs`), with AVX2 + FMA kernels when the CPU and the kernel allow them (`cpudrv::avx2`), SSE otherwise. The checkpoint
//!   (a single-file fp16 `.safetensors`) is read from a FAT volume -- `media/<vol>/models/`, `.../models/Stable-diffusion/`,
//!   `.../Stable-diffusion/` -- and its ~2 GB of weights are held in RAM above 4 GiB when the machine has it (`mm::highmem`).
//!
//! `auto` uses the server when it answers and falls back to the CPU when it does not (or a request to it fails).
//!
//! Everything slow happens in "system" coroutines (they outlive the app's frames and keep running if it is parked): the job
//! task (load + generate, yielding every ~16 ms from inside the engine's rounds of work), the network task (probe / LAN search)
//! and, during a server job, a progress poller. The app polls the state each frame and draws the result / live preview with
//! `diffusion_draw`. Finished pictures are PNGs (with the WebUI's `parameters` text) in the user's Photos/LingDiffusion folder.
use crate::arch::timer;
use crate::drivers::netstack as ns;
use crate::lingrt::{boolean, list_from, mk_str, num, to_f64, to_string, UNIT};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

// ── state ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Loading,
    Working,
    Done,
    Failed,
}

#[derive(Clone)]
struct Req {
    backend: String,
    prompt: String,
    negative: String,
    sampler: String,
    steps: usize,
    cfg: f32,
    w: usize,
    h: usize,
    seed: u64,
}

struct Pic {
    /// 0x00RRGGBB
    px: Vec<u32>,
    w: usize,
    h: usize,
    png: Vec<u8>,
    info: String,
    seed: u64,
    path: String,
    by: String,
}

struct St {
    phase: Phase,
    msg: String,
    step: usize,
    steps: usize,
    frac: f32,
    t0: u64,
    t1: u64,
    /// seconds per step of the last local run
    sps: f32,
    pending: Option<Req>,
    result: Option<Pic>,
    /// live preview (0x00RRGGBB) while a job runs
    preview: Vec<u32>,
    pw: usize,
    ph: usize,
    /// the scaled copy last drawn: (source id, w, h, pixels)
    scaled: (u64, usize, usize, Vec<u32>),
    pic_id: u64,
    // server
    server_cfg: String,
    server_found: String,
    server_state: &'static str,
    server_info: String,
    /// the server is LingDiffusion (has the queued /api/generate)
    server_ld: bool,
    remote_job: String,
    server_checked: u64,
    want_probe: bool,
    want_search: bool,
    searching: bool,
    remote_busy: bool,
    // local engine
    model_path: String,
    loaded: Option<Box<ling_sd::Model>>,
    loaded_path: String,
    thumbs: Vec<(String, usize, usize, Vec<u32>)>,
}

static mut ST: Option<St> = None;
static mut TASK: u64 = 0;
static mut NET_TASK: u64 = 0;
static mut LAST_YIELD: u64 = 0;

fn st() -> &'static mut St {
    unsafe {
        (*(&raw mut ST)).get_or_insert_with(|| St {
            phase: Phase::Idle,
            msg: String::new(),
            step: 0,
            steps: 0,
            frac: 0.0,
            t0: 0,
            t1: 0,
            sps: 0.0,
            pending: None,
            result: None,
            preview: Vec::new(),
            pw: 0,
            ph: 0,
            scaled: (0, 0, 0, Vec::new()),
            pic_id: 1,
            server_cfg: String::new(),
            server_found: String::new(),
            server_state: "unknown",
            server_info: String::new(),
            server_ld: false,
            remote_job: String::new(),
            server_checked: 0,
            want_probe: false,
            want_search: false,
            searching: false,
            remote_busy: false,
            model_path: String::new(),
            loaded: None,
            loaded_path: String::new(),
            thumbs: Vec::new(),
        })
    }
}

fn log(s: &str) {
    crate::drivers::serial::write(s.as_bytes());
    crate::drivers::serial::write(b"\n");
}

/// From inside the engine's rounds and between disk pieces: hand the CPU to the desktop at most every 16 ms.
fn maybe_yield() {
    if !crate::coro::in_coro() {
        return;
    }
    let t = timer::now_us();
    unsafe {
        if t.wrapping_sub(LAST_YIELD) >= 16_000 {
            crate::coro::yield_now();
            LAST_YIELD = timer::now_us();
        }
    }
}

fn sd_parallel(n: usize, ctx: *const (), f: unsafe fn(*const (), usize)) {
    struct J(*const (), unsafe fn(*const (), usize));
    unsafe impl Sync for J {}
    impl J {
        fn run(&self, i: usize) {
            // SAFETY: ling-sd hands over a context that outlives the call and a function for it
            unsafe { (self.1)(self.0, i) }
        }
    }
    let j = J(ctx, f);
    crate::par::for_each(n, &|i: usize| j.run(i));
}

/// AVX2 + FMA usable (the kernel enabled the YMM state) and F16C present.
fn simd_avx2() -> bool {
    let f16c = unsafe { crate::arch::cpu::cpuid(1, 0).2 & (1 << 29) != 0 };
    crate::arch::cpudrv::avx2() && f16c
}

/// Weight blocks: RAM above 4 GiB first (none of it is used by anything else), then the heap in blocks just under 64 MiB.
fn block_alloc(min: usize) -> Option<(*mut u8, usize)> {
    if let Some(b) = crate::mm::highmem::alloc(min, 256 << 20) {
        return Some(b);
    }
    let size = min.max((64 << 20) - (64 << 10));
    let lay = core::alloc::Layout::from_size_align(size, 4096).ok()?;
    let p = unsafe { alloc::alloc::alloc(lay) };
    (!p.is_null()).then_some((p, size))
}

fn block_free(p: *mut u8, size: usize) {
    if crate::mm::highmem::owns(p) {
        crate::mm::highmem::free(p, size);
    } else if let Ok(lay) = core::alloc::Layout::from_size_align(size, 4096) {
        unsafe { alloc::alloc::dealloc(p, lay) };
    }
}

fn hooks() {
    ling_sd::kern::set_parallel(sd_parallel, crate::par::cores());
    ling_sd::kern::set_yield(maybe_yield);
    ling_sd::kern::set_avx2(simd_avx2());
    ling_sd::ops::set_block_alloc(block_alloc, block_free);
}

fn free_ram() -> u64 {
    crate::mm::frame::free_frame_count() as u64 * 4096
}

/// "8 cores, AVX2 + FMA" -- what the local engine runs on.
pub fn cpu_line() -> String {
    let simd = if simd_avx2() { "AVX2 + FMA" } else { "SSE" };
    alloc::format!("{} cores, {simd}", crate::par::cores())
}

fn mem_line() -> String {
    let hi = crate::mm::highmem::total();
    if hi > 0 {
        alloc::format!("{} MB free + {} MB above 4 GiB for weights", free_ram() >> 20, crate::mm::highmem::free_bytes() >> 20)
    } else {
        alloc::format!("{} MB free", free_ram() >> 20)
    }
}

// ── files ──────────────────────────────────────────────────────────────────────

struct Src(crate::fs::media::MediaRange);

impl ling_sd::Source for Src {
    fn len(&self) -> u64 {
        self.0.size()
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> bool {
        let mut done = 0;
        while done < buf.len() {
            let n = (buf.len() - done).min(1 << 20);
            if !self.0.read_at(off + done as u64, &mut buf[done..done + n]) {
                return false;
            }
            done += n;
            maybe_yield();
        }
        true
    }
}

/// Checkpoints on the mounted volumes: `<vol>/models/`, `<vol>/models/Stable-diffusion/`, `<vol>/Stable-diffusion/`, the
/// volume's root, and the user's home Models folder on the data disk; files over 1 GB ending in .safetensors.
pub fn find_models() -> Vec<String> {
    let mut out = Vec::new();
    let user = crate::vfs::home();
    for vol in crate::fs::media::names() {
        let v = alloc::format!("media/{vol}");
        for dir in [
            v.clone(),
            alloc::format!("{v}/models"),
            alloc::format!("{v}/models/Stable-diffusion"),
            alloc::format!("{v}/Stable-diffusion"),
            alloc::format!("{v}/{user}/Models"),
            alloc::format!("{v}/{user}/Programs/LingDiffusion/models"),
        ] {
            for (name, is_dir, size) in crate::fs::media::list(&dir) {
                if !is_dir && name.to_ascii_lowercase().ends_with(".safetensors") && size > 1_000_000_000 {
                    let p = alloc::format!("{dir}/{name}");
                    if !out.contains(&p) {
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

/// CLIP's vocab.json + merges.txt: next to the checkpoint, in `<vol>/models/clip/`, else the copy in the image.
fn tokenizer_files(ckpt: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let dir = crate::vfs::parent(ckpt);
    let mut cands: Vec<String> = alloc::vec![dir.clone(), alloc::format!("{dir}/clip")];
    for vol in crate::fs::media::names() {
        cands.push(alloc::format!("media/{vol}/models/clip"));
    }
    for d in cands {
        if let (Some(v), Some(m)) = (crate::fs::media::read(&alloc::format!("{d}/vocab.json")), crate::fs::media::read(&alloc::format!("{d}/merges.txt"))) {
            return Some((v, m));
        }
    }
    for base in ["apps/lingdiffusion/models/clip", "models/clip"] {
        if let (Some(v), Some(m)) = (crate::appfs::get(&alloc::format!("{base}/vocab.json")), crate::appfs::get(&alloc::format!("{base}/merges.txt"))) {
            return Some((v.to_vec(), m.to_vec()));
        }
    }
    None
}

/// Where pictures go: Photos/LingDiffusion in the user's home -- on the FAT data volume when there is one (lingfs holds only a
/// few MB), else in lingfs.
fn gallery_dir() -> String {
    let home = crate::vfs::home();
    match crate::vfs::package_volume() {
        Some(v) if v != "card" => alloc::format!("media/{v}/{home}/Photos/LingDiffusion"),
        _ => alloc::format!("{home}/Photos/LingDiffusion"),
    }
}

fn ensure_dir(d: &str) {
    if crate::fs::media::is_media_path(d) {
        crate::fs::media::mkdir_p(d);
    } else {
        crate::vfs::mkdir(d);
    }
}

pub fn gallery() -> Vec<String> {
    let d = gallery_dir();
    let mut v: Vec<String> = crate::vfs::list(&d).into_iter().filter(|(n, dir, _)| !dir && n.to_ascii_lowercase().ends_with(".png")).map(|(n, _, _)| alloc::format!("{d}/{n}")).collect();
    // names start with a running number: newest first
    v.sort();
    v.reverse();
    v
}

/// the running number for the next picture in the gallery folder `d`
fn next_number(d: &str) -> u32 {
    let mut next = 0u32;
    for (n, _, _) in crate::vfs::list(d) {
        if let Some(num) = n.split('-').next().and_then(|s| s.parse::<u32>().ok()) {
            next = next.max(num + 1);
        }
    }
    next
}

fn save_png(pic: &Pic) -> String {
    let d = gallery_dir();
    ensure_dir(&d);
    let next = next_number(&d);
    let path = alloc::format!("{d}/{next:05}-{}.png", pic.seed);
    if crate::vfs::write(&path, &pic.png) {
        log(&alloc::format!("diffusion: saved {path} ({} KB)", pic.png.len() >> 10));
        path
    } else {
        log(&alloc::format!("diffusion: could not write {path}"));
        String::new()
    }
}

// ── PNG ────────────────────────────────────────────────────────────────────────

fn crc32(data: &[&[u8]]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for part in data {
        for &b in part.iter() {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
    }
    !c
}

fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// RGB -> PNG with the WebUI's `parameters` text chunk.
fn encode_png(rgb: &[u8], w: usize, h: usize, params: &str) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for y in 0..h {
        // filter "sub" per row: smaller files than none, cheap
        raw.push(1);
        let row = &rgb[y * w * 3..(y + 1) * w * 3];
        for x in 0..w * 3 {
            let left = if x >= 3 { row[x - 3] } else { 0 };
            raw.push(row[x].wrapping_sub(left));
        }
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
    let mut out = alloc::vec![137u8, 80, 78, 71, 13, 10, 26, 10];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let mut t = b"parameters\0".to_vec();
    t.extend(params.bytes().map(|b| if b < 0x80 { b } else { b'?' }));
    chunk(&mut out, b"tEXt", &t);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The `parameters` text of a PNG (tEXt or iTXt), "" when there is none.
fn png_params(png: &[u8]) -> String {
    let mut pos = 8;
    while pos + 8 <= png.len() {
        let len = u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]]) as usize;
        let kind = &png[pos + 4..pos + 8];
        let Some(data) = png.get(pos + 8..pos + 8 + len) else { break };
        if kind == b"tEXt" && data.starts_with(b"parameters\0") {
            return String::from_utf8_lossy(&data[11..]).into_owned();
        }
        if kind == b"iTXt" && data.starts_with(b"parameters\0") {
            // keyword\0 flag method lang\0 tkey\0 text (uncompressed only)
            let rest = &data[11..];
            if rest.first() == Some(&0) {
                let mut z = 0;
                let mut i = 2;
                while i < rest.len() && z < 2 {
                    if rest[i] == 0 {
                        z += 1;
                    }
                    i += 1;
                }
                return String::from_utf8_lossy(&rest[i.min(rest.len())..]).into_owned();
            }
        }
        pos += 12 + len;
    }
    String::new()
}

fn rgb_to_px(rgb: &[u8]) -> Vec<u32> {
    rgb.chunks_exact(3).map(|c| (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32).collect()
}

// ── HTTP (plain, LAN) ──────────────────────────────────────────────────────────

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut it = s.split('.');
    for o in out.iter_mut() {
        *o = it.next()?.parse().ok()?;
    }
    it.next().is_none().then_some(out)
}

fn split_addr(addr: &str) -> (String, u16) {
    let a = addr.trim().trim_start_matches("http://").trim_end_matches('/');
    match a.rfind(':') {
        Some(i) => (a[..i].to_string(), a[i + 1..].parse().unwrap_or(7860)),
        None => (a.to_string(), 7860),
    }
}

/// One HTTP/1.0 request; (status, body). `wait_ms`: how long the server may take to answer (a txt2img runs minutes).
fn http(addr: &str, method: &str, path: &str, body: &[u8], connect_ms: u64, wait_ms: u64) -> Result<(u16, Vec<u8>), String> {
    let (host, port) = split_addr(addr);
    let ip = parse_ip(&host).or_else(|| ns::dns_resolve(&host)).ok_or_else(|| alloc::format!("{host}: unknown host"))?;
    // Connect and read the LWP way (pump once, look, yield): the waits never spin with the CPU held, so the app and the
    // desktop keep drawing while a server works for minutes (poll_until only yields a coroutine that holds the net lock).
    let h = ns::tcp_connect_begin_h(ip, port).ok_or_else(|| "no free network socket".to_string())?;
    let t0 = timer::now_ms();
    loop {
        ns::pump_once();
        if ns::tcp_established_h(h) {
            break;
        }
        if ns::tcp_closed_h(h) || timer::now_ms() - t0 > connect_ms {
            ns::tcp_free_h(h);
            return Err(alloc::format!("no answer from {host}:{port}"));
        }
        idle();
    }
    let mut req = alloc::format!("{method} {path} HTTP/1.0\r\nHost: {host}:{port}\r\nUser-Agent: LingOS-LingDiffusion\r\nAccept: application/json\r\nConnection: close\r\n").into_bytes();
    if !body.is_empty() || method == "POST" {
        req.extend_from_slice(alloc::format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()).as_bytes());
    }
    req.extend_from_slice(b"\r\n");
    req.extend_from_slice(body);
    for part in req.chunks(1200) {
        if !ns::tcp_write_h(h, part) {
            ns::tcp_close_h(h);
            return Err(alloc::format!("{host}:{port} stopped taking the request"));
        }
    }
    let mut resp: Vec<u8> = Vec::new();
    let mut buf = alloc::vec![0u8; 16384];
    let start = timer::now_ms();
    let mut last = start;
    let mut want: Option<usize> = None;
    loop {
        ns::pump_once();
        let n = ns::tcp_read_h(h, &mut buf, 0);
        if n > 0 {
            resp.extend_from_slice(&buf[..n]);
            last = timer::now_ms();
            if want.is_none() {
                if let Some(he) = find(&resp, b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&resp[..he]).to_ascii_lowercase();
                    if let Some(cl) = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()) {
                        want = Some(he + 4 + cl);
                    }
                }
            }
            if want.is_some_and(|w| resp.len() >= w) {
                break;
            }
            if resp.len() > 64 << 20 {
                break;
            }
            continue;
        }
        if ns::tcp_closed_h(h) {
            break;
        }
        let now = timer::now_ms();
        // waiting for the first byte may take the whole job; once data flows, a 20 s stall ends it
        if (resp.is_empty() && now - start > wait_ms) || (!resp.is_empty() && now - last > 20_000) {
            ns::tcp_free_h(h);
            return Err(alloc::format!("{host}:{port} took too long to answer"));
        }
        idle();
    }
    if ns::tcp_closed_h(h) {
        ns::tcp_free_h(h);
    } else {
        ns::tcp_close_h(h);
    }
    let he = find(&resp, b"\r\n\r\n").ok_or_else(|| alloc::format!("{host}:{port} sent no HTTP answer"))?;
    let status = core::str::from_utf8(&resp[..he]).ok().and_then(|s| s.split(' ').nth(1)).and_then(|c| c.parse().ok()).unwrap_or(0);
    let mut body = resp.split_off(he + 4);
    if let Some(w) = want {
        body.truncate(w - he - 4);
    }
    Ok((status, body))
}

/// Waiting on the network: give the CPU to the app / desktop (a coroutine), or just a breath (main).
fn idle() {
    if crate::coro::in_coro() {
        crate::coro::yield_now();
    } else {
        core::hint::spin_loop();
    }
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn json(b: &[u8]) -> Option<serde_json::Value> {
    serde_json::from_slice(b).ok()
}

/// The server in use: the configured one, else what the search found.
fn server() -> String {
    let s = st();
    if !s.server_cfg.trim().is_empty() {
        s.server_cfg.trim().to_string()
    } else {
        s.server_found.clone()
    }
}

/// Is `addr` a Stable Diffusion server? Some(description).
fn probe(addr: &str, connect_ms: u64) -> Option<(String, bool)> {
    let (st_code, body) = http(addr, "GET", "/sdapi/v1/options", b"", connect_ms, 4000).ok()?;
    if st_code != 200 {
        return None;
    }
    let o = json(&body)?;
    let model = o.get("sd_model_checkpoint").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut desc = alloc::format!("{addr}");
    // LingDiffusion says what it renders on
    let mut device = o.get("device").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut kind = "";
    if let Ok((200, eb)) = http(addr, "GET", "/api/engine", b"", connect_ms, 3000) {
        if let Some(e) = json(&eb) {
            kind = "LingDiffusion";
            if let Some(d) = e.get("device").and_then(|v| v.as_str()) {
                if !d.is_empty() {
                    device = d.to_string();
                }
            }
            if e.get("cuda").and_then(|v| v.as_bool()) == Some(true) && device.is_empty() {
                device = "CUDA".to_string();
            }
        }
    }
    // the WebUI reports its CUDA memory
    if device.is_empty() {
        if let Ok((200, mb)) = http(addr, "GET", "/sdapi/v1/memory", b"", connect_ms, 3000) {
            if let Some(m) = json(&mb) {
                if let Some(t) = m.pointer("/cuda/system/total").and_then(|v| v.as_f64()) {
                    device = alloc::format!("CUDA GPU, {:.0} GB", t / 1e9);
                }
                kind = "WebUI";
            }
        }
    }
    if !kind.is_empty() {
        desc.push_str(&alloc::format!(" ({kind})"));
    }
    if !device.is_empty() {
        desc.push_str(&alloc::format!(", {device}"));
    }
    if !model.is_empty() {
        let short = model.rsplit(['/', '\\']).next().unwrap_or(&model).to_string();
        desc.push_str(&alloc::format!(", {short}"));
    }
    Some((desc, kind == "LingDiffusion"))
}

fn net_task() {
    loop {
        let s = st();
        if s.want_probe {
            s.want_probe = false;
            let addr = server();
            if addr.is_empty() {
                s.server_state = "none";
            } else {
                s.server_state = "checking";
                match probe(&addr, 1500) {
                    Some((d, ld)) => {
                        let s = st();
                        s.server_state = "online";
                        s.server_info = d;
                        s.server_ld = ld;
                    },
                    None => {
                        let s = st();
                        s.server_state = "offline";
                        s.server_info = alloc::format!("{addr} does not answer");
                        // the configured one is down: look around once
                        if s.server_cfg.trim().is_empty() || !s.server_found.is_empty() {
                            s.server_found.clear();
                        }
                    },
                }
            }
            st().server_checked = timer::now_ms();
        }
        if st().want_search {
            st().want_search = false;
            search();
        }
        crate::coro::sleep_ms(200);
    }
}

/// Look for a server: the gateway first (QEMU's host is 10.0.2.2), then every address of its /24, ports 7860 and 7861.
fn search() {
    let s = st();
    s.searching = true;
    s.server_state = "searching";
    let gw = ns::gateway_ip();
    let me = ns::self_ip();
    let mut cands: Vec<[u8; 4]> = Vec::new();
    if gw != [0; 4] {
        cands.push(gw);
    }
    if me != [0; 4] {
        for i in 1..255u8 {
            let ip = [me[0], me[1], me[2], i];
            if ip != me && !cands.contains(&ip) {
                cands.push(ip);
            }
        }
    }
    let mut found = None;
    // the gateway on both ports first (a PC sharing its connection, QEMU's host), then the rest of the /24
    let mut order: Vec<([u8; 4], u16)> = Vec::new();
    let has_gw = gw != [0; 4];
    if has_gw {
        order.push((gw, 7860));
        order.push((gw, 7861));
    }
    for port in [7860u16, 7861] {
        for ip in cands.iter().skip(has_gw as usize) {
            order.push((*ip, port));
        }
    }
    for (ip, port) in order {
        let addr = alloc::format!("{}.{}.{}.{}:{port}", ip[0], ip[1], ip[2], ip[3]);
        st().server_info = alloc::format!("looking for a GPU server... {addr}");
        if let Some((d, ld)) = probe(&addr, 250) {
            st().server_ld = ld;
            found = Some((addr, d));
            break;
        }
        crate::coro::yield_now();
    }
    let s = st();
    s.searching = false;
    match found {
        Some((addr, d)) => {
            log(&alloc::format!("diffusion: found a server at {d}"));
            s.server_found = addr;
            s.server_state = "online";
            s.server_info = d;
        },
        None => {
            s.server_state = "none";
            s.server_info = "no GPU server answered on this network (ports 7860 / 7861)".to_string();
        },
    }
    s.server_checked = timer::now_ms();
}

fn ensure_net_task() {
    unsafe {
        if NET_TASK == 0 || !crate::coro::alive(NET_TASK) {
            NET_TASK = crate::coro::spawn_owned("diffusion-net", false, "system", Box::new(net_task));
        }
    }
}

// ── the job task ───────────────────────────────────────────────────────────────

fn fail(msg: String) {
    log(&alloc::format!("diffusion: {msg}"));
    let s = st();
    s.phase = Phase::Failed;
    s.msg = msg;
    s.t1 = timer::now_ms();
}

fn set_msg(m: String) {
    st().msg = m;
}

/// Load `path` unless it is what is loaded.
fn ensure_model(path: &str) -> Result<(), String> {
    let s = st();
    if s.loaded.is_some() && s.loaded_path == path {
        return Ok(());
    }
    s.loaded = None;
    s.loaded_path.clear();
    let mr = crate::fs::media::open_range(path).ok_or_else(|| alloc::format!("{path}: cannot open it"))?;
    let src = Src(mr);
    let (vocab, merges) = tokenizer_files(path).ok_or_else(|| "CLIP's vocab.json + merges.txt are missing (put them next to the checkpoint or in models/clip/)".to_string())?;
    // RAM check before reading 2 GB: the weights (f16) + room to work
    let header = ling_sd::st::SafeTensors::parse(&src)?;
    let need = ling_sd::model::check(&header)?;
    drop(header);
    let hi = crate::mm::highmem::free_bytes();
    let lo = free_ram();
    let work = 900u64 << 20;
    if hi + lo < need + work || lo < work {
        return Err(alloc::format!(
            "not enough memory for this checkpoint: its weights need {} MB and generating needs about {} MB more; {} MB are free below 4 GiB and {} MB above",
            need >> 20,
            work >> 20,
            lo >> 20,
            hi >> 20
        ));
    }
    let name = crate::vfs::basename(path);
    let s = st();
    s.phase = Phase::Loading;
    s.frac = 0.0;
    s.msg = alloc::format!("loading {name} ({} MB)", src.0.size() >> 20);
    log(&alloc::format!("diffusion: loading {path}; {}", mem_line()));
    let t0 = timer::now_ms();
    hooks();
    let m = ling_sd::Model::load(&src, &vocab, &merges, &mut |done, total| {
        let s = st();
        s.frac = done as f32 / total.max(1) as f32;
        s.msg = alloc::format!("loading {name}: {} of {} MB", done >> 20, total >> 20);
    })?;
    log(&alloc::format!("diffusion: {} MB of weights loaded in {} s; {}", m.weight_bytes >> 20, (timer::now_ms() - t0) / 1000, mem_line()));
    let s = st();
    s.loaded = Some(Box::new(m));
    s.loaded_path = path.to_string();
    Ok(())
}

fn run_local(r: &Req) -> Result<Pic, String> {
    let path = {
        let s = st();
        if !s.model_path.is_empty() {
            s.model_path.clone()
        } else {
            find_models().into_iter().next().ok_or_else(|| "no Stable Diffusion 1.5 checkpoint found: put a .safetensors file in models/ on a USB stick or the data disk".to_string())?
        }
    };
    ensure_model(&path)?;
    hooks();
    let s = st();
    s.phase = Phase::Working;
    s.step = 0;
    s.steps = r.steps;
    s.frac = 0.0;
    let p = ling_sd::Params {
        prompt: r.prompt.clone(),
        negative: r.negative.clone(),
        sampler: r.sampler.clone(),
        steps: r.steps,
        cfg: r.cfg,
        width: r.w,
        height: r.h,
        seed: r.seed,
        clip_skip: 1,
    };
    let t0 = timer::now_ms();
    let mut t_step = t0;
    let mut first_step = 0u64;
    let model = st().loaded.as_mut().ok_or("the model went away")?;
    let img = model.txt2img(&p, &mut |pr| {
        let s = st();
        match pr.stage {
            ling_sd::Stage::Text => s.msg = "reading the prompt (CLIP)".to_string(),
            ling_sd::Stage::Sampling => {
                let now = timer::now_ms();
                let dt = now - t_step;
                t_step = now;
                if pr.step == 1 {
                    first_step = dt;
                }
                log(&alloc::format!("diffusion: step {}/{} in {}.{:02} s", pr.step, pr.steps, dt / 1000, (dt % 1000) / 10));
                s.step = pr.step;
                s.steps = pr.steps;
                s.frac = pr.step as f32 / pr.steps.max(1) as f32 * 0.95;
                s.sps = dt as f32 / 1000.0;
                let left = (pr.steps - pr.step) as u64 * dt / 1000;
                s.msg = alloc::format!("step {} of {}  ({:.1} s a step, about {} s left)", pr.step, pr.steps, s.sps, left + 2);
                if let Some((rgb, w, h)) = pr.preview {
                    s.preview = rgb_to_px(rgb);
                    s.pw = w;
                    s.ph = h;
                    s.pic_id += 1;
                }
            },
            ling_sd::Stage::Decoding => {
                s.msg = "decoding the picture (VAE)".to_string();
                s.frac = 0.96;
            },
        }
        maybe_yield();
        true
    })?;
    let total = timer::now_ms() - t0;
    let _ = first_step;
    log(&alloc::format!("diffusion: {}x{} in {}.{} s on {}", img.w, img.h, total / 1000, (total % 1000) / 100, cpu_line()));
    let info = alloc::format!("{}, Time: {:.1} s on {}", img.info, total as f32 / 1000.0, cpu_line());
    let png = encode_png(&img.rgb, img.w, img.h, &info);
    Ok(Pic { px: rgb_to_px(&img.rgb), w: img.w, h: img.h, png, info, seed: img.seed, path: String::new(), by: alloc::format!("this PC's CPU ({})", cpu_line()) })
}

fn jstr(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn decode_b64_png(s: &str) -> Option<(Vec<u8>, crate::drivers::image::Image)> {
    use base64::Engine;
    let s = s.split(',').next_back().unwrap_or(s).trim();
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).ok()?;
    let img = crate::drivers::image::decode_png(&bytes)?;
    Some((bytes, img))
}

/// While a server job runs: its progress (step, ETA, preview) every ~1.5 s.
fn progress_poller(addr: String) {
    while st().remote_busy {
        crate::coro::sleep_ms(1500);
        if !st().remote_busy {
            break;
        }
        let Ok((200, b)) = http(&addr, "GET", "/sdapi/v1/progress?skip_current_image=false", b"", 1500, 5000) else { continue };
        let Some(j) = json(&b) else { continue };
        let s = st();
        if !s.remote_busy {
            break;
        }
        let frac = j.get("progress").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let eta = j.get("eta_relative").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let step = j.pointer("/state/sampling_step").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let steps = j.pointer("/state/sampling_steps").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        if steps > 0 {
            s.step = step;
            s.steps = steps;
        }
        s.frac = frac.clamp(0.0, 0.97);
        s.msg = if steps > 0 { alloc::format!("GPU server: step {step} of {steps}, about {:.0} s left", eta.max(0.0)) } else { alloc::format!("GPU server: {:.0}%", frac * 100.0) };
        if let Some(ci) = j.get("current_image").and_then(|v| v.as_str()) {
            if let Some((_, img)) = decode_b64_png(ci) {
                let s = st();
                s.preview = img.px;
                s.pw = img.w as usize;
                s.ph = img.h as usize;
                s.pic_id += 1;
            }
        }
    }
}

/// LingDiffusion's queued API: POST /api/generate -> {job, outputs_root}; GET /api/status?job= (state, step, steps, eta,
/// images = server paths, infotexts, seeds); GET /api/preview?job= (live preview); the PNG from /outputs/<path under root>.
fn run_remote_queued(r: &Req, addr: &str, body: &str) -> Result<Pic, String> {
    let t0 = timer::now_ms();
    // the queued API hands the picture back as a file under the server's outputs (it keeps a copy, as the WebUI does)
    let body = body.replace("\"save_images\":false", "\"save_images\":true");
    let (code, resp) = http(addr, "POST", "/api/generate?mode=txt2img", body.as_bytes(), 3000, 20_000)?;
    let j = json(&resp).ok_or("no queued API")?;
    let Some(job) = j.get("job").and_then(|v| v.as_str()).map(|s| s.to_string()) else {
        if code == 404 {
            return Err("no queued API".to_string());
        }
        let why = j.get("detail").or_else(|| j.get("error")).and_then(|v| v.as_str()).unwrap_or("error");
        return Err(alloc::format!("the GPU server said: {why} (HTTP {code})"));
    };
    let root = j.get("outputs_root").and_then(|v| v.as_str()).unwrap_or("").replace('\\', "/");
    st().remote_job = job.clone();
    let mut last_preview = 0u64;
    let status = loop {
        crate::coro::sleep_ms(700);
        if st().phase != Phase::Working {
            let _ = http(addr, "POST", &alloc::format!("/api/interrupt?job={job}"), b"{}", 1500, 3000);
            st().remote_job.clear();
            return Err("stopped".to_string());
        }
        let Ok((200, b)) = http(addr, "GET", &alloc::format!("/api/status?job={job}"), b"", 2000, 5000) else { continue };
        let Some(sj) = json(&b) else { continue };
        let state = sj.get("state").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let step = sj.get("step").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let steps = sj.get("steps").and_then(|v| v.as_u64()).unwrap_or(r.steps as u64) as usize;
        let eta = sj.get("eta").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let frac = sj.get("progress").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let s = st();
        s.step = step;
        s.steps = steps.max(1);
        s.frac = frac.clamp(0.0, 0.97);
        s.msg = match state.as_str() {
            "queued" => "GPU server: waiting in its queue".to_string(),
            "loading" => "GPU server: loading the checkpoint".to_string(),
            "decoding" => "GPU server: decoding the picture".to_string(),
            _ => alloc::format!("GPU server: step {step} of {steps}, about {:.0} s left", eta.max(0.0)),
        };
        if matches!(state.as_str(), "done" | "error" | "interrupted" | "unknown") {
            break sj;
        }
        // the live preview every ~1.5 s
        if state == "running" && timer::now_ms() - last_preview > 1500 {
            last_preview = timer::now_ms();
            if let Ok((200, pb)) = http(addr, "GET", &alloc::format!("/api/preview?job={job}"), b"", 2000, 5000) {
                if let Some(img) = json(&pb).and_then(|p| p.get("image").and_then(|v| v.as_str()).map(|s| s.to_string())) {
                    if let Some((_, im)) = decode_b64_png(&img) {
                        let s = st();
                        s.preview = im.px;
                        s.pw = im.w as usize;
                        s.ph = im.h as usize;
                        s.pic_id += 1;
                    }
                }
            }
        }
    };
    st().remote_job.clear();
    let state = status.get("state").and_then(|v| v.as_str()).unwrap_or("");
    if state != "done" {
        let why = status.get("error").and_then(|v| v.as_str()).filter(|e| !e.is_empty()).unwrap_or(state);
        return Err(alloc::format!("the GPU server's job ended: {why}"));
    }
    let path = status.pointer("/images/0").and_then(|v| v.as_str()).unwrap_or("").replace('\\', "/");
    let rel = path.strip_prefix(&root).unwrap_or(&path).trim_start_matches('/').to_string();
    let url = alloc::format!("/outputs/{}", rel.replace(' ', "%20"));
    let (code, png) = http(addr, "GET", &url, b"", 3000, 20_000)?;
    if code != 200 {
        return Err(alloc::format!("could not fetch the picture from the GPU server ({url}: HTTP {code})"));
    }
    let img = crate::drivers::image::decode_png(&png).ok_or("the server's picture is not a PNG this kernel can read")?;
    let mut info = png_params(&png);
    if info.is_empty() {
        info = status.pointer("/infotexts/0").and_then(|v| v.as_str()).unwrap_or("").to_string();
    }
    let seed = status.pointer("/seeds/0").and_then(|v| v.as_i64()).filter(|v| *v >= 0).map(|v| v as u64).unwrap_or(r.seed);
    let secs = (timer::now_ms() - t0) as f32 / 1000.0;
    log(&alloc::format!("diffusion: {}x{} from {addr} (queued job {job}) in {secs:.1} s", img.w, img.h));
    Ok(Pic { px: img.px, w: img.w as usize, h: img.h as usize, png, info, seed, path: String::new(), by: alloc::format!("GPU server {}", st().server_info) })
}

fn run_remote(r: &Req, addr: &str) -> Result<Pic, String> {
    let s = st();
    s.phase = Phase::Working;
    s.step = 0;
    s.steps = r.steps;
    s.frac = 0.0;
    s.msg = alloc::format!("sending to the GPU server at {addr}");
    let body = alloc::format!(
        "{{\"prompt\":{},\"negative_prompt\":{},\"sampler_name\":{},\"steps\":{},\"cfg_scale\":{},\"width\":{},\"height\":{},\"seed\":{},\"batch_size\":1,\"n_iter\":1,\"send_images\":true,\"save_images\":false}}",
        jstr(&r.prompt),
        jstr(&r.negative),
        jstr(&r.sampler),
        r.steps,
        r.cfg,
        r.w,
        r.h,
        r.seed
    );
    // LingDiffusion answers /sdapi/v1/txt2img only when the picture is done (and serves nothing else meanwhile): its own
    // queued API gives the step, the ETA and a live preview while the GPU works.
    if s.server_ld {
        match run_remote_queued(r, addr, &body) {
            Err(e) if e == "no queued API" => {},
            other => return other,
        }
    }
    let s = st();
    s.remote_busy = true;
    let a2 = addr.to_string();
    crate::coro::spawn_owned("diffusion-progress", false, "system", Box::new(move || progress_poller(a2)));
    let t0 = timer::now_ms();
    let res = http(addr, "POST", "/sdapi/v1/txt2img", body.as_bytes(), 3000, 30 * 60 * 1000);
    st().remote_busy = false;
    let (code, resp) = res?;
    if st().phase != Phase::Working {
        return Err("stopped".to_string());
    }
    let j = json(&resp).ok_or_else(|| alloc::format!("the server's answer is not JSON (HTTP {code})"))?;
    if code != 200 {
        let why = j.get("detail").or_else(|| j.get("error")).and_then(|v| v.as_str()).unwrap_or("error");
        return Err(alloc::format!("the GPU server said: {why} (HTTP {code})"));
    }
    let first = j.pointer("/images/0").and_then(|v| v.as_str()).ok_or("the server sent no image")?;
    let (png, img) = decode_b64_png(first).ok_or("the server's image is not a PNG this kernel can read")?;
    let mut info = png_params(&png);
    let mut seed = r.seed;
    if let Some(inf) = j.get("info").and_then(|v| v.as_str()).and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()) {
        if let Some(sd) = inf.get("seed").and_then(|v| v.as_u64()) {
            seed = sd;
        }
        if info.is_empty() {
            if let Some(t) = inf.pointer("/infotexts/0").and_then(|v| v.as_str()) {
                info = t.to_string();
            }
        }
    }
    let secs = (timer::now_ms() - t0) as f32 / 1000.0;
    log(&alloc::format!("diffusion: {}x{} from {addr} in {secs:.1} s", img.w, img.h));
    let png = if png_params(&png).is_empty() { encode_png_from_px(&img.px, img.w as usize, img.h as usize, &info) } else { png };
    Ok(Pic { px: img.px, w: img.w as usize, h: img.h as usize, png, info, seed, path: String::new(), by: alloc::format!("GPU server {}", st().server_info) })
}

fn encode_png_from_px(px: &[u32], w: usize, h: usize, info: &str) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(px.len() * 3);
    for &p in px {
        rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    encode_png(&rgb, w, h, info)
}

fn run(r: Req) {
    let s = st();
    s.t0 = timer::now_ms();
    s.preview.clear();
    s.pw = 0;
    s.ph = 0;
    ling_sd::kern::set_cancel(false);
    let addr = server();
    let backend = r.backend.as_str();
    // the server, when chosen or (auto) when it answers
    let try_remote = match backend {
        "local" => false,
        "remote" => true,
        _ => !addr.is_empty() && s.server_state == "online",
    };
    let mut out: Result<Pic, String> = Err(String::new());
    if try_remote {
        if addr.is_empty() {
            return fail("no GPU server set: enter its address in Settings (e.g. 192.168.1.20:7860), or pick the CPU".to_string());
        }
        out = run_remote(&r, &addr);
        if let Err(e) = &out {
            if e == "stopped" {
                return fail("stopped".to_string());
            }
            st().server_state = "offline";
            if backend == "remote" {
                return fail(e.clone());
            }
            log(&alloc::format!("diffusion: server failed ({e}); falling back to the CPU"));
            set_msg(alloc::format!("{e} -- using this PC's CPU instead"));
        }
    }
    if out.is_err() {
        out = run_local(&r);
    }
    match out {
        Ok(mut pic) => {
            pic.path = save_png(&pic);
            let s = st();
            s.pic_id += 1;
            s.result = Some(pic);
            s.phase = Phase::Done;
            s.frac = 1.0;
            s.t1 = timer::now_ms();
            let secs = (s.t1 - s.t0) as f32 / 1000.0;
            s.msg = alloc::format!("done in {secs:.1} s");
            s.thumbs.clear();
        },
        Err(e) => fail(e),
    }
}

fn task() {
    loop {
        if let Some(r) = st().pending.take() {
            run(r);
        }
        crate::coro::sleep_ms(50);
    }
}

fn ensure_task() -> bool {
    unsafe {
        if TASK != 0 && crate::coro::alive(TASK) {
            return true;
        }
        TASK = crate::coro::spawn_owned("diffusion", false, "system", Box::new(task));
        TASK != 0
    }
}

fn random_seed() -> u64 {
    let r = unsafe { crate::arch::cpu::rdrand64() }.unwrap_or(0) ^ timer::now_us().wrapping_mul(0x9E37_79B9_7F4A_7C15);
    r % 4_294_967_295
}

// ── drawing ────────────────────────────────────────────────────────────────────

/// Bilinear resample of `src` (sw x sh) to dw x dh.
fn resample(src: &[u32], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u32> {
    let mut out = alloc::vec![0u32; dw * dh];
    if sw == 0 || sh == 0 {
        return out;
    }
    // shrinking a lot: average boxes; otherwise bilinear
    let down = sw >= dw * 2;
    for y in 0..dh {
        for x in 0..dw {
            out[y * dw + x] = if down {
                let (x0, x1) = (x * sw / dw, ((x + 1) * sw / dw).max(x * sw / dw + 1).min(sw));
                let (y0, y1) = (y * sh / dh, ((y + 1) * sh / dh).max(y * sh / dh + 1).min(sh));
                let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        let p = src[yy * sw + xx];
                        r += (p >> 16) & 255;
                        g += (p >> 8) & 255;
                        b += p & 255;
                        n += 1;
                    }
                }
                ((r / n) << 16) | ((g / n) << 8) | (b / n)
            } else {
                let fx = ((x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5).max(0.0);
                let fy = ((y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5).max(0.0);
                let (ix, iy) = ((fx as usize).min(sw - 1), (fy as usize).min(sh - 1));
                let (jx, jy) = ((ix + 1).min(sw - 1), (iy + 1).min(sh - 1));
                let (ax, ay) = (fx - ix as f32, fy - iy as f32);
                let mut c = 0u32;
                for sh_ in [16u32, 8, 0] {
                    let g = |p: u32| ((p >> sh_) & 255) as f32;
                    let top = g(src[iy * sw + ix]) * (1.0 - ax) + g(src[iy * sw + jx]) * ax;
                    let bot = g(src[jy * sw + ix]) * (1.0 - ax) + g(src[jy * sw + jx]) * ax;
                    c |= ((top * (1.0 - ay) + bot * ay + 0.5) as u32).min(255) << sh_;
                }
                c
            };
        }
    }
    out
}

/// Blit `px` (w x h) at (x, y) into the app window.
fn blit(px: &[u32], w: usize, h: usize, x: i64, y: i64) {
    let Some((buf, bw, bh)) = crate::lingapp::surface() else { return };
    for row in 0..h {
        let yy = y + row as i64;
        if yy < 0 || yy >= bh as i64 {
            continue;
        }
        for col in 0..w {
            let xx = x + col as i64;
            if xx < 0 || xx >= bw as i64 {
                continue;
            }
            buf[yy as usize * bw + xx as usize] = px[row * w + col];
        }
    }
}

/// Fit (aspect kept, centred) into the box; the scaled copy is kept for the next frame.
fn draw_fitted(id: u64, px: &[u32], w: usize, h: usize, bx: f64, by: f64, bw: f64, bh: f64, cache: bool) -> bool {
    if w == 0 || h == 0 || bw < 2.0 || bh < 2.0 {
        return false;
    }
    let sc = f64::min(bw / w as f64, bh / h as f64);
    let (dw, dh) = (((w as f64 * sc) as usize).max(1), ((h as f64 * sc) as usize).max(1));
    let (ox, oy) = ((bx + (bw - dw as f64) / 2.0) as i64, (by + (bh - dh as f64) / 2.0) as i64);
    if cache {
        let s = st();
        if s.scaled.0 != id || s.scaled.1 != dw || s.scaled.2 != dh {
            s.scaled = (id, dw, dh, resample(px, w, h, dw, dh));
        }
        let sc = &st().scaled.3;
        blit(sc, dw, dh, ox, oy);
    } else {
        let v = resample(px, w, h, dw, dh);
        blit(&v, dw, dh, ox, oy);
    }
    true
}

fn draw_current(x: f64, y: f64, w: f64, h: f64) -> bool {
    let s = st();
    let id = s.pic_id;
    let working = matches!(s.phase, Phase::Working | Phase::Loading);
    if working && s.pw > 0 && !s.preview.is_empty() {
        let (pw, ph) = (s.pw, s.ph);
        let pv = core::mem::take(&mut s.preview);
        let ok = draw_fitted(id, &pv, pw, ph, x, y, w, h, true);
        st().preview = pv;
        return ok;
    }
    if let Some(p) = s.result.take() {
        let ok = draw_fitted(id, &p.px, p.w, p.h, x, y, w, h, true);
        st().result = Some(p);
        return ok;
    }
    false
}

/// A gallery thumbnail, decoded once.
fn draw_thumb(path: &str, x: f64, y: f64, w: f64, h: f64) -> bool {
    let s = st();
    let side = (w.max(h) as usize).max(8);
    if !s.thumbs.iter().any(|t| t.0 == path) {
        let Some(bytes) = crate::vfs::read(path) else { return false };
        let Some(img) = crate::drivers::image::decode_png(&bytes) else { return false };
        let (iw, ih) = (img.w as usize, img.h as usize);
        let sc = side as f64 / iw.max(ih) as f64;
        let (tw, th) = (((iw as f64 * sc) as usize).max(1), ((ih as f64 * sc) as usize).max(1));
        let t = resample(&img.px, iw, ih, tw, th);
        let s = st();
        if s.thumbs.len() > 48 {
            s.thumbs.remove(0);
        }
        s.thumbs.push((path.to_string(), tw, th, t));
    }
    let s = st();
    let Some(t) = s.thumbs.iter().find(|t| t.0 == path) else { return false };
    let (tw, th) = (t.1, t.2);
    let px = t.3.clone();
    draw_fitted(0, &px, tw, th, x, y, w, h, false)
}

/// Show a saved picture as the result (from the gallery).
fn open_pic(path: &str) -> bool {
    let Some(bytes) = crate::vfs::read(path) else { return false };
    let Some(img) = crate::drivers::image::decode_png(&bytes) else { return false };
    let info = png_params(&bytes);
    let seed = info.split("Seed: ").nth(1).and_then(|r| r.split(',').next()).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let s = st();
    s.pic_id += 1;
    s.result = Some(Pic { px: img.px, w: img.w as usize, h: img.h as usize, png: bytes, info, seed, path: path.to_string(), by: "saved picture".to_string() });
    if !matches!(s.phase, Phase::Working | Phase::Loading) {
        s.phase = Phase::Done;
        s.msg = crate::vfs::basename(path);
    }
    true
}

// ── builtins ───────────────────────────────────────────────────────────────────

/// The `diffusion_*` builtins; None for any other name.
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    if !name.starts_with(b"diffusion_") {
        return None;
    }
    let sarg = |i: usize| args.get(i).map(|&v| to_string(v)).unwrap_or_default();
    let narg = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).filter(|v| v.is_finite()).unwrap_or(d);
    Some(match name {
        b"diffusion_generate" => {
            if !ensure_task() {
                return Some(boolean(false));
            }
            ensure_net_task();
            let s = st();
            if matches!(s.phase, Phase::Working | Phase::Loading) || s.pending.is_some() {
                return Some(boolean(false));
            }
            let seed = narg(8, -1.0);
            let seed = if seed < 0.0 { random_seed() } else { seed as u64 };
            let snap = |v: f64| (libm::round(v.clamp(64.0, 1024.0) / 64.0) * 64.0) as usize;
            s.pending = Some(Req {
                backend: sarg(0),
                prompt: sarg(1),
                negative: sarg(2),
                sampler: { let v = sarg(3); if v.is_empty() { "Euler a".to_string() } else { v } },
                steps: narg(4, 20.0).clamp(1.0, 150.0) as usize,
                cfg: narg(5, 7.0).clamp(1.0, 30.0) as f32,
                w: snap(narg(6, 512.0)),
                h: snap(narg(7, 512.0)),
                seed,
            });
            s.phase = Phase::Working;
            s.msg = "starting".to_string();
            s.frac = 0.0;
            s.step = 0;
            boolean(true)
        },
        b"diffusion_cancel" => {
            let s = st();
            if matches!(s.phase, Phase::Working | Phase::Loading) {
                ling_sd::kern::set_cancel(true);
                if s.remote_busy {
                    let addr = server();
                    s.remote_busy = false;
                    // the server stops its job; our request then returns
                    let _ = http(&addr, "POST", "/sdapi/v1/interrupt", b"{}", 1500, 3000);
                }
                s.phase = Phase::Failed;
                s.msg = "stopping...".to_string();
            }
            UNIT
        },
        b"diffusion_state" => mk_str(match st().phase {
            Phase::Idle => "idle",
            Phase::Loading => "loading",
            Phase::Working => "working",
            Phase::Done => "done",
            Phase::Failed => "error",
        }),
        b"diffusion_message" => mk_str(&st().msg),
        b"diffusion_progress" => num(st().frac as f64),
        b"diffusion_step" => num(st().step as f64),
        b"diffusion_steps" => num(st().steps as f64),
        b"diffusion_elapsed" => {
            let s = st();
            let end = if matches!(s.phase, Phase::Working | Phase::Loading) { timer::now_ms() } else { s.t1 };
            num(end.saturating_sub(s.t0) as f64 / 1000.0)
        },
        b"diffusion_draw" => boolean(draw_current(narg(0, 0.0), narg(1, 0.0), narg(2, 512.0), narg(3, 512.0))),
        b"diffusion_has_result" => boolean(st().result.is_some()),
        b"diffusion_result_info" => mk_str(st().result.as_ref().map(|p| p.info.as_str()).unwrap_or("")),
        b"diffusion_result_seed" => num(st().result.as_ref().map(|p| p.seed as f64).unwrap_or(-1.0)),
        b"diffusion_result_path" => mk_str(st().result.as_ref().map(|p| p.path.as_str()).unwrap_or("")),
        b"diffusion_result_by" => mk_str(st().result.as_ref().map(|p| p.by.as_str()).unwrap_or("")),
        b"diffusion_result_size" => mk_str(&st().result.as_ref().map(|p| alloc::format!("{}x{}", p.w, p.h)).unwrap_or_default()),
        b"diffusion_save_as" => {
            let to = sarg(0);
            let ok = match st().result.as_ref() {
                Some(p) if !to.is_empty() => {
                    let to = if to.to_ascii_lowercase().ends_with(".png") { to } else { alloc::format!("{to}.png") };
                    let d = crate::vfs::parent(&to);
                    if !d.is_empty() {
                        ensure_dir(&d);
                    }
                    crate::vfs::write(&to, &p.png)
                },
                _ => false,
            };
            boolean(ok)
        },
        b"diffusion_gallery" => list_from(gallery().iter().map(|p| mk_str(p)).collect()),
        b"diffusion_gallery_dir" => mk_str(&gallery_dir()),
        b"diffusion_thumb" => boolean(draw_thumb(&sarg(0), narg(1, 0.0), narg(2, 0.0), narg(3, 96.0), narg(4, 96.0))),
        b"diffusion_open" => boolean(open_pic(&sarg(0))),
        b"diffusion_delete" => {
            let p = sarg(0);
            st().thumbs.retain(|t| t.0 != p);
            boolean(crate::vfs::delete(&p))
        },
        // the picture clipboard (drivers/clipboard.rs): the result onto it, or a clipboard picture into the gallery
        b"diffusion_copy_result" => {
            let ok = match st().result.as_ref() {
                Some(p) if !p.path.is_empty() && crate::vfs::exists(&p.path) => crate::drivers::clipboard::image_copy_file(&p.path, false),
                Some(p) => {
                    let mut rgba = Vec::with_capacity(p.px.len() * 4);
                    for &c in &p.px {
                        rgba.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]);
                    }
                    crate::drivers::clipboard::image_set_rgba(p.w, p.h, rgba, &alloc::format!("lingdiffusion-{}.png", p.seed), p.png.clone(), false)
                },
                None => false,
            };
            boolean(ok)
        },
        b"diffusion_paste" => {
            let d = gallery_dir();
            ensure_dir(&d);
            let next = next_number(&d);
            let out = crate::drivers::clipboard::image_paste_to(&alloc::format!("{d}/{next:05}-pasted.png")).unwrap_or_default();
            mk_str(&out)
        },
        // the GPU server
        b"diffusion_set_server" => {
            let s = st();
            let v = sarg(0).trim().to_string();
            if v != s.server_cfg {
                s.server_cfg = v;
                s.server_state = "unknown";
                s.server_info.clear();
            }
            ensure_net_task();
            st().want_probe = true;
            UNIT
        },
        b"diffusion_server" => mk_str(&server()),
        b"diffusion_server_state" => {
            let s = st();
            // re-check now and then (the server may have started or stopped)
            if !s.searching && timer::now_ms().saturating_sub(s.server_checked) > 30_000 && (s.server_state == "online" || s.server_state == "offline") {
                s.server_checked = timer::now_ms();
                s.want_probe = true;
                ensure_net_task();
            }
            mk_str(st().server_state)
        },
        b"diffusion_server_info" => mk_str(&st().server_info),
        b"diffusion_probe" => {
            ensure_net_task();
            st().want_probe = true;
            UNIT
        },
        b"diffusion_discover" => {
            ensure_net_task();
            let s = st();
            if !s.searching {
                s.want_search = true;
                s.server_state = "searching";
            }
            UNIT
        },
        // the local engine
        b"diffusion_models" => list_from(find_models().iter().map(|p| mk_str(p)).collect()),
        b"diffusion_set_model" => {
            st().model_path = sarg(0);
            UNIT
        },
        b"diffusion_model" => mk_str(&st().model_path),
        b"diffusion_model_loaded" => mk_str(&st().loaded_path),
        b"diffusion_unload" => {
            let s = st();
            if !matches!(s.phase, Phase::Working | Phase::Loading) {
                s.loaded = None;
                s.loaded_path.clear();
            }
            UNIT
        },
        b"diffusion_cpu" => mk_str(&cpu_line()),
        b"diffusion_memory" => mk_str(&mem_line()),
        b"diffusion_last_step_secs" => num(st().sps as f64),
        _ => return None,
    })
}
