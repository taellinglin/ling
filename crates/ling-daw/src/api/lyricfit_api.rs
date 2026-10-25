//! Builtins for fitting pasted lyrics to a clip's notes (see `lyricfit.rs`): `daw_lyrf_*`.
use super::{Daw, A, Val};
use crate::lyricfit::{self, NoteIn, Opts};
use crate::model::*;

fn info_list(i: &lyricfit::Info) -> Val {
    Val::List([i.syllables, i.lines, i.groups, i.phrases, i.chords, i.bearing].iter().map(|x| Val::Num(*x as f64)).collect())
}

impl Daw {
    pub(super) fn lyrf_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_lyrf_prepare" => {
                // (raw text) -> the cleaned lyrics with every word split into syllables by hyphens, lines joined with " / "
                ok!(lyricfit::prepare(&a.s(0)))
            }
            "daw_lyrf_system" => ok!(lyricfit::qwen_system()),
            "daw_lyrf_prompt" => ok!(lyricfit::qwen_prompt(&a.s(0))),
            "daw_lyrf_accept" => {
                // (raw, model reply) -> the hyphenated lyrics: the model's splitting where it kept every word, the rules' otherwise
                ok!(lyricfit::accept_split(&a.s(0), &a.s(1)))
            }
            "daw_lyrf_info" | "daw_lyrf_apply" => {
                // (t, c, raw lyrics, options "fit:mode:loop:short:chord:gap:lang") -> [notes sung, syllables, lines, note groups, phrases, chords, groups that got a syllable]
                // `info` only measures; `apply` stores a lyric on every note and remembers the text and the options on the clip
                let apply = name == "daw_lyrf_apply";
                let raw = a.s(2);
                let optstr = a.s(3);
                let opts = Opts::parse(&optstr).unwrap_or_default();
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(clip) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) else { return ok!(Val::List(vec![])) };
                let ClipData::Midi(notes) = &mut clip.data else { return ok!(Val::List(vec![])) };
                // "shape": the sung line is first reshaped to the words (a note per syllable, the last word held, a breath after it)
                if opts.shape && apply && !notes.is_empty() {
                    let input: Vec<NoteIn> = notes.iter().map(|n| NoteIn { start: n.start, len: n.len, key: n.key }).collect();
                    let shaped = lyricfit::shape(&input, &raw, &opts);
                    let old = notes.clone();
                    *notes = shaped
                        .iter()
                        .map(|s| {
                            // velocity and expression from the note that was sounding there
                            let src = old.iter().filter(|n| n.start <= s.start + 1e-6).last().or(old.first()).cloned().unwrap_or_else(|| Note::new(0.0, 1.0, s.key, 0.8));
                            Note { start: s.start, len: s.len, key: s.key, lyric: String::new(), ..src }
                        })
                        .collect();
                }
                let input: Vec<NoteIn> = notes.iter().map(|n| NoteIn { start: n.start, len: n.len, key: n.key }).collect();
                let (lyrics, info) = lyricfit::fit(&input, &raw, &opts);
                let given = lyrics.iter().filter(|l| !l.is_empty()).count();
                if apply {
                    for (n, l) in notes.iter_mut().zip(lyrics) {
                        n.lyric = l;
                    }
                    clip.lyric_text = raw;
                    clip.lyric_lang = if optstr.starts_with("fit") { optstr } else { "fit".to_string() };
                    drop(en);
                    self.touch();
                }
                let mut v = vec![Val::Num(given as f64)];
                if let Val::List(l) = info_list(&info) {
                    v.extend(l);
                }
                ok!(Val::List(v))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
