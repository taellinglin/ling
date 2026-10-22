//! Not a normal test: prints what the theory analysis finds in a project (and in the raw MIDI import) so the fixer can be checked by eye.
//!   ANTHEM_JSON=<project.json> ANTHEM_MID=<file.mid> cargo test --release -p ling-daw --test anthem_dump -- --ignored --nocapture
use ling_daw::{Daw, Val};

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn call(d: &mut Daw, name: &str, a: &[Val]) -> Val {
    d.call(name, a).unwrap_or_else(|| panic!("{name} not handled")).unwrap_or_else(|e| panic!("{name}: {e}"))
}
fn nums(v: &Val) -> Vec<f64> {
    match v {
        Val::List(l) => l.iter().map(|x| if let Val::Num(x) = x { *x } else { f64::NAN }).collect(),
        _ => vec![],
    }
}
fn text(v: &Val) -> Vec<String> {
    match v {
        Val::List(l) => l.iter().map(|x| match x {
            Val::Str(s) => s.clone(),
            Val::Num(n) => format!("{n}"),
            _ => String::new(),
        }).collect(),
        _ => vec![],
    }
}

const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
fn pitch(k: f64) -> String {
    format!("{}{}", NAMES[(k as usize) % 12], (k as i32) / 12 - 1)
}

fn report(d: &mut Daw, label: &str) {
    println!("\n===== {label}");
    let nt = nums(&call(d, "daw_track_count", &[]))[..].first().copied().unwrap_or(0.0) as usize;
    let nt = if nt == 0 { if let Val::Num(x) = call(d, "daw_track_count", &[]) { x as usize } else { 0 } } else { nt };
    for t in 0..nt {
        let name = if let Val::Str(s) = call(d, "daw_track_name", &[n(t as f64)]) { s } else { String::new() };
        let nc = if let Val::Num(x) = call(d, "daw_clip_count", &[n(t as f64)]) { x as usize } else { 0 };
        for c in 0..nc {
            let sm = call(d, "daw_th_summary", &[n(t as f64), n(c as f64)]);
            let s = text(&sm);
            if s.is_empty() {
                continue;
            }
            println!("-- track {t} {name} clip {c}: key {} conf {:.2} chords {} out {} tension {} passing {} keychanges {} fromothers {}", s[0], s[3].parse::<f64>().unwrap_or(0.0), s[4], s[5], s[6], s[7], s[9], s[10]);
            let keys = text(&call(d, "daw_th_keys", &[n(t as f64), n(c as f64)]));
            let ks: Vec<String> = keys.chunks(5).map(|k| format!("{}..{}={}", k[0], k[1], k[4])).collect();
            println!("   keys: {}", ks.join("  "));
            let notes = nums(&call(d, "daw_clip_notes", &[n(t as f64), n(c as f64)]));
            let roles = nums(&call(d, "daw_th_roles", &[n(t as f64), n(c as f64)]));
            let mut shown = 0;
            for i in 0..notes.len() / 7 {
                let (start, len, key) = (notes[i * 7], notes[i * 7 + 1], notes[i * 7 + 2]);
                let role = roles.get(i * 2).copied().unwrap_or(-1.0);
                if role >= 3.0 && shown < 14 {
                    println!("   bar {:>5.1}  {} len {:.2}  role {} (suggest {})", start / 4.0 + 1.0, pitch(key), len, role, roles.get(i * 2 + 1).copied().unwrap_or(-1.0));
                    shown += 1;
                }
            }
        }
    }
}

#[test]
#[ignore]
fn dump() {
    let json = std::env::var("ANTHEM_JSON").unwrap_or_default();
    let mid = std::env::var("ANTHEM_MID").unwrap_or_default();
    if !mid.is_empty() {
        let mut d = Daw::new();
        call(&mut d, "daw_new", &[]);
        let scan = call(&mut d, "daw_midi_scan", &[Val::Str(mid.clone())]);
        let parts = if let Val::List(l) = &scan { l.len() / 6 } else { 0 };
        let choices = Val::List(vec![Val::Str("auto".into()); parts]);
        call(&mut d, "daw_midi_import", &[Val::Str(mid), n(0.0), choices]);
        report(&mut d, "RAW IMPORT");
        let r = call(&mut d, "daw_fix_run", &[n(15.0), n(8.0)]);
        println!("fix_run -> {:?}", nums(&r));
        report(&mut d, "AFTER daw_fix_run(31, 8)");
    }
    if !json.is_empty() {
        let mut d = Daw::new();
        call(&mut d, "daw_project_load", &[Val::Str(json)]);
        report(&mut d, "SAVED PROJECT");
    }
}


