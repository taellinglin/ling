//! Musical context for effects, and bus moves on effects.
//!
//! * `feed_keys` hands every key-following effect (the Colour Bass's chord, the Comb's key tracking) the notes sounding in what it
//!   follows: its own track, one other track (a "key track"), or the song's chord (the other pitched tracks, chord parts first).
//!   The engine calls it once per slice before the tracks render; an effect is only asked again when the beat leaves the range its
//!   keys were valid for (the next note start or end), so it costs nothing between note changes. Arrangement clips only (loops
//!   included, muted tracks count, so a muted "key track" can steer the harmony); the launcher's clips are not read.
//! * `riser_narrowing` is the build move for risers: an Imager width lane that narrows the riser to mono through each build and
//!   opens it wider than normal on the drop.
use crate::dsp::fx::KeySource;
use crate::model::{AutoLane, ClipData, EffectSlot, Track};

/// may this track's notes stand for the harmony? (pitched instruments with clips; drum kits are not)
fn pitched(t: &Track) -> bool {
    t.inst.is_some() && t.inst_kind != "drums" && !t.clips.is_empty()
}

/// the keys of `t` sounding at beat `p0` as a bit set; `until` is pulled down to the next beat where that can change
fn sounding(t: &Track, p0: f64, until: &mut f64) -> u128 {
    let mut set = 0u128;
    for clip in &t.clips {
        let (cs, cl) = (clip.start, clip.len);
        if p0 >= cs + cl {
            continue;
        }
        if p0 < cs {
            *until = until.min(cs);
            continue;
        }
        let ClipData::Midi(notes) = &clip.data else { continue };
        let ll = if clip.loop_len >= 0.25 && clip.loop_len < cl { clip.loop_len } else { 0.0 };
        let (base, limit) = if ll > 0.0 {
            let b = cs + ((p0 - cs) / ll).floor() * ll;
            (b, ll.min(cs + cl - b))
        } else {
            (cs, cl)
        };
        *until = until.min(base + limit);
        let local = p0 - base;
        for n in notes {
            if n.start >= limit {
                continue;
            }
            let end = (n.start + n.len).min(limit);
            if n.start <= local + 1e-9 && end > local + 1e-9 {
                set |= 1u128 << (n.key & 127);
                *until = until.min(base + end);
            } else if n.start > local {
                *until = until.min(base + n.start);
            }
        }
    }
    set
}

/// The keys an effect on track `own` following `src` hears at beat `p0`, written into `out` (sorted); returns how many and the beat
/// until which they hold.
pub fn keys_at(tracks: &[Track], own: usize, src: KeySource, p0: f64, out: &mut [u8; 32]) -> (usize, f64) {
    let mut until = p0 + 4.0;
    let set = match src {
        KeySource::Own => tracks.get(own).map(|t| sounding(t, p0, &mut until)).unwrap_or(0),
        KeySource::Track(k) => tracks.get(k).map(|t| sounding(t, p0, &mut until)).unwrap_or(0),
        KeySource::Auto => {
            // chord parts (two or more notes at once) say the harmony; the lowest note of everything says its root
            let (mut chordal, mut all) = (0u128, 0u128);
            for (i, t) in tracks.iter().enumerate() {
                if i == own || !pitched(t) {
                    continue;
                }
                let s = sounding(t, p0, &mut until);
                if s.count_ones() >= 2 {
                    chordal |= s;
                }
                all |= s;
            }
            let set = if chordal != 0 { chordal | (all & all.wrapping_neg()) } else { all };
            if set != 0 {
                set
            } else {
                tracks.get(own).map(|t| sounding(t, p0, &mut until)).unwrap_or(0)
            }
        }
    };
    let mut n = 0;
    let mut s = set;
    while s != 0 && n < 32 {
        let k = s.trailing_zeros();
        out[n] = k as u8;
        n += 1;
        s &= s - 1;
    }
    (n, until.max(p0 + 1e-6))
}

