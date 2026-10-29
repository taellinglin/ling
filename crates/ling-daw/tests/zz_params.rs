use ling_daw::dsp::make_instrument;
#[test]
fn dump() {
    for k in ["synth","growl","prism","pluck","chip","sampler","voice","drums"] {
        let i = make_instrument(k, 44100.0).unwrap();
        let names: Vec<String> = i.params().into_iter().map(|(n,_)| n.to_string()).collect();
        println!("## {k}: {}", names.join("|"));
    }
}
