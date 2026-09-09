use ling_mic::{MicConfig, MicInput};
use std::time::Duration;

fn main() {
    match MicInput::open(MicConfig::default()) {
        Ok(mic) => {
            println!("opened OK");
            match mic.start(|_s| {}) {
                Ok(()) => println!("stream started, sample_rate={}", mic.sample_rate()),
                Err(e) => {
                    println!("start() failed: {e:?}");
                    return;
                },
            }
            for i in 0..20 {
                std::thread::sleep(Duration::from_millis(150));
                println!(
                    "t={:>4}ms rms={:.5} peak={:.5} samples={}",
                    i * 150,
                    mic.rms(),
                    mic.peak(),
                    mic.latest_samples().len()
                );
            }
        },
        Err(e) => println!("open() failed: {e:?}"),
    }
}
