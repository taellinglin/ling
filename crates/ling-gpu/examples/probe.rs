// Probe: report the active backend and cross-check GPU vs CPU correctness.
// Run on a CUDA machine with:  cargo run -p ling-gpu --features cuda --example probe

fn main() {
    let b = ling_gpu::backend();
    println!("active backend: {} (gpu={})", b.name(), b.is_gpu());

    // N-body on a small random-ish cloud.
    let n = 256usize;
    let mut pos = Vec::with_capacity(3 * n);
    let mut mass = Vec::with_capacity(n);
    for i in 0..n {
        let f = i as f32;
        pos.push((f * 0.137).sin() * 5.0);
        pos.push((f * 0.071).cos() * 5.0);
        pos.push((f * 0.029).sin() * 5.0);
        mass.push(1.0 + (f * 0.01).fract());
    }
    let mut got = vec![0.0f32; 3 * n];
    b.nbody_accel(&pos, &mass, 0.5, 0.05, &mut got);

    // Reference CPU result (force a fresh CPU backend via the public trait).
    // We compare against an inline CPU computation to validate the active backend.
    let eps2 = 0.05f32 * 0.05;
    let mut max_err = 0.0f32;
    for i in 0..n {
        let (xi, yi, zi) = (pos[3 * i], pos[3 * i + 1], pos[3 * i + 2]);
        let (mut ax, mut ay, mut az) = (0.0f32, 0.0, 0.0);
        for j in 0..n {
            if j == i {
                continue;
            }
            let dx = pos[3 * j] - xi;
            let dy = pos[3 * j + 1] - yi;
            let dz = pos[3 * j + 2] - zi;
            let r2 = dx * dx + dy * dy + dz * dz + eps2;
            let inv = r2.sqrt().recip();
            let inv3 = inv * inv * inv;
            let s = 0.5 * mass[j] * inv3;
            ax += s * dx;
            ay += s * dy;
            az += s * dz;
        }
        for (k, r) in [ax, ay, az].into_iter().enumerate() {
            max_err = max_err.max((got[3 * i + k] - r).abs());
        }
    }
    println!("nbody max abs error vs CPU reference: {max_err:.3e}  (n={n})");
    assert!(max_err < 1e-2, "GPU/CPU mismatch too large");
    println!("OK — backend result matches CPU reference");
}
