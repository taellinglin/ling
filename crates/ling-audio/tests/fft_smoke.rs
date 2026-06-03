//! ling-audio smoke tests: the FFT analyzer accepts samples and reports
//! sane RMS / band data.

use ling_audio::FftAnalyzer;

#[test]
fn analyzer_constructs_and_handles_silence() {
    let mut fft = FftAnalyzer::new(1024, 44_100);
    let silence = vec![0.0f32; 1024];
    fft.push_samples(&silence);
    assert!(fft.rms() < 1e-3, "silence has ~zero RMS");
}

#[test]
fn rms_rises_with_signal() {
    let mut fft = FftAnalyzer::new(1024, 44_100);
    // A full-scale sine wave at 1 kHz.
    let sr = 44_100.0f32;
    let freq = 1_000.0f32;
    let buf: Vec<f32> = (0..1024)
        .map(|i| (std::f32::consts::TAU * freq * i as f32 / sr).sin())
        .collect();
    fft.push_samples(&buf);
    assert!(fft.rms() > 0.1, "a loud sine should have substantial RMS");
}

#[test]
fn freq_bands_have_requested_length() {
    let mut fft = FftAnalyzer::new(1024, 44_100);
    fft.push_samples(&vec![0.0f32; 1024]);
    let bands = fft.freq_bands(8);
    assert_eq!(bands.len(), 8, "freq_bands(n) returns n bands");
    assert!(bands.iter().all(|&b| b.is_finite()), "bands are finite");
}