/// Give every key-following effect the notes it follows at beat `p0` (only while the transport plays; stopped, the effects keep
/// what they last heard). Called by the engine once per slice.
pub fn feed_keys(tracks: &mut [Track], p0: f64, playing: bool) {
    if !playing || !p0.is_finite() {
        return;
    }
    for ti in 0..tracks.len() {
        for ei in 0..tracks[ti].effects.len() {
            let Some(kf) = tracks[ti].effects[ei].fx.key_follow() else { continue };
            if p0 >= kf.valid.0 && p0 < kf.valid.1 {
                continue;
            }
            let mut keys = [0u8; 32];
            let (n, until) = keys_at(tracks, ti, kf.src, p0, &mut keys);
            tracks[ti].effects[ei].fx.set_keys(&keys[..n], p0, until);
        }
    }
}

/// The build spans (start, end in beats) of a project's "sections" meta (`start:end:tag;...`), the tags a song build marks as builds.
pub fn build_spans(sections: &str) -> Vec<(f64, f64)> {
    let mut out = vec![];
    for part in sections.split(';') {
        let f: Vec<&str> = part.split(':').collect();
        if f.len() < 3 {
            continue;
        }
        let (Ok(a), Ok(b)) = (f[0].trim().parse::<f64>(), f[1].trim().parse::<f64>()) else { continue };
        // "build 2" is a build too
        let tag = f[2].trim().trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ').to_lowercase();
        if b > a && crate::songgen::is_build_tag(&tag) {
            out.push((a, b));
        }
    }
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    out
}

/// Is this a riser / sweep track (by its name)?
pub fn is_riser(name: &str) -> bool {
    let n = name.to_lowercase();
    ["riser", "sweep", "uplift", "whoosh", "rise fx"].iter().any(|w| n.contains(w))
}

