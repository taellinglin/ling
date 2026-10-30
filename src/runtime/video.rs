//! `video_*` builtins: MP4 (H.264 + AAC) playback for Horizon's `<video>` element.
//!
//! `video_open(url)` returns an id immediately; a background thread fetches the whole file (http/https via reqwest
//! under the `web` feature, or a local path / `file:` URL), demuxes it with the `mp4` crate and decodes H.264 with
//! OpenH264 into a small queue of RGBA frames (downscaled to <= 1280 wide). A second thread decodes the AAC track
//! with symphonia and plays it through cpal, kept in step with the player's wall clock. The Ling thread only copies the
//! current frame into the interpreter's image table in `video_frame` (one handle per video, updated in place).
//!
//! State lives in a process-wide registry (no interpreter field), so the interpreter struct is untouched.
use super::{EvalResult, Interpreter, Value};

impl Interpreter {
    pub(super) fn video_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        let rest = name.strip_prefix("video_")?;
        #[cfg(feature = "video")]
        {
            imp::dispatch(self, rest, args)
        }
        #[cfg(not(feature = "video"))]
        {
            let _ = args;
            // Built without the `video` feature: every video fails to open.
            Some(Ok(match rest {
                "open" | "state" | "frame" => Value::Number(-1.0),
                "error" => Value::Str("video support not compiled in (build with --features video)".into()),
                "width" | "height" | "duration" | "time" | "playing" => Value::Number(0.0),
                "play" | "pause" | "seek" | "volume" | "close" => Value::Unit,
                _ => return None,
            }))
        }
    }
}

#[cfg(feature = "video")]
mod imp {
    use super::super::{EvalResult, Interpreter, Value};
    use std::collections::{HashMap, VecDeque};
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    const MAX_BYTES: usize = 300 * 1024 * 1024;
    const MAX_W: usize = 1280;
    const QUEUE: usize = 6;

    struct Frame {
        pts: f64,
        w: u32,
        h: u32,
        rgba: Vec<u8>,
        seq: u64,
    }

    struct St {
        state: i32,
        error: String,
        width: f64,
        height: f64,
        duration: f64,
        playing: bool,
        base_pos: f64,
        base_at: Instant,
        volume: f32,
        seek_gen: u64,
        seek_target: f64,
        frames: VecDeque<Frame>,
        decode_done: bool,
        next_seq: u64,
    }

    impl St {
        fn clock(&self) -> f64 {
            let t = if self.playing { self.base_pos + self.base_at.elapsed().as_secs_f64() } else { self.base_pos };
            if self.duration > 0.0 { t.min(self.duration) } else { t }
        }
        /// Ends playback once the clock passes the end and the decoder has nothing left.
        fn tick(&mut self) {
            if self.playing && self.decode_done && self.frames.len() <= 1 && self.duration > 0.0 && self.clock() >= self.duration - 1e-3 {
                self.base_pos = self.duration;
                self.playing = false;
                self.state = 2;
            }
        }
        fn fail(&mut self, msg: String) {
            self.state = -1;
            self.error = msg;
            self.playing = false;
        }
    }

    struct Shared {
        st: Mutex<St>,
        stop: AtomicBool,
    }

    struct Player {
        sh: Arc<Shared>,
        image: Option<usize>,
        shown_seq: u64,
    }

