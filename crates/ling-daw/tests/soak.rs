//! Not part of the normal run: plays every project in a folder through the engine for several minutes of audio, faster than real time,
//! and reports any panic or non-finite sample.  cargo test --release -p ling-daw --test soak -- --ignored --nocapture
//! SOAK_DIR = the projects folder, SOAK_MINUTES = minutes of audio per project (default 6), SOAK_ONLY = a name filter.
use ling_daw::{Daw, Val};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn projects(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            for f in ["project.json", &format!("{}.json", e.file_name().to_string_lossy())] {
                let c = p.join(f);
                if c.exists() {
                    out.push(c);
                    break;
                }
            }
        }
    }
    out.sort();
    out
}

#[test]
#[ignore]
fn soak_projects() {
    let dir = std::env::var("SOAK_DIR").unwrap_or_else(|_| r"C:\Users\User\Programs\LingStationX\projects".into());
    let minutes: f64 = std::env::var("SOAK_MINUTES").ok().and_then(|s| s.parse().ok()).unwrap_or(6.0);
    let only = std::env::var("SOAK_ONLY").unwrap_or_default();
    let mut bad = vec![];
    for path in projects(std::path::Path::new(&dir)) {
        let name = path.to_string_lossy().to_string();
        if !only.is_empty() && !name.contains(&only) {
            continue;
        }
        let t0 = std::time::Instant::now();
        let r = catch_unwind(AssertUnwindSafe(|| {
            let mut d = Daw::new();
            let ok = d.call("daw_project_load", &[Val::Str(name.clone())]).and_then(|r| r.ok());
            if !matches!(ok, Some(Val::Num(x)) if x > 0.5) {
                return Err("did not load".to_string());
            }
            d.call("daw_set_loop", &[Val::Num(0.0), Val::Num(0.0), Val::Num(64.0)]);
            d.call("daw_play", &[]);
            let eng = d.engine();
            let frames_total = (minutes * 60.0 * 48000.0) as usize;
            let mut buf = vec![0.0f32; 2 * 1024];
            let mut done = 0usize;
            let mut nonfinite = 0usize;
            let mut peak = 0.0f32;
            while done < frames_total {
                {
                    let mut en = eng.lock().unwrap();
                    en.render(&mut buf);
                }
                for x in &buf {
                    if !x.is_finite() {
                        nonfinite += 1;
                    } else {
                        peak = peak.max(x.abs());
                    }
                }
                done += 1024;
                // what the UI does between blocks
                if done % (48000 * 5) < 1024 {
                    d.call("daw_position", &[]);
                    d.call("daw_playing", &[]);
                }
            }
            Ok((nonfinite, peak))
        }));
        match r {
            Ok(Ok((nf, peak))) => {
                println!("ok    {name}  {:.1}s  peak {peak:.2}  non-finite {nf}", t0.elapsed().as_secs_f32());
                if nf > 0 {
                    bad.push(format!("{name}: {nf} non-finite samples"));
                }
            }
            Ok(Err(e)) => println!("skip  {name}: {e}"),
            Err(p) => {
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                println!("PANIC {name}: {msg}");
                bad.push(format!("{name}: panic {msg}"));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
