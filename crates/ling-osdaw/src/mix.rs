//! Mix AI at kernel scale: measure each channel soloed and set volume and pan so the mix sits right (the LingStationX "AI Mix & Master" rules
//! without the language model: roles come from the instrument).
use crate::{Engine, Song, CHANNELS, NOTE_OFF, SAMPLE_RATE};
use alloc::vec;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Kick,
    Snare,
    Hat,
    Bass,
    Lead,
    Pad,
    Keys,
    Silent,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Kick => "kick",
            Role::Snare => "snare",
            Role::Hat => "hat",
            Role::Bass => "bass",
            Role::Lead => "lead",
            Role::Pad => "pad",
            Role::Keys => "keys",
            Role::Silent => "empty",
        }
    }
    /// target loudness (RMS of the soloed channel while it sounds), a linear amplitude
    fn target(self) -> f32 {
        match self {
            Role::Kick => 0.20,
            Role::Bass => 0.17,
            Role::Snare => 0.13,
            Role::Lead => 0.11,
            Role::Keys => 0.09,
            Role::Pad => 0.07,
            Role::Hat => 0.045,
            Role::Silent => 0.0,
        }
    }
    fn pan(self) -> i8 {
        match self {
            Role::Hat => 25,
            Role::Pad => -20,
            Role::Keys => 30,
            _ => 0,
        }
    }
}

pub fn role_of(inst: u8, has_notes: bool) -> Role {
    if !has_notes {
        return Role::Silent;
    }
    match inst {
        5 => Role::Kick,
        6 | 12 | 13 => Role::Snare,
        7 => Role::Hat,
        1 | 9 => Role::Bass,
        0 | 8 | 10 => Role::Lead,
        3 => Role::Pad,
        _ => Role::Keys,
    }
}

pub struct Report {
    pub roles: [Role; CHANNELS],
    /// the mix's peak after balancing, 0..1
    pub peak: f32,
}

fn render_stats(song: &Song) -> (f32, f32) {
    let mut e = Engine::new(song.clone());
    e.master = 1.0;
    e.looping = false;
    e.play();
    let frames = ((e.song.seconds() * SAMPLE_RATE as f32) as usize).min(SAMPLE_RATE as usize * 24);
    let mut buf = vec![0i16; 2048];
    let (mut sum, mut cnt, mut peak) = (0.0f64, 0u64, 0.0f32);
    let mut done = 0;
    while done < frames {
        let n = (frames - done).min(1024);
        e.render(&mut buf[..n * 2]);
        for f in 0..n {
            let v = buf[f * 2] as f32 / 32768.0;
            peak = peak.max(v.abs()).max((buf[f * 2 + 1] as f32 / 32768.0).abs());
            // only the moments the channel sounds count: silence between hits must not drag the average down
            if v.abs() > 0.004 {
                sum += (v * v) as f64;
                cnt += 1;
            }
        }
        done += n;
    }
    (if cnt == 0 { 0.0 } else { libm::sqrt(sum / cnt as f64) as f32 }, peak)
}

fn solo_rms(song: &Song, ch: usize) -> f32 {
    let mut s = song.clone();
    s.fx = [0; 4];
    for (i, c) in s.chans.iter_mut().enumerate() {
        c.mute = i != ch;
        c.vol = 100;
        c.pan = 0;
    }
    render_stats(&s).0
}

/// Set every channel's volume and pan from its measured level and role. Returns what it decided.
pub fn auto_mix(song: &mut Song) -> Report {
    let mut roles = [Role::Silent; CHANNELS];
    let mut gain = [0.0f32; CHANNELS];
    for c in 0..CHANNELS {
        let has = (0..song.length as usize).any(|r| song.cells[r][c].note > 0 && song.cells[r][c].note != NOTE_OFF);
        roles[c] = role_of(song.chans[c].inst, has);
        if roles[c] == Role::Silent {
            continue;
        }
        let rms = solo_rms(song, c);
        gain[c] = if rms > 1e-5 { roles[c].target() / rms } else { 0.0 };
    }
    // the channel that wants the most gets full volume, the others keep their ratio to it
    let top = gain.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    for c in 0..CHANNELS {
        if roles[c] == Role::Silent {
            continue;
        }
        song.chans[c].vol = ((gain[c] / top) * 100.0).clamp(8.0, 100.0) as u8;
        let p = roles[c].pan();
        // a second channel of the same role fans out to the other side
        let twin = (0..c).any(|o| roles[o] == roles[c]);
        song.chans[c].pan = if twin { -p.abs().max(25) } else { p };
    }
    Report { roles, peak: render_stats(song).1 }
}
