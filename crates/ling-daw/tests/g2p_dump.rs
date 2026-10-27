//! The DAW's own lyric -> IPA (lyricfit::syllabify_line, what sung notes get) for a file of lines, so the Ling Singer's training
//! labels use exactly the phonemes the DAW will ask it to sing. Input `LYR_IN`: "name<TAB>text" per line; output `LYR_OUT`:
//! "name<TAB>ipa syllables separated by spaces". Run: LYR_IN=a.tsv LYR_OUT=b.tsv cargo test -p ling-daw --test g2p_dump -- --ignored
use ling_daw::lyricfit::syllabify_line;
use ling_daw::lyrics::Lang;

#[test]
#[ignore]
fn dump() {
    let (Ok(inp), Ok(out)) = (std::env::var("LYR_IN"), std::env::var("LYR_OUT")) else { return };
    let text = std::fs::read_to_string(inp).unwrap();
    let mut lines = vec![];
    for l in text.lines() {
        let Some((name, words)) = l.split_once('\t') else { continue };
        let syl: Vec<String> = syllabify_line(words, Lang::Auto).into_iter().filter(|s| !s.ipa.is_empty()).map(|s| s.ipa).collect();
        lines.push(format!("{name}\t{}", syl.join(" ")));
    }
    std::fs::write(out, lines.join("\n")).unwrap();
}
