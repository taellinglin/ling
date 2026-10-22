//! Builtins for the Sampler instrument (`daw_smp_*`, see `dsp/sampler.rs`) and for finding drum samples on disk.
use super::{Daw, A, Val};

/// audio files under `roots`, as (path, lower-case path words)
fn index(roots: &[String]) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut stack: Vec<std::path::PathBuf> = roots.iter().map(std::path::PathBuf::from).collect();
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(ext) = p.extension().map(|x| x.to_string_lossy().to_lowercase()) {
                if ["wav", "flac", "ogg", "mp3", "wv", "aif", "aiff"].contains(&ext.as_str()) {
                    seen += 1;
                    if seen > 200_000 {
                        return out;
                    }
                    let full = p.to_string_lossy().replace('\\', "/");
                    out.push((full.clone(), full.to_lowercase()));
                }
            }
        }
    }
    out
}

/// how well a path matches the wanted words: every word must appear; words in the file name count more, a short name beats a long one
fn score(path_lc: &str, words: &[String]) -> Option<f64> {
    let name = path_lc.rsplit('/').next().unwrap_or(path_lc);
    let mut s = 0.0;
    for w in words {
        if name.contains(w.as_str()) {
            s += 3.0;
        } else if path_lc.contains(w.as_str()) {
            s += 1.0;
        } else {
            return None;
        }
    }
    Some(s - name.len() as f64 * 0.01)
}

/// Word sets to try, most specific first: "acoustic bass drum" -> ["kick"], "open hi-hat" -> ["open", "hat"] -> ["hat"] ...
fn search_sets(words: &[String]) -> Vec<Vec<String>> {
    let text = words.join(" ");
    let mut t = text.clone();
    for (from, to) in [("acoustic bass drum", "kick"), ("bass drum", "kick"), ("bassdrum", "kick"), ("snare drum", "snare"), ("hi-hat", "hat"), ("hihat", "hat"), ("hi hat", "hat"), ("high floor tom", "tom"), ("low floor tom", "tom"), ("floor tom", "tom"), ("chinese cymbal", "china cymbal"), ("splash cymbal", "splash"), ("crash cymbal", "crash"), ("ride cymbal", "ride"), ("wood block", "wood"), ("side stick", "rim"), ("hand clap", "clap")] {
        t = t.replace(from, to);
    }
    let norm: Vec<String> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 1).map(|w| w.to_string()).collect();
    let generic = ["drum", "drums", "low", "high", "mid", "acoustic", "electric", "electronic", "closed", "pedal", "hit", "sample", "one", "shot", "oneshot", "1", "2", "3"];
    let core: Vec<String> = norm.iter().filter(|w| !generic.contains(&w.as_str())).cloned().collect();
    let mut sets: Vec<Vec<String>> = vec![words.to_vec(), norm.clone(), core.clone()];
    if let Some(last) = core.last() {
        sets.push(vec![last.clone()]);
    }
    if let Some(first) = core.first() {
        sets.push(vec![first.clone()]);
    }
    sets.retain(|s| !s.is_empty());
    sets.dedup();
    sets
}