/// The narrow-then-wide bus move on a riser: through each build (start, end) the width falls from normal (100 %) to mono, on the drop
/// it opens to 150 % and settles back to 100 % over the next bar. Uses the track's Imager (adds one at the end of the chain with its
/// mono-below split off when there is none) and replaces its width lane. Returns the lane's name, None when there is no build.
pub fn riser_narrowing(track: &mut Track, builds: &[(f64, f64)], sr: f32) -> Option<String> {
    if builds.is_empty() {
        return None;
    }
    let idx = match track.effects.iter().position(|e| e.name == "Imager") {
        Some(i) => i,
        None => {
            let mut s = EffectSlot::new("Imager", sr)?;
            if let Some(i) = s.params.iter().position(|p| p.0 == "mono below") {
                s.set(i, 0.0);
            }
            track.effects.push(s);
            track.effects.len() - 1
        }
    };
    let wp = track.effects[idx].params.iter().position(|p| p.0 == "width")?;
    let normal = 0.5f32;
    let mut pts: Vec<(f64, f32)> = vec![];
    for (k, &(a, b)) in builds.iter().enumerate() {
        let next = builds.get(k + 1).map(|x| x.0).unwrap_or(f64::INFINITY);
        if pts.last().map(|p| p.0 >= a).unwrap_or(false) {
            continue;
        }
        pts.push((a, normal));
        pts.push(((b - 0.02).max(a + 0.01), 0.0));
        pts.push((b, 0.75));
        let settle = (b + 4.0).min(next - 0.01);
        if settle > b + 0.05 {
            pts.push((settle, normal));
        }
    }
    let name = format!("fx{idx}.{wp}");
    track.auto.retain(|l| l.param != name);
    track.auto.push(AutoLane { param: name.clone(), points: pts });
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, Note, TrackKind};

    fn track(name: &str, notes: &[(f64, f64, u8)]) -> Track {
        let mut t = Track::new(name, TrackKind::Instrument, 0, crate::dsp::make_instrument("synth", 48000.0));
        t.inst_kind = "synth".into();
        let mut c = Clip::midi(0.0, 16.0, "c");
        c.data = ClipData::Midi(notes.iter().map(|(s, l, k)| Note::new(*s, *l, *k, 0.8)).collect());
        t.clips.push(c);
        t
    }

    #[test]
    fn auto_follows_the_chord_parts_and_the_bass_root() {
        // chords: Am (beats 0-4) then F (4-8); a lead melody; a bass; the growl's own track
        let chords = track("Chords", &[(0.0, 4.0, 57), (0.0, 4.0, 60), (0.0, 4.0, 64), (4.0, 4.0, 53), (4.0, 4.0, 57), (4.0, 4.0, 60)]);
        let lead = track("Lead", &[(0.0, 1.0, 76), (1.0, 1.0, 74), (4.0, 2.0, 72)]);
        let bass = track("Bass", &[(0.0, 4.0, 33), (4.0, 4.0, 29)]);
        let growl = track("Growl", &[(0.0, 8.0, 45)]);
        let tracks = vec![chords, lead, bass, growl];
        let mut k = [0u8; 32];
        let (n, until) = keys_at(&tracks, 3, KeySource::Auto, 0.5, &mut k);
        assert_eq!(&k[..n], &[33, 57, 60, 64], "the chord and the bass, not the lead");
        assert!((until - 1.0).abs() < 1e-9, "valid until the lead's next note: {until}");
        let (n, _) = keys_at(&tracks, 3, KeySource::Auto, 5.0, &mut k);
        assert_eq!(&k[..n], &[29, 53, 57, 60]);
        let (n, _) = keys_at(&tracks, 3, KeySource::Own, 5.0, &mut k);
        assert_eq!(&k[..n], &[45]);
        let (n, _) = keys_at(&tracks, 3, KeySource::Track(2), 5.0, &mut k);
        assert_eq!(&k[..n], &[29]);
        // nothing else plays: its own notes
        let (n, _) = keys_at(&tracks[3..], 0, KeySource::Auto, 1.0, &mut k);
        assert_eq!(&k[..n], &[45]);
    }

    #[test]
    fn the_engine_feeds_a_colour_bass_the_chord() {
        let chords = track("Chords", &[(0.0, 4.0, 57), (0.0, 4.0, 60), (0.0, 4.0, 64), (4.0, 4.0, 53), (4.0, 4.0, 57), (4.0, 4.0, 60)]);
        let mut growl = track("Growl", &[(0.0, 8.0, 45)]);
        growl.effects.push(EffectSlot::new("Colour Bass", 48000.0).unwrap());
        let mut tracks = vec![chords, growl];
        feed_keys(&mut tracks, 0.0, true);
        let mut b = vec![0.0f32; 128];
        tracks[1].effects[0].fx.process(&mut b);
        assert_eq!(tracks[1].effects[0].fx.info("chord"), "C E A");
        feed_keys(&mut tracks, 4.0, true);
        tracks[1].effects[0].fx.process(&mut b);
        assert_eq!(tracks[1].effects[0].fx.info("chord"), "C F A");
        // a Comb that tracks its own notes, an octave up
        let mut t = track("Pluck", &[(0.0, 2.0, 50)]);
        let mut c = EffectSlot::new("Comb", 48000.0).unwrap();
        let ki = c.params.iter().position(|p| p.0 == "keytrack").unwrap();
        c.set(ki, 0.3);
        t.effects.push(c);
        let mut v = vec![t];
        feed_keys(&mut v, 0.5, true);
        assert_eq!(v[0].effects[0].fx.key_follow().unwrap().src, KeySource::Own);
        let kf = v[0].effects[0].fx.key_follow().unwrap();
        assert!((kf.valid.1 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn risers_narrow_through_the_build_and_open_on_the_drop() {
        let spans = build_spans("0:32:intro;32:48:build;48:80:drop;80:96:break;96:112:build 2;112:144:drop");
        assert_eq!(spans.len(), 2, "{spans:?}");
        let mut t = track("Riser", &[(32.0, 16.0, 60)]);
        assert!(is_riser(&t.name) && !is_riser("Bass"));
        let lane = riser_narrowing(&mut t, &spans, 48000.0).unwrap();
        assert_eq!(t.effects[0].name, "Imager");
        let l = t.auto.iter().find(|l| l.param == lane).unwrap();
        assert!((l.value_at(32.0).unwrap() - 0.5).abs() < 1e-6);
        assert!(l.value_at(40.0).unwrap() < 0.3, "narrower halfway");
        assert!(l.value_at(47.97).unwrap() < 0.02, "mono at the end of the build");
        assert!((l.value_at(48.0).unwrap() - 0.75).abs() < 1e-6, "wide on the drop");
        assert!((l.value_at(60.0).unwrap() - 0.5).abs() < 1e-6, "back to normal");
        assert!(l.points.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
