//! Real instruments and the ensembles they play in, for File > New AI Track: from a full orchestra (each section in its own
//! register) to a garage band, a jazz combo, a string quartet or an exotic lute duet.
//!
//! An [`Inst`] is a part with its job (melody, chords, bass, pad, arpeggio, drums), the register the real instrument plays in
//! (written pitch, MIDI) and the General MIDI sound that plays it (instruments/GM/NNN *.ling). An [`Ensemble`] is the set of parts
//! a group has, found from words in the description ("garage band", "string quartet", "oud and lute duet") and the genre it
//! implies. songgen uses both: an ensemble replaces the genre's default roles, and any instrument named in the description is
//! added as a part of its own.
use crate::compose::Kind;

pub struct Inst {
    pub role: &'static str,
    pub kind: Kind,
    pub lo: u8,
    pub hi: u8,
    /// General MIDI program, 1-based (the number the GM patch files start with); 0 = a drum kit
    pub gm: u8,
    /// words in a description that name it
    pub words: &'static [&'static str],
}

const fn i(role: &'static str, kind: Kind, lo: u8, hi: u8, gm: u8, words: &'static [&'static str]) -> Inst {
    Inst { role, kind, lo, hi, gm, words }
}

pub const INSTRUMENTS: &[Inst] = &[
    // orchestra: strings by section, winds, brass, percussion
    i("violins", Kind::Melody, 55, 91, 49, &["violins", "first violins"]),
    i("second violins", Kind::Arp, 55, 84, 49, &["second violins"]),
    i("violas", Kind::Chords, 48, 76, 42, &["violas"]),
    i("cellos", Kind::Pad, 36, 67, 43, &["cellos", "celli"]),
    i("contrabasses", Kind::Bass, 28, 52, 44, &["contrabasses", "double basses", "basses"]),
    i("violin", Kind::Melody, 55, 91, 41, &["violin", "solo violin"]),
    i("second violin", Kind::Arp, 55, 84, 41, &["second violin"]),
    i("viola", Kind::Chords, 48, 76, 42, &["viola"]),
    i("cello", Kind::Bass, 36, 67, 43, &["cello"]),
    i("flute", Kind::Melody, 60, 93, 74, &["flute", "flutes", "dizi"]),
    i("piccolo", Kind::Melody, 74, 103, 73, &["piccolo"]),
    i("oboe", Kind::Melody, 58, 86, 69, &["oboe", "oboes"]),
    i("clarinet", Kind::Melody, 50, 84, 72, &["clarinet", "clarinets"]),
    i("bassoon", Kind::Bass, 34, 65, 71, &["bassoon", "bassoons"]),
    i("recorder", Kind::Melody, 60, 91, 75, &["recorder"]),
    i("french horn", Kind::Pad, 41, 72, 61, &["french horn", "french horns", "horns", "horn section"]),
    i("trumpet", Kind::Melody, 55, 82, 57, &["trumpet", "trumpets"]),
    i("trombone", Kind::Chords, 40, 70, 58, &["trombone", "trombones"]),
    i("tuba", Kind::Bass, 28, 55, 59, &["tuba"]),
    i("timpani", Kind::Bass, 40, 55, 48, &["timpani", "kettle drums"]),
    i("harp", Kind::Arp, 36, 91, 47, &["harp", "qanun", "kanun"]),
    i("celesta", Kind::Arp, 60, 96, 9, &["celesta"]),
    i("glockenspiel", Kind::Arp, 72, 96, 10, &["glockenspiel"]),
    i("tubular bells", Kind::Arp, 60, 77, 15, &["tubular bells", "chimes"]),
    i("harpsichord", Kind::Arp, 41, 84, 7, &["harpsichord"]),
    i("pipe organ", Kind::Pad, 36, 84, 20, &["pipe organ", "church organ"]),
    // band
    i("lead guitar", Kind::Melody, 52, 86, 31, &["lead guitar", "guitar solo", "distorted guitar"]),
    i("rhythm guitar", Kind::Chords, 40, 72, 30, &["rhythm guitar", "power chords", "crunch guitar"]),
    i("clean guitar", Kind::Arp, 45, 79, 28, &["clean guitar", "electric guitar"]),
    i("acoustic guitar", Kind::Chords, 40, 76, 26, &["acoustic guitar", "steel string"]),
    i("bass guitar", Kind::Bass, 28, 52, 34, &["bass guitar", "electric bass", "finger bass"]),
    i("rock organ", Kind::Pad, 48, 79, 19, &["rock organ", "hammond"]),
    i("upright bass", Kind::Bass, 28, 55, 33, &["upright bass", "double bass", "acoustic bass", "walking bass"]),
    i("jazz guitar", Kind::Chords, 45, 76, 27, &["jazz guitar", "archtop"]),
    i("tenor sax", Kind::Melody, 44, 75, 67, &["tenor sax", "tenor saxophone", "saxophone", "sax"]),
    i("alto sax", Kind::Melody, 49, 81, 66, &["alto sax", "alto saxophone"]),
    i("vibraphone", Kind::Arp, 53, 89, 12, &["vibraphone", "vibes"]),
    i("accordion", Kind::Chords, 48, 79, 22, &["accordion", "bandoneon"]),
    i("banjo", Kind::Arp, 48, 84, 106, &["banjo"]),
    i("mandolin", Kind::Arp, 55, 88, 26, &["mandolin", "bouzouki"]),
    // the world's lutes, fiddles, flutes and drums
    i("lute", Kind::Arp, 40, 76, 25, &["lute", "nylon guitar", "classical guitar"]),
    i("oud", Kind::Melody, 45, 77, 107, &["oud", "saz", "baglama"]),
    i("sitar", Kind::Melody, 52, 84, 105, &["sitar"]),
    i("tanpura", Kind::Pad, 41, 60, 105, &["tanpura", "tambura", "drone"]),
    i("koto", Kind::Arp, 50, 88, 108, &["koto", "guzheng", "zither"]),
    i("shamisen", Kind::Melody, 50, 84, 107, &["shamisen", "pipa"]),
    i("shakuhachi", Kind::Melody, 55, 84, 78, &["shakuhachi", "ney", "bansuri"]),
    i("erhu", Kind::Melody, 55, 86, 111, &["erhu"]),
    i("fiddle", Kind::Melody, 55, 88, 111, &["fiddle", "folk violin"]),
    i("tin whistle", Kind::Melody, 72, 96, 79, &["tin whistle", "whistle", "penny whistle"]),
    i("bagpipe", Kind::Pad, 55, 76, 110, &["bagpipe", "bagpipes"]),
    i("kalimba", Kind::Arp, 60, 84, 109, &["kalimba", "mbira"]),
    i("marimba", Kind::Arp, 48, 84, 13, &["marimba", "balafon", "gamelan"]),
    i("steel drums", Kind::Arp, 55, 84, 115, &["steel drums", "steelpan"]),
    i("hand drums", Kind::Drums, 0, 127, 0, &["hand drums", "tabla", "darbuka", "doumbek", "frame drum", "bodhran", "djembe", "congas", "taiko", "percussion"]),
];

pub struct Ensemble {
    pub name: &'static str,
    pub words: &'static [&'static str],
    pub parts: &'static [&'static str],
    /// the genre it plays in unless the description names one
    pub genre: &'static str,
}

