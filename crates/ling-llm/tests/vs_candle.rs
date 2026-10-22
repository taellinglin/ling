//! The engine against the real thing: the same GGUF through candle's quantized Qwen3 and the HF tokenizer.
//!   cargo test -p ling-llm --features std --release --test vs_candle -- --nocapture
//! LING_MODEL / LING_TOKENIZER override the paths. Without the model files the tests do nothing (they print why).
#![cfg(feature = "std")]
use ling_llm::chat::build_prompt;
use ling_llm::gguf::{Gguf, Source};
use ling_llm::{Error, Llm};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;

struct FileSrc {
    f: Mutex<File>,
    len: u64,
}
impl FileSrc {
    fn open(path: &str) -> Option<FileSrc> {
        let f = File::open(path).ok()?;
        let len = f.metadata().ok()?.len();
        Some(FileSrc { f: Mutex::new(f), len })
    }
}
impl Source for FileSrc {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let mut f = self.f.lock().unwrap();
        f.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        f.read_exact(buf).map_err(|_| Error::Io)
    }
}

fn model_path() -> String {
    std::env::var("LING_MODEL").unwrap_or_else(|_| r"C:\Users\User\Programs\LingStationX\assets\models\Qwen3-0.6B-Q8_0.gguf".into())
}
fn tokenizer_path() -> String {
    std::env::var("LING_TOKENIZER").unwrap_or_else(|_| r"C:\Users\User\Programs\LingStationX\assets\models\tokenizer.json".into())
}

#[test]
fn the_gguf_header_and_config_read() {
    let Some(src) = FileSrc::open(&model_path()) else { return println!("no model at {}: skipped", model_path()) };
    let g = Gguf::parse(&src).unwrap();
    assert_eq!(g.get_str("general.architecture"), Some("qwen3"));
    let cfg = ling_llm::model::Config::from_gguf(&g).unwrap();
    println!("{}", ling_llm::model::describe(&cfg));
    assert!(cfg.n_layer > 0 && cfg.vocab > 100_000);
    assert!(g.tensor("token_embd.weight").is_some());
}

#[test]
fn the_tokenizer_matches_the_hf_tokenizer() {
    let (Some(src), true) = (FileSrc::open(&model_path()), std::path::Path::new(&tokenizer_path()).exists()) else { return println!("no model or tokenizer.json: skipped") };
    let g = Gguf::parse(&src).unwrap();
    let ours = ling_llm::Tokenizer::from_gguf(&g).unwrap();
    let hf = tokenizers::Tokenizer::from_file(tokenizer_path()).unwrap();
    let samples = [
        "Hello, world!",
        "The song is at 128 BPM in C minor with 8 parts.",
        "  leading spaces and trailing  ",
        "line one\nline two\n\n\nline four",
        "don't I'll we've THEY'RE",
        "a1234567b 3.14159",
        "fn main() { println!(\"{}\", x + 1); }",
        "naïve café 你好,世界! こんにちは",
        "emoji 🎵 and symbols ♯♭ ##",
        "<|im_start|>user\nName three colours.<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
        "{\"style\": \"house\", \"parts\": [{\"part\": 0, \"role\": \"drums\"}]}",
        "tabs\tand\r\nwindows lines",
        "",
        "x",
    ];
    for s in samples {
        let a = ours.encode(s);
        let b: Vec<u32> = hf.encode(s, false).unwrap().get_ids().to_vec();
        assert_eq!(a, b, "tokenization differs for {s:?}");
        assert_eq!(ours.decode(&a), s, "decode does not round-trip {s:?}");
    }
    println!("tokenizer: {} samples match", samples.len());
}

#[test]
fn logits_and_greedy_text_match_candle() {
    use candle_core::quantized::gguf_file;
    use candle_core::{Device, Tensor};
    use candle_transformers::models::quantized_qwen3;
    let Some(src) = FileSrc::open(&model_path()) else { return println!("no model at {}: skipped", model_path()) };
    let t0 = std::time::Instant::now();
    let llm = Llm::load(&src, &mut |_, _| {}).unwrap();
    println!("loaded {} MB in {:.1}s", llm.model.weight_bytes() >> 20, t0.elapsed().as_secs_f32());
    let prompt = build_prompt("You are terse.", "<|user|>\nName three colours.\n");
    let ids = llm.tok.encode(&prompt);
    println!("{} prompt tokens", ids.len());

    // ours: prefill, then ten greedy tokens
    let mut st = llm.model.new_state(256);
    let t1 = std::time::Instant::now();
    llm.prefill(&mut st, &ids, &mut |_, _| {}).unwrap();
    let first_logits = st.logits.clone();
    let mut our_tokens = vec![];
    let argmax = |l: &[f32]| l.iter().enumerate().fold((0usize, f32::NEG_INFINITY), |a, (i, v)| if *v > a.1 { (i, *v) } else { a }).0 as u32;
    let mut next = argmax(&st.logits);
    for _ in 0..10 {
        our_tokens.push(next);
        llm.model.forward(&mut st, &[next], true).unwrap();
        next = argmax(&st.logits);
    }
    println!("ours: {:.2}s -> {:?}", t1.elapsed().as_secs_f32(), llm.tok.decode(&our_tokens));

    // candle
    let device = Device::Cpu;
    let mut file = File::open(model_path()).unwrap();
    let content = gguf_file::Content::read(&mut file).unwrap();
    let mut w = quantized_qwen3::ModelWeights::from_gguf(content, &mut file, &device).unwrap();
    let input = Tensor::new(ids.as_slice(), &device).unwrap().unsqueeze(0).unwrap();
    let logits = w.forward(&input, 0).unwrap().squeeze(0).unwrap().to_vec1::<f32>().unwrap();
    let mut cand_tokens = vec![];
    let mut cnext = argmax(&logits);
    let mut pos = ids.len();
    for _ in 0..10 {
        cand_tokens.push(cnext);
        let inp = Tensor::new(&[cnext], &device).unwrap().unsqueeze(0).unwrap();
        let l = w.forward(&inp, pos).unwrap().squeeze(0).unwrap().to_vec1::<f32>().unwrap();
        pos += 1;
        cnext = argmax(&l);
    }
    println!("candle: {:?}", llm.tok.decode(&cand_tokens));

    // the first logits agree closely...
    let dot: f64 = first_logits.iter().zip(&logits).map(|(a, b)| *a as f64 * *b as f64).sum();
    let (na, nb): (f64, f64) = (first_logits.iter().map(|a| (*a as f64).powi(2)).sum::<f64>().sqrt(), logits.iter().map(|a| (*a as f64).powi(2)).sum::<f64>().sqrt());
    let cosine = dot / (na * nb);
    println!("cosine of the first logits: {cosine:.6}");
    // the cache holds halves and candle does not: a few parts in a thousand of difference is expected
    assert!(cosine > 0.995, "logits differ: cosine {cosine}");
    assert_eq!(argmax(&first_logits), argmax(&logits), "the first token differs");
    // ...and so does the text
    let same = our_tokens.iter().zip(&cand_tokens).take_while(|(a, b)| a == b).count();
    println!("{same} of 10 greedy tokens identical");
    assert!(same >= 8, "greedy text diverges after {same} tokens: {our_tokens:?} vs {cand_tokens:?}");
}