impl Daw {
    /// the best sample file for some words (a drum name) under the sample folders, if there is one
    pub(super) fn find_sample(&mut self, words: &str) -> Option<String> {
        let roots: Vec<String> = self.smp_roots.split('|').filter(|r| !r.trim().is_empty()).map(|r| r.trim().to_string()).collect();
        let words: Vec<String> = words.to_lowercase().split_whitespace().map(|w| w.to_string()).collect();
        if words.is_empty() || roots.is_empty() {
            return None;
        }
        if self.smp_index.as_ref().map(|(k, _)| *k != self.smp_roots).unwrap_or(true) {
            self.smp_index = Some((self.smp_roots.clone(), index(&roots)));
        }
        let idx = &self.smp_index.as_ref()?.1;
        // the words as asked, then as a sample library says them, then fewer and fewer of them
        for set in search_sets(&words) {
            let mut hits: Vec<(f64, &String)> = idx.iter().filter_map(|(p, lc)| score(lc, &set).map(|s| (s, p))).collect();
            hits.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(y.1)));
            // the best one that can actually be read
            for (_, p) in hits.into_iter().take(80) {
                if crate::dsp::sampler::load_audio(p).map(|(d, _)| !d.is_empty()).unwrap_or(false) {
                    return Some(p.clone());
                }
            }
        }
        None
    }

    pub(super) fn smp_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        if name == "daw_smp_roots_set" {
            // folders to look for samples in, joined with |
            self.smp_roots = a.s(0);
            self.smp_index = None;
            return ok!(true);
        }
        if name == "daw_smp_pick" {
            // (words) -> the single best sample file, "" when none
            return ok!(self.find_sample(&a.s(0)).unwrap_or_default());
        }
        if name == "daw_smp_find" {
            // (words "snare 808", roots "dir1|dir2", how many) -> best matching sample files
            let words: Vec<String> = a.s(0).to_lowercase().split_whitespace().map(|w| w.to_string()).collect();
            let roots: Vec<String> = a.s(1).split('|').filter(|r| !r.trim().is_empty()).map(|r| r.trim().to_string()).collect();
            if words.is_empty() || roots.is_empty() {
                return ok!(Val::List(vec![]));
            }
            let key = a.s(1);
            if self.smp_index.as_ref().map(|(k, _)| *k != key).unwrap_or(true) {
                self.smp_index = Some((key, index(&roots)));
            }
            let idx = &self.smp_index.as_ref().unwrap().1;
            let mut hits: Vec<(f64, &String)> = idx.iter().filter_map(|(p, lc)| score(lc, &words).map(|s| (s, p))).collect();
            hits.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(y.1)));
            return ok!(Val::List(hits.into_iter().take(a.u(2).clamp(1, 50)).map(|h| Val::Str(h.1.clone())).collect()));
        }
        if name == "daw_smp_forget" {
            self.smp_index = None;
            return ok!(true);
        }
        let ti = a.u(0);
        let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
        let Some(inst) = en.project.tracks.get_mut(ti).and_then(|t| t.inst.as_mut()) else { return ok!(Val::List(vec![])) };
        if inst.name() != "Sampler" {
            return ok!(Val::List(vec![]));
        }
        let nums = |v: Vec<f64>| Val::List(v.into_iter().map(Val::Num).collect());
        match name {
            "daw_smp_zones" => ok!(nums(inst.command("zones", &[], ""))),
            "daw_smp_names" => ok!(Val::List(inst.info("names").split('\n').filter(|s| !s.is_empty()).map(|s| Val::Str(s.to_string())).collect())),
            "daw_smp_add" => {
                let r = inst.command("add", &[], &a.s(1));
                let err = inst.info("error");
                drop(en);
                if r.first().copied().unwrap_or(-1.0) < 0.0 {
                    self.status = format!("Sampler: {err}");
                } else {
                    self.touch();
                }
                ok!(nums(r))
            }
            "daw_smp_fx" => ok!(nums(inst.command("zfx", &[a.n(1)], ""))),
            "daw_smp_env" => ok!(nums(inst.command("env", &[a.n(1)], ""))),
            "daw_smp_remove" | "daw_smp_set" | "daw_smp_detect" | "daw_smp_autoloop" | "daw_smp_spread" | "daw_smp_drop_env" => {
                let cmd = &name["daw_smp_".len()..];
                let args: Vec<f64> = (1..a.0.len()).map(|i| a.n(i)).collect();
                let r = inst.command(cmd, &args, "");
                drop(en);
                self.touch();
                ok!(nums(r))
            }
            "daw_smp_peaks" => ok!(nums(inst.command("peaks", &[a.n(1), a.n(2)], ""))),
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