const fn e(name: &'static str, words: &'static [&'static str], parts: &'static [&'static str], genre: &'static str) -> Ensemble {
    Ensemble { name, words, parts, genre }
}

/// most specific first: "string quartet" before "orchestra" before a single instrument
pub const ENSEMBLES: &[Ensemble] = &[
    e("string quartet", &["string quartet", "quartet of strings"], &["violin", "second violin", "viola", "cello"], "orchestral"),
    e("woodwind quintet", &["woodwind quintet", "wind quintet", "woodwinds"], &["flute", "oboe", "clarinet", "french horn", "bassoon"], "orchestral"),
    e("brass quintet", &["brass quintet", "brass band", "brass ensemble"], &["trumpet", "french horn", "trombone", "tuba"], "orchestral"),
    e("baroque ensemble", &["baroque", "chamber"], &["harpsichord", "violin", "recorder", "cello"], "orchestral"),
    e("orchestra", &["full orchestra", "symphony orchestra", "orchestra", "orchestral", "symphony", "symphonic", "philharmonic"], &["violins", "second violins", "violas", "cellos", "contrabasses", "flute", "french horn", "trumpet", "timpani"], "orchestral"),
    e("big band", &["big band", "swing band"], &["drums", "upright bass", "piano", "trumpet", "trombone", "alto sax", "tenor sax"], "jazz"),
    e("jazz combo", &["jazz combo", "jazz trio", "jazz quartet", "jazz band", "combo"], &["drums", "upright bass", "piano", "tenor sax", "jazz guitar"], "jazz"),
    e("garage band", &["garage band", "garage rock", "rock band", "punk band", "indie band", "power trio"], &["drums", "bass guitar", "rhythm guitar", "lead guitar", "rock organ"], "rock"),
    e("bluegrass band", &["bluegrass", "country band"], &["banjo", "fiddle", "mandolin", "acoustic guitar", "upright bass"], "folk"),
    e("celtic band", &["celtic", "irish", "scottish", "jig", "reel"], &["fiddle", "tin whistle", "harp", "acoustic guitar", "hand drums"], "folk"),
    e("tango orchestra", &["tango"], &["accordion", "violin", "piano", "upright bass"], "folk"),
    e("lute duet", &["exotic lute duet", "lute duet", "oud and lute", "lute and oud", "oud duet"], &["oud", "lute"], "folk"),
    e("middle eastern ensemble", &["middle eastern", "arabic", "persian", "turkish", "maqam"], &["oud", "harp", "shakuhachi", "hand drums"], "folk"),
    e("indian ensemble", &["indian classical", "raga", "hindustani", "carnatic"], &["sitar", "tanpura", "shakuhachi", "hand drums"], "folk"),
    e("japanese ensemble", &["japanese traditional", "japanese folk", "edo", "gagaku"], &["koto", "shakuhachi", "shamisen", "hand drums"], "folk"),
    e("chinese ensemble", &["chinese traditional", "guoyue", "silk and bamboo"], &["erhu", "shamisen", "koto", "flute", "hand drums"], "folk"),
    e("gamelan", &["gamelan", "balinese", "javanese"], &["marimba", "vibraphone", "glockenspiel", "tubular bells", "hand drums"], "ambient"),
    e("caribbean band", &["calypso", "steel band", "soca"], &["steel drums", "bass guitar", "acoustic guitar", "hand drums"], "afrobeat"),
    e("solo piano", &["solo piano", "piano solo", "piano only"], &["piano"], "pop"),
    e("acoustic duo", &["acoustic duo", "unplugged", "singer-songwriter"], &["acoustic guitar", "piano"], "folk"),
];

pub fn instrument(role: &str) -> Option<&'static Inst> {
    let r = role.trim().to_lowercase();
    INSTRUMENTS.iter().find(|i| i.role == r)
}

fn contains_word(text: &str, w: &str) -> bool {
    let t = format!(" {} ", text.to_lowercase().replace([',', '.', ';', ':', '!', '?', '(', ')', '/'], " "));
    t.contains(&format!(" {w} "))
}

/// the ensemble a description names, the most specific match first
pub fn ensemble_in(text: &str) -> Option<&'static Ensemble> {
    ENSEMBLES.iter().find(|e| e.words.iter().any(|w| contains_word(text, w)))
}