#[test]
#[ignore]
fn diff() {
    let mid = std::env::var("ANTHEM_MID").unwrap_or_default();
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    let scan = call(&mut d, "daw_midi_scan", &[Val::Str(mid.clone())]);
    let parts = if let Val::List(l) = &scan { l.len() / 6 } else { 0 };
    call(&mut d, "daw_midi_import", &[Val::Str(mid), n(0.0), Val::List(vec![Val::Str("auto".into()); parts])]);
    let nt = if let Val::Num(x) = call(&mut d, "daw_track_count", &[]) { x as usize } else { 0 };
    let before: Vec<Vec<f64>> = (0..nt).map(|t| nums(&call(&mut d, "daw_clip_notes", &[n(t as f64), n(0.0)]))).collect();
    call(&mut d, "daw_fix_run", &[n(15.0), n(8.0)]);
    for t in 0..nt {
        let after = nums(&call(&mut d, "daw_clip_notes", &[n(t as f64), n(0.0)]));
        let name = if let Val::Str(s) = call(&mut d, "daw_track_name", &[n(t as f64)]) { s } else { String::new() };
        let (b, a) = (&before[t], &after);
        println!("
== track {t} {name}: {} -> {} notes", b.len() / 7, a.len() / 7);
        let mut shown = 0;
        // notes are compared by order; removed notes shift the pairing, so also report count changes
        let m = (b.len() / 7).min(a.len() / 7);
        for i in 0..m {
            let (bs, bl, bk) = (b[i * 7], b[i * 7 + 1], b[i * 7 + 2]);
            let (as_, al, ak) = (a[i * 7], a[i * 7 + 1], a[i * 7 + 2]);
            if (bk - ak).abs() > 0.5 || (bs - as_).abs() > 0.01 || (bl - al).abs() > 0.01 {
                if shown < 40 {
                    println!("   #{i:<4} bar {:>5.2}: {} len {:.2}  ->  bar {:>5.2}: {} len {:.2}{}", bs / 4.0 + 1.0, pitch(bk), bl, as_ / 4.0 + 1.0, pitch(ak), al, if (bk - ak).abs() > 0.5 { format!("   PITCH {:+}", ak - bk) } else { String::new() });
                }
                shown += 1;
            }
        }
        println!("   ({shown} changes)");
    }
}


#[test]
#[ignore]
fn show() {
    let mid = std::env::var("ANTHEM_MID").unwrap_or_default();
    let (t, b0, b1): (usize, f64, f64) = (std::env::var("T").unwrap().parse().unwrap(), std::env::var("B0").unwrap().parse().unwrap(), std::env::var("B1").unwrap().parse().unwrap());
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    let scan = call(&mut d, "daw_midi_scan", &[Val::Str(mid.clone())]);
    let parts = if let Val::List(l) = &scan { l.len() / 6 } else { 0 };
    call(&mut d, "daw_midi_import", &[Val::Str(mid), n(0.0), Val::List(vec![Val::Str("auto".into()); parts])]);
    let notes = nums(&call(&mut d, "daw_clip_notes", &[n(t as f64), n(0.0)]));
    let mut line = String::new();
    for i in 0..notes.len() / 7 {
        let bar = notes[i * 7] / 4.0 + 1.0;
        if bar >= b0 && bar < b1 {
            line += &format!("{:.2}:{}({:.2}) ", bar, pitch(notes[i * 7 + 2]), notes[i * 7 + 1]);
        }
    }
    println!("TRACK {t} bars {b0}-{b1}: {line}");
    // the chords the others play
    for tt in 0..8usize {
        if tt == t { continue; }
        let nn = nums(&call(&mut d, "daw_clip_notes", &[n(tt as f64), n(0.0)]));
        let mut l2 = String::new();
        for i in 0..nn.len() / 7 {
            let bar = nn[i * 7] / 4.0 + 1.0;
            if bar >= b0 && bar < b1 && nn[i * 7 + 1] >= 0.5 { l2 += &format!("{:.2}:{}({:.2}) ", bar, pitch(nn[i * 7 + 2]), nn[i * 7 + 1]); }
        }
        if !l2.is_empty() { println!("  other {tt}: {l2}"); }
    }
}