    fn registry() -> &'static Mutex<(u32, HashMap<u32, Player>)> {
        static R: OnceLock<Mutex<(u32, HashMap<u32, Player>)>> = OnceLock::new();
        R.get_or_init(|| Mutex::new((0, HashMap::new())))
    }

    pub(super) fn dispatch(it: &mut Interpreter, rest: &str, args: &[Value]) -> Option<EvalResult> {
        let num = |it: &Interpreter, n: usize, d: f64| it.arg_num(args, n, d);
        if rest == "open" {
            let url = it.arg_str(args, 0, "");
            let base = it.source_dir.clone();
            return Some(Ok(Value::Number(open(url, base) as f64)));
        }
        let id = match num(it, 0, -1.0) {
            Ok(v) => v as i64,
            Err(e) => return Some(Err(e)),
        };
        let mut reg = registry().lock().unwrap();
        if rest == "close" {
            if let Some(p) = (id >= 0).then(|| reg.1.remove(&(id as u32))).flatten() {
                p.sh.stop.store(true, Ordering::Relaxed);
                p.sh.st.lock().unwrap().frames.clear();
                if let Some(h) = p.image {
                    if h < it.images.len() {
                        it.images[h] = image::RgbaImage::new(1, 1);
                    }
                }
            }
            return Some(Ok(Value::Unit));
        }
        let Some(p) = (id >= 0).then(|| reg.1.get_mut(&(id as u32))).flatten() else {
            return Some(Ok(match rest {
                "state" | "frame" => Value::Number(-1.0),
                "error" => Value::Str("no such video".into()),
                "width" | "height" | "duration" | "time" | "playing" => Value::Number(0.0),
                "play" | "pause" | "seek" | "volume" | "close" => Value::Unit,
                _ => return None,
            }));
        };
        let v = match rest {
            "state" => {
                let mut st = p.sh.st.lock().unwrap();
                st.tick();
                Value::Number(st.state as f64)
            },
            "error" => Value::Str(p.sh.st.lock().unwrap().error.clone()),
            "width" => Value::Number(p.sh.st.lock().unwrap().width),
            "height" => Value::Number(p.sh.st.lock().unwrap().height),
            "duration" => Value::Number(p.sh.st.lock().unwrap().duration),
            "time" => {
                let mut st = p.sh.st.lock().unwrap();
                st.tick();
                Value::Number(st.clock())
            },
            "playing" => {
                let mut st = p.sh.st.lock().unwrap();
                st.tick();
                Value::Number(if st.playing { 1.0 } else { 0.0 })
            },
            "play" => {
                let mut st = p.sh.st.lock().unwrap();
                st.tick();
                if st.state == 2 {
                    seek(&mut st, 0.0);
                }
                if !st.playing && st.state >= 0 {
                    st.playing = true;
                    st.base_at = Instant::now();
                }
                Value::Unit
            },
            "pause" => {
                let mut st = p.sh.st.lock().unwrap();
                if st.playing {
                    st.base_pos = st.clock();
                    st.playing = false;
                }
                Value::Unit
            },
            "seek" => {
                let t = match num(it, 1, 0.0) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                let mut st = p.sh.st.lock().unwrap();
                if st.state >= 0 {
                    let t = if st.duration > 0.0 { t.clamp(0.0, st.duration) } else { t.max(0.0) };
                    seek(&mut st, t);
                }
                Value::Unit
            },
            "volume" => {
                let v = match num(it, 1, 1.0) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                p.sh.st.lock().unwrap().volume = v.clamp(0.0, 1.0) as f32;
                Value::Unit
            },
            "frame" => {
                let mut st = p.sh.st.lock().unwrap();
                st.tick();
                let c = st.clock();
                while st.frames.len() >= 2 && st.frames[1].pts <= c + 0.004 {
                    st.frames.pop_front();
                }
                if let Some(f) = st.frames.front() {
                    if p.image.is_none() || f.seq != p.shown_seq {
                        let h = match p.image {
                            Some(h) if h < it.images.len() => h,
                            _ => {
                                it.images.push(image::RgbaImage::new(f.w, f.h));
                                it.images.len() - 1
                            },
                        };
                        let img = &mut it.images[h];
                        if img.width() != f.w || img.height() != f.h {
                            *img = image::RgbaImage::new(f.w, f.h);
                        }
                        let dst: &mut [u8] = img.as_mut();
                        dst.copy_from_slice(&f.rgba);
                        p.image = Some(h);
                        p.shown_seq = f.seq;
                    }
                }
                Value::Number(match p.image {
                    Some(h) => h as f64,
                    None => -1.0,
                })
            },
            _ => return None,
        };
        Some(Ok(v))
    }

    fn seek(st: &mut St, t: f64) {
        st.seek_gen += 1;
        st.seek_target = t;
        st.base_pos = t;
        st.base_at = Instant::now();
        st.frames.clear();
        st.decode_done = false;
        if st.state == 2 {
            st.state = 1;
        }
    }

    fn open(url: String, base: Option<std::path::PathBuf>) -> u32 {
        let sh = Arc::new(Shared {
            st: Mutex::new(St {
                state: 0,
                error: String::new(),
                width: 0.0,
                height: 0.0,
                duration: 0.0,
                playing: false,
                base_pos: 0.0,
                base_at: Instant::now(),
                volume: 1.0,
                seek_gen: 0,
                seek_target: 0.0,
                frames: VecDeque::new(),
                decode_done: false,
                next_seq: 1,
            }),
            stop: AtomicBool::new(false),
        });
        let mut reg = registry().lock().unwrap();
        reg.0 += 1;
        let id = reg.0;
        reg.1.insert(id, Player { sh: sh.clone(), image: None, shown_seq: 0 });
        drop(reg);
        let _ = std::thread::Builder::new().name("video-decode".into()).spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&sh, &url, base)));
            let msg = match r {
                Ok(Ok(())) => return,
                Ok(Err(e)) => e,
                Err(_) => "video decoder crashed".to_string(),
            };
            sh.st.lock().unwrap().fail(msg);
        });
        id
    }

    // ---------------------------------------------------------------- fetch

    fn fetch(url: &str, base: Option<std::path::PathBuf>, sh: &Shared) -> Result<Vec<u8>, String> {
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return fetch_http(url, sh);
        }
        if lower.starts_with("data:") || lower.starts_with("blob:") {
            return Err("data:/blob: video URLs are not supported".into());
        }
        let mut path = url.to_string();
        if lower.starts_with("file:") {
            path = path[5..].trim_start_matches('/').to_string();
            // file:///C:/x -> C:/x ; file:///home/x -> /home/x
            if !(path.len() > 1 && path.as_bytes()[1] == b':') {
                path = format!("/{path}");
            }
            path = percent_decode(&path);
        }
        let p = std::path::PathBuf::from(path.replace('\\', "/"));
        let p = if p.exists() { p } else if let Some(b) = base { b.join(&p) } else { p };
        let meta = std::fs::metadata(&p).map_err(|e| format!("cannot open {}: {e}", p.display()))?;
        if meta.len() as usize > MAX_BYTES {
            return Err("video file too large (over 300 MB)".into());
        }
        std::fs::read(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))
    }

    fn percent_decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[cfg(feature = "web")]
    fn fetch_http(url: &str, sh: &Shared) -> Result<Vec<u8>, String> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        rt.block_on(async {
            let client = reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36")
                .connect_timeout(Duration::from_secs(15))
                .build()
                .map_err(|e| e.to_string())?;
            let mut resp = client.get(url).send().await.map_err(|e| format!("network error: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!("HTTP {}", resp.status().as_u16()));
            }
            if let Some(n) = resp.content_length() {
                if n as usize > MAX_BYTES {
                    return Err("video file too large (over 300 MB)".into());
                }
            }
            let mut buf = Vec::with_capacity(resp.content_length().unwrap_or(0) as usize);
            while let Some(chunk) = resp.chunk().await.map_err(|e| format!("network error: {e}"))? {
                if sh.stop.load(Ordering::Relaxed) {
                    return Err("closed".into());
                }
                buf.extend_from_slice(&chunk);
                if buf.len() > MAX_BYTES {
                    return Err("video file too large (over 300 MB)".into());
                }
            }
            Ok(buf)
        })
    }

    #[cfg(not(feature = "web"))]
    fn fetch_http(_url: &str, _sh: &Shared) -> Result<Vec<u8>, String> {
        Err("http video needs the `web` feature".into())
    }

    // ---------------------------------------------------------------- demux + video decode

    #[derive(Clone)]
    struct Bytes(Arc<Vec<u8>>);
    impl AsRef<[u8]> for Bytes {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    fn sniff(data: &[u8]) -> Result<(), String> {
        if data.len() >= 4 && data[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
            return Err("WebM/VP9 is not supported".into());
        }
        if data.starts_with(b"#EXTM3U") {
            return Err("HLS (.m3u8) streams are not supported".into());
        }
        if data.starts_with(b"OggS") {
            return Err("Ogg/Theora video is not supported".into());
        }
        let head = &data[..data.len().min(64)];
        if head.windows(4).any(|w| w == b"<!DO" || w == b"<htm" || w == b"<HTM") {
            return Err("server returned HTML, not a video".into());
        }
        if data.len() < 12 || !(&data[4..8] == b"ftyp" || &data[4..8] == b"moov" || &data[4..8] == b"mdat" || &data[4..8] == b"free" || &data[4..8] == b"wide") {
            return Err("unrecognized media format (only MP4 with H.264 is supported)".into());
        }
        Ok(())
    }

    struct SampleInfo {
        cts: f64,
        sync: bool,
    }

    fn run(sh: &Arc<Shared>, url: &str, base: Option<std::path::PathBuf>) -> Result<(), String> {
        let data = fetch(url, base, sh)?;
        if sh.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        sniff(&data)?;
        let data = Bytes(Arc::new(data));
        let size = data.0.len() as u64;
        let mut mp4 = mp4::Mp4Reader::read_header(Cursor::new(data.clone()), size).map_err(|e| format!("bad MP4: {e}"))?;

        // Pick the video and audio tracks.
        let mut vtrack: Option<u32> = None;
        let mut atrack: Option<u32> = None;
        let mut other_video = None;
        let mut ids: Vec<u32> = mp4.tracks().keys().copied().collect();
        ids.sort();
        for tid in ids {
            let t = &mp4.tracks()[&tid];
            match t.media_type() {
                Ok(mp4::MediaType::H264) if vtrack.is_none() => vtrack = Some(tid),
                Ok(mp4::MediaType::AAC) if atrack.is_none() => atrack = Some(tid),
                Ok(mp4::MediaType::H265) => other_video = Some("HEVC/H.265 is not supported"),
                Ok(mp4::MediaType::VP9) => other_video = Some("VP9 is not supported"),
                Err(_) if matches!(t.track_type(), Ok(mp4::TrackType::Video)) => {
                    other_video = Some("unsupported video codec (only H.264 is supported)")
                },
                _ => {},
            }
        }
        let Some(vt) = vtrack else {
            // An audio-only file (.m4a): no pictures, just the AAC track on the player's clock.
            let Some(at) = atrack else {
                return Err(other_video.unwrap_or("no H.264 video or AAC audio track").to_string());
            };
            let t = &mp4.tracks()[&at];
            let duration = t.duration().as_secs_f64();
            {
                let mut st = sh.st.lock().unwrap();
                st.duration = duration;
                st.decode_done = true;
                if st.state == 0 {
                    st.state = 1;
                }
            }
            audio::run(sh, data, at);
            return Ok(());
        };
        let (avcc, timescale, tw, th) = {
            let t = &mp4.tracks()[&vt];
            let avc1 = t.trak.mdia.minf.stbl.stsd.avc1.as_ref().ok_or("missing avc1")?;
            (avc1.avcc.clone(), t.timescale().max(1) as f64, t.width() as f64, t.height() as f64)
        };
        let nal_len = (avcc.length_size_minus_one & 3) as usize + 1;
        let mut header = Vec::new();
        for n in avcc.sequence_parameter_sets.iter().chain(avcc.picture_parameter_sets.iter()) {
            header.extend_from_slice(&[0, 0, 0, 1]);
            header.extend_from_slice(&n.bytes);
        }

        // Sample table: composition times (seconds, first frame at 0) and keyframe flags.
        let count = mp4.sample_count(vt).map_err(|e| e.to_string())?;
        if count == 0 {
            return Err("video track has no samples".into());
        }
        let mut samples: Vec<SampleInfo> = Vec::with_capacity(count as usize);
        let mut end = 0f64;
        for i in 1..=count {
            match mp4.read_sample(vt, i) {
                Ok(Some(s)) => {
                    let cts = (s.start_time as i64 + s.rendering_offset as i64) as f64 / timescale;
                    end = end.max(cts + s.duration as f64 / timescale);
                    samples.push(SampleInfo { cts, sync: s.is_sync });
                },
                _ => samples.push(SampleInfo { cts: f64::NAN, sync: false }),
            }
        }
        let t0 = samples.iter().take(64).map(|s| s.cts).filter(|c| c.is_finite()).fold(f64::INFINITY, f64::min);
        let t0 = if t0.is_finite() { t0 } else { 0.0 };
        for s in &mut samples {
            s.cts -= t0;
        }
        let duration = (end - t0).max(0.0);
        let frame_dur = (duration / count as f64).clamp(1.0 / 240.0, 1.0);
        {
            let mut st = sh.st.lock().unwrap();
            st.width = tw;
            st.height = th;
            st.duration = duration;
        }

        if let Some(at) = atrack {
            let sh2 = sh.clone();
            let d2 = data.clone();
            let _ = std::thread::Builder::new().name("video-audio".into()).spawn(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| audio::run(&sh2, d2, at)));
            });
        }

        let mut dec: Option<openh264::decoder::Decoder> = None;
        let mut gen = u64::MAX;
        let mut i = 0usize; // next sample index (0-based)
        let mut pending: Vec<f64> = Vec::new();
        let mut skip_before = 0f64;
        let mut au = Vec::new();
        let nf = openh264::decoder::DecodeOptions::new().flush_after_decode(openh264::decoder::Flush::NoFlush);
        loop {
            if sh.stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            // Seek / start: restart decoding at the keyframe before the target.
            let (cur_gen, target, qlen, playing, clock) = {
                let st = sh.st.lock().unwrap();
                (st.seek_gen, st.seek_target, st.frames.len(), st.playing, st.clock())
            };
            if cur_gen != gen {
                gen = cur_gen;
                let mut k = 0;
                for (j, s) in samples.iter().enumerate() {
                    if s.sync && s.cts <= target + 1e-6 {
                        k = j;
                    }
                    if s.sync && s.cts > target + 5.0 {
                        break;
                    }
                }
                i = k;
                pending.clear();
                skip_before = target - frame_dur * 1.01;
                let mut d = openh264::decoder::Decoder::with_api_config(
                    openh264::OpenH264API::from_source(),
                    openh264::decoder::DecoderConfig::new().flush_after_decode(openh264::decoder::Flush::NoFlush),
                )
                .map_err(|e| format!("H.264 decoder init failed: {e}"))?;
                let _ = d.decode_with_options(&header, nf.clone());
                dec = Some(d);
            }
            let d = dec.as_mut().unwrap();
            if qlen >= QUEUE || i > samples.len() {
                std::thread::sleep(Duration::from_millis(4));
                continue;
            }
            if i == samples.len() {
                // End of stream: drain the reorder buffer.
                let mut out = Vec::new();
                if let Ok(frames) = d.flush_remaining() {
                    for y in frames {
                        pending.sort_by(|a, b| b.partial_cmp(a).unwrap());
                        let pts = pending.pop().unwrap_or(duration);
                        if pts >= skip_before {
                            out.push((pts, to_rgba(&y)));
                        }
                    }
                }
                let mut st = sh.st.lock().unwrap();
                if st.seek_gen == gen {
                    for (pts, (w, h, rgba)) in out {
                        push(&mut st, pts, w, h, rgba);
                    }
                    st.decode_done = true;
                }
                i += 1;
                continue;
            }
            let s = match mp4.read_sample(vt, i as u32 + 1) {
                Ok(Some(s)) => s,
                _ => {
                    i += 1;
                    continue;
                },
            };
            let pts = samples[i].cts;
            i += 1;
            // AVCC (length-prefixed) -> Annex B.
            au.clear();
            let b = &s.bytes[..];
            let mut p = 0;
            while p + nal_len <= b.len() {
                let mut n = 0usize;
                for k in 0..nal_len {
                    n = (n << 8) | b[p + k] as usize;
                }
                p += nal_len;
                if n == 0 || p + n > b.len() {
                    break;
                }
                au.extend_from_slice(&[0, 0, 0, 1]);
                au.extend_from_slice(&b[p..p + n]);
                p += n;
            }
            if au.is_empty() {
                continue;
            }
            pending.push(pts);
            match d.decode_with_options(&au, nf.clone()) {
                Ok(Some(y)) => {
                    // OpenH264 hands pictures back in display order: the earliest pending pts is this one.
                    pending.sort_by(|a, b| b.partial_cmp(a).unwrap());
                    let fpts = pending.pop().unwrap_or(pts);
                    if pending.len() > 32 {
                        pending.sort_by(|a, b| b.partial_cmp(a).unwrap());
                        pending.pop();
                    }
                    // Skip the colour conversion for frames before a seek target, or hopelessly late ones.
                    let late = playing && qlen > 0 && fpts < clock - 0.15;
                    if fpts < skip_before || late {
                        continue;
                    }
                    let (w, h, rgba) = to_rgba(&y);
                    let mut st = sh.st.lock().unwrap();
                    if st.seek_gen == gen {
                        push(&mut st, fpts, w, h, rgba);
                    }
                },
                Ok(None) => {},
                Err(_) => {
                    // corrupt access unit: forget its timestamp, keep going
                    if let Some(pos) = pending.iter().position(|&x| x == pts) {
                        pending.remove(pos);
                    }
                },
            }
        }
    }

    fn push(st: &mut St, pts: f64, w: u32, h: u32, rgba: Vec<u8>) {
        let seq = st.next_seq;
        st.next_seq += 1;
        st.frames.push_back(Frame { pts, w, h, rgba, seq });
        if st.state == 0 {
            st.state = 1;
        }
    }

    /// I420 -> RGBA (BT.601 limited range), nearest-neighbour downscaled to at most MAX_W wide.
    fn to_rgba(y: &openh264::decoder::DecodedYUV<'_>) -> (u32, u32, Vec<u8>) {
        use openh264::formats::YUVSource;
        use rayon::prelude::*;
        let (sw, sh) = y.dimensions();
        let (ys, us, _vs) = y.strides();
        let (dw, dh) = if sw > MAX_W { (MAX_W, (sh * MAX_W / sw).max(1)) } else { (sw, sh) };
        let (yp, up, vp) = (y.y(), y.u(), y.v());
        let mut out = vec![0u8; dw * dh * 4];
        out.par_chunks_mut(dw * 4).enumerate().for_each(|(dy, row)| {
            let sy = if dh == sh { dy } else { dy * sh / dh };
            let yrow = &yp[sy * ys..];
            let urow = &up[(sy / 2) * us..];
            let vrow = &vp[(sy / 2) * us..];
            for dx in 0..dw {
                let sx = if dw == sw { dx } else { dx * sw / dw };
                let c = (yrow[sx] as i32 - 16) * 298;
                let d = urow[sx / 2] as i32 - 128;
                let e = vrow[sx / 2] as i32 - 128;
                let o = dx * 4;
                row[o] = ((c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
                row[o + 1] = ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
                row[o + 2] = ((c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
                row[o + 3] = 255;
            }
        });
        (dw as u32, dh as u32, out)
    }

    // ---------------------------------------------------------------- audio

    mod audio {
        use super::{Bytes, Shared};
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use std::collections::VecDeque;
        use std::io::Cursor;
        use std::sync::atomic::Ordering;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;
        use symphonia::core::audio::SampleBuffer;
        use symphonia::core::codecs::{CodecParameters, DecoderOptions, CODEC_TYPE_AAC};
        use symphonia::core::formats::Packet;

        struct Ring {
            buf: VecDeque<f32>,
            front_pts: f64,
            gen: u64,
        }

        pub(super) fn run(sh: &Arc<Shared>, data: Bytes, tid: u32) {
            let size = data.0.len() as u64;
            let Ok(mut mp4) = mp4::Mp4Reader::read_header(Cursor::new(data), size) else { return };
            let (timescale, sample_rate, src_ch, asc) = {
                let t = &mp4.tracks()[&tid];
                let Some(mp4a) = t.trak.mdia.minf.stbl.stsd.mp4a.as_ref() else { return };
                let Some(esds) = mp4a.esds.as_ref() else { return };
                let ds = &esds.es_desc.dec_config.dec_specific;
                let mut prof = ds.profile;
                if prof == 5 || prof == 29 || prof == 0 {
                    prof = 2; // HE-AAC: decode the AAC-LC core
                }
                const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
                let fi = ds.freq_index.min(12);
                let ch = if ds.chan_conf == 0 { mp4a.channelcount.max(1) as u8 } else { ds.chan_conf };
                let asc = [(prof << 3) | (fi >> 1), ((fi & 1) << 7) | ((ch & 0xF) << 3)];
                (t.timescale().max(1) as f64, RATES[fi as usize], ch.clamp(1, 8) as usize, asc)
            };
            let count = mp4.sample_count(tid).unwrap_or(0);
            if count == 0 {
                return;
            }
            let mut params = CodecParameters::new();
            params.for_codec(CODEC_TYPE_AAC).with_sample_rate(sample_rate).with_extra_data(Box::new(asc));
            let Ok(mut dec) = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()) else { return };

            // Output device.
            let host = cpal::default_host();
            let Some(dev) = host.default_output_device() else { return };
            let Ok(cfg) = dev.default_output_config() else { return };
            let rate = cfg.sample_rate().0 as f64;
            let chans = cfg.channels() as usize;
            let fmt = cfg.sample_format();
            let ring = Arc::new(Mutex::new(Ring { buf: VecDeque::new(), front_pts: 0.0, gen: u64::MAX }));
            let config: cpal::StreamConfig = cfg.into();

            let fill = {
                let ring = ring.clone();
                let sh = sh.clone();
                move |out: &mut [f32]| {
                    out.fill(0.0);
                    let (playing, clock, vol, gen) = {
                        let st = sh.st.lock().unwrap();
                        (st.playing, st.clock(), st.volume, st.seek_gen)
                    };
                    if !playing {
                        return;
                    }
                    let mut r = ring.lock().unwrap();
                    if r.gen != gen || r.buf.is_empty() {
                        return;
                    }
                    let drift = r.front_pts - clock;
                    if drift > 0.12 {
                        return; // audio ahead of the picture clock: wait
                    }
                    if drift < -0.12 {
                        let drop = (((-drift) * rate) as usize * chans).min(r.buf.len());
                        let drop = drop - drop % chans;
                        r.buf.drain(..drop);
                        r.front_pts += (drop / chans) as f64 / rate;
                    }
                    let n = out.len().min(r.buf.len());
                    let n = n - n % chans;
                    for (o, s) in out.iter_mut().zip(r.buf.drain(..n)) {
                        *o = s * vol;
                    }
                    r.front_pts += (n / chans) as f64 / rate;
                }
            };
            let err = |_e: cpal::StreamError| {};
            let stream = match fmt {
                cpal::SampleFormat::F32 => dev.build_output_stream(&config, move |o: &mut [f32], _: &cpal::OutputCallbackInfo| fill(o), err, None),
                cpal::SampleFormat::I16 => {
                    let mut tmp = Vec::new();
                    dev.build_output_stream(
                        &config,
                        move |o: &mut [i16], _: &cpal::OutputCallbackInfo| {
                            tmp.resize(o.len(), 0.0);
                            fill(&mut tmp);
                            for (d, s) in o.iter_mut().zip(&tmp) {
                                *d = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                            }
                        },
                        err,
                        None,
                    )
                },
                cpal::SampleFormat::U16 => {
                    let mut tmp = Vec::new();
                    dev.build_output_stream(
                        &config,
                        move |o: &mut [u16], _: &cpal::OutputCallbackInfo| {
                            tmp.resize(o.len(), 0.0);
                            fill(&mut tmp);
                            for (d, s) in o.iter_mut().zip(&tmp) {
                                *d = ((s.clamp(-1.0, 1.0) * 32767.0) + 32768.0) as u16;
                            }
                        },
                        err,
                        None,
                    )
                },
                _ => return,
            };
            let Ok(stream) = stream else { return };
            if stream.play().is_err() {
                return;
            }
            if std::env::var_os("LING_VIDEO_DEBUG").is_some() {
                eprintln!("video audio: AAC {sample_rate} Hz x{src_ch} -> device {rate} Hz x{chans} ({fmt:?})");
            }

            // Decode loop: keep ~0.5 s buffered at the device rate/channel count.
            let ratio = sample_rate as f64 / rate;
            let mut i = 1u32;
            let mut gen = u64::MAX;
            let mut prev = vec![0f32; src_ch];
            let mut pos = 0f64;
            let mut sbuf: Option<SampleBuffer<f32>> = None;
            let mut outv: Vec<f32> = Vec::new();
            while !sh.stop.load(Ordering::Relaxed) {
                let (cur_gen, target) = {
                    let st = sh.st.lock().unwrap();
                    (st.seek_gen, st.seek_target)
                };
                if cur_gen != gen {
                    gen = cur_gen;
                    // AAC frames are a fixed 1024 samples, so the sample index follows from the time.
                    i = ((target * timescale / 1024.0).floor() as u32).clamp(0, count.saturating_sub(1)) + 1;
                    dec.reset();
                    prev.iter_mut().for_each(|v| *v = 0.0);
                    pos = 0.0;
                    let mut r = ring.lock().unwrap();
                    r.buf.clear();
                    r.gen = gen;
                }
                let buffered = ring.lock().unwrap().buf.len();
                if buffered > (rate * 0.5) as usize * chans || i > count {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                let Ok(Some(s)) = mp4.read_sample(tid, i) else {
                    i += 1;
                    continue;
                };
                i += 1;
                let pts = s.start_time as f64 / timescale;
                let pkt = Packet::new_from_slice(0, s.start_time, s.duration as u64, &s.bytes);
                let Ok(buf) = dec.decode(&pkt) else { continue };
                let spec = *buf.spec();
                let frames = buf.frames();
                if frames == 0 {
                    continue;
                }
                let sb = sbuf.get_or_insert_with(|| SampleBuffer::<f32>::new(buf.capacity() as u64, spec));
                if sb.capacity() < buf.capacity() * spec.channels.count() {
                    *sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
                }
                sb.copy_interleaved_ref(buf);
                let sc = spec.channels.count().max(1);
                let src = sb.samples();
                let n = src.len() / sc;
                if prev.len() != sc {
                    prev = vec![0.0; sc];
                }
                // Streaming linear resample: index 0 is the previous packet's last frame.
                outv.clear();
                let frame = |k: usize, c: usize| if k == 0 { prev[c] } else { src[(k - 1) * sc + c] };
                let first_out_pts = pts + (pos - 1.0) / sample_rate as f64;
                while pos < n as f64 {
                    let k = pos as usize;
                    let f = (pos - k as f64) as f32;
                    for d in 0..chans {
                        let c = if sc == 1 { 0 } else { d % sc };
                        let a = frame(k, c);
                        let b = frame((k + 1).min(n), c);
                        outv.push(a + (b - a) * f);
                    }
                    pos += ratio;
                }
                pos -= n as f64;
                prev.copy_from_slice(&src[(n - 1) * sc..n * sc]);
                let mut r = ring.lock().unwrap();
                if r.gen != gen {
                    continue;
                }
                if r.buf.is_empty() {
                    r.front_pts = first_out_pts.max(0.0);
                }
                r.buf.extend(outv.iter().copied());
            }
            drop(stream);
        }
    }
}