/// instruments a description names one by one ("with a cello and a sitar"), the longest name first so "lead guitar" wins over
/// "guitar"
pub fn instruments_in(text: &str) -> Vec<&'static str> {
    let mut hits: Vec<(&'static str, usize)> = vec![];
    for inst in INSTRUMENTS {
        for w in inst.words {
            if contains_word(text, w) && !hits.iter().any(|(r, _)| *r == inst.role) {
                hits.push((inst.role, w.len()));
            }
        }
    }
    hits.sort_by(|a, b| b.1.cmp(&a.1));
    hits.into_iter().map(|(r, _)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_find_their_ensembles_and_instruments() {
        assert_eq!(ensemble_in("an exotic lute duet at dusk").map(|e| e.name), Some("lute duet"));
        assert_eq!(ensemble_in("loud garage band with fuzz").map(|e| e.name), Some("garage band"));
        assert_eq!(ensemble_in("epic full orchestra").map(|e| e.name), Some("orchestra"));
        assert_eq!(ensemble_in("a string quartet in the rain").map(|e| e.name), Some("string quartet"));
        assert!(ensemble_in("uk garage at 132 bpm").is_none(), "UK garage is a genre, not a garage band");
        let found = instruments_in("lofi beat with a cello, sitar and lead guitar");
        assert!(found.contains(&"cello") && found.contains(&"sitar") && found.contains(&"lead guitar"), "{found:?}");
        // every ensemble's parts are instruments the tables know (or the generic drums / piano roles)
        for e in ENSEMBLES {
            for p in e.parts {
                assert!(instrument(p).is_some() || ["drums", "piano"].contains(p), "{} {p}", e.name);
            }
        }
        for inst in INSTRUMENTS {
            assert!(inst.kind == Kind::Drums || (inst.lo + 12 <= inst.hi && inst.gm >= 1 && inst.gm <= 128), "{}", inst.role);
        }
    }
}
