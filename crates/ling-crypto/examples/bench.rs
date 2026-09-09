//! Rough throughput/latency comparison across every primitive in this crate,
//! DICE-42 (the Möbius-Helix Reactor) included. Not a rigorous criterion-style
//! benchmark (no warmup isolation, no statistical outlier rejection) — good
//! enough to sanity-check relative costs and to see what DICE-42 actually
//! costs on top of the vetted AEAD it's meant to wrap.
//!
//! Run with: cargo run -p ling-crypto --release --example bench

use std::time::{Duration, Instant};

use ling_crypto::hash::{Blake3, Sha3_256, Sha3_512, Shake256};
use ling_crypto::hybrid::HybridKeypair;
use ling_crypto::mobius_helix::MobiusHelixReactor;
use ling_crypto::pq::MlKem768Keypair;
use ling_crypto::pq_sig::{MlDsa65Keypair, MlDsa87Keypair};
use ling_crypto::shamir::{reconstruct_secret, split_secret};
use ling_crypto::symmetric::{AesGcm256, XChaCha20};
use ling_crypto::zkp::{schnorr_verify, SchnorrKeypair};
use ling_crypto::{
    hkdf_sha3, vrf::vrf_verify, Argon2idParams, Ed25519Keypair, MandalaHash, MandalaParams,
    VrfKeypair, X25519Secret,
};

const PAYLOAD_LEN: usize = 4096;
const MAX_TIME: Duration = Duration::from_millis(400);
const MIN_ITERS: u32 = 5;

/// Run `f` repeatedly until at least MIN_ITERS iterations have completed AND
/// at least MAX_TIME has elapsed (whichever op is slower dominates the
/// stopping condition, so both fast and slow primitives get a fair sample).
fn bench(mut f: impl FnMut()) -> (u32, Duration) {
    let start = Instant::now();
    let mut iters = 0u32;
    loop {
        f();
        iters += 1;
        if iters >= MIN_ITERS && start.elapsed() >= MAX_TIME {
            break;
        }
    }
    (iters, start.elapsed())
}

fn report(name: &str, iters: u32, elapsed: Duration, payload_len: Option<usize>) {
    let per_op = elapsed / iters;
    let ops_per_sec = iters as f64 / elapsed.as_secs_f64();
    match payload_len {
        Some(len) => {
            let mb_per_sec = (iters as f64 * len as f64) / elapsed.as_secs_f64() / 1_000_000.0;
            println!(
                "  {name:<34} {per_op:>10.2?}/op  {ops_per_sec:>12.1} ops/s  {mb_per_sec:>9.1} MB/s  (n={iters})"
            );
        }
        None => {
            println!("  {name:<34} {per_op:>10.2?}/op  {ops_per_sec:>12.1} ops/s  (n={iters})");
        }
    }
}

fn section(title: &str) {
    println!("\n== {title} ==");
}

fn main() {
    let payload = vec![0x5Au8; PAYLOAD_LEN];
    println!("ling-crypto primitive benchmark — payload = {PAYLOAD_LEN} bytes, each row runs for >= {MAX_TIME:?} (>= {MIN_ITERS} iters)");

    // ── Hashing ─────────────────────────────────────────────────────────
    section("Hashing");
    {
        let (n, t) = bench(|| {
            Blake3::hash(&payload);
        });
        report("Blake3::hash", n, t, Some(PAYLOAD_LEN));

        let (n, t) = bench(|| {
            Sha3_256::hash(&payload);
        });
        report("Sha3_256::hash", n, t, Some(PAYLOAD_LEN));

        let (n, t) = bench(|| {
            Sha3_512::hash(&payload);
        });
        report("Sha3_512::hash", n, t, Some(PAYLOAD_LEN));

        let (n, t) = bench(|| {
            Shake256::hash(&payload, 32);
        });
        report("Shake256::hash(->32B)", n, t, Some(PAYLOAD_LEN));
    }

    // ── Symmetric AEAD (the layer DICE-42 wraps) ───────────────────────
    section("Symmetric AEAD");
    let xchacha_key = XChaCha20::generate_key();
    let xchacha = XChaCha20::new(xchacha_key);
    let aesgcm = AesGcm256::new(AesGcm256::generate_key());
    let xchacha_ct = xchacha.encrypt(&payload).unwrap();
    let aesgcm_ct = aesgcm.encrypt(&payload).unwrap();
    {
        let (n, t) = bench(|| {
            xchacha.encrypt(&payload).unwrap();
        });
        report("XChaCha20::encrypt", n, t, Some(PAYLOAD_LEN));

        let (n, t) = bench(|| {
            xchacha.decrypt(&xchacha_ct).unwrap();
        });
        report("XChaCha20::decrypt", n, t, Some(PAYLOAD_LEN));

        let (n, t) = bench(|| {
            aesgcm.encrypt(&payload).unwrap();
        });
        report("AesGcm256::encrypt", n, t, Some(PAYLOAD_LEN));

        let (n, t) = bench(|| {
            aesgcm.decrypt(&aesgcm_ct).unwrap();
        });
        report("AesGcm256::decrypt", n, t, Some(PAYLOAD_LEN));
    }

    // ── DICE-42 (Möbius-Helix Reactor) ─────────────────────────────────
    section("DICE-42 (Mobius-Helix Reactor) — cascade layer on top of the AEAD above");
    let reactor = MobiusHelixReactor::derive(&[0x11u8; 32]);
    let nonce = [0x22u8; 24];
    let dice_sealed = reactor.seal(&nonce, &payload);
    let dice_seal_stats: (u32, Duration);
    let dice_open_stats: (u32, Duration);
    {
        let (n, t) = bench(|| {
            reactor.seal(&nonce, &payload);
        });
        report("DICE-42::seal (standalone)", n, t, Some(PAYLOAD_LEN));
        dice_seal_stats = (n, t);

        let (n, t) = bench(|| {
            reactor.open(&dice_sealed).unwrap();
        });
        report("DICE-42::open (standalone)", n, t, Some(PAYLOAD_LEN));
        dice_open_stats = (n, t);
    }

    // ── The actual lingtp record cascade: XChaCha20 AEAD, then DICE-42 ──
    section("lingtp record cascade — XChaCha20-Poly1305 -> DICE-42 (what goes on the wire)");
    let (cascade_seal_n, cascade_seal_t) = bench(|| {
        let inner = xchacha.encrypt(&payload).unwrap();
        reactor.seal(&nonce, &inner);
    });
    report("cascade seal (encrypt+DICE-42)", cascade_seal_n, cascade_seal_t, Some(PAYLOAD_LEN));

    let cascade_wire = reactor.seal(&nonce, &xchacha.encrypt(&payload).unwrap());
    let (cascade_open_n, cascade_open_t) = bench(|| {
        let inner = reactor.open(&cascade_wire).unwrap();
        xchacha.decrypt(&inner).unwrap();
    });
    report("cascade open (DICE-42+decrypt)", cascade_open_n, cascade_open_t, Some(PAYLOAD_LEN));

    // ── KDF / password hashing ──────────────────────────────────────────
    section("KDF");
    {
        let (n, t) = bench(|| {
            hkdf_sha3(b"shared secret", b"salt", b"lingtp-v1 label", 32).unwrap();
        });
        report("hkdf_sha3(->32B)", n, t, None);

        let params = Argon2idParams::default();
        let (n, t) = bench(|| {
            params.hash_password(b"correct horse battery staple").unwrap();
        });
        report("Argon2idParams::hash_password", n, t, None);
    }

    // ── Classical asymmetric ─────────────────────────────────────────────
    section("Classical asymmetric (pre-quantum baseline)");
    {
        let (n, t) = bench(|| {
            Ed25519Keypair::generate();
        });
        report("Ed25519Keypair::generate", n, t, None);

        let ed = Ed25519Keypair::generate();
        let ed_sig = ed.sign(&payload[..64]);
        let (n, t) = bench(|| {
            ed.sign(&payload[..64]);
        });
        report("Ed25519::sign(64B)", n, t, None);

        let (n, t) = bench(|| {
            Ed25519Keypair::verify(&ed.public_key(), &payload[..64], &ed_sig).unwrap();
        });
        report("Ed25519::verify(64B)", n, t, None);

        let x_a = X25519Secret::generate();
        let x_b = X25519Secret::generate();
        let x_b_pub = x_b.public_key();
        let (n, t) = bench(|| {
            x_a.diffie_hellman(&x_b_pub);
        });
        report("X25519::diffie_hellman", n, t, None);
    }

    // ── Post-quantum signatures ──────────────────────────────────────────
    section("Post-quantum signatures (FIPS 204)");
    {
        let (n, t) = bench(|| {
            MlDsa65Keypair::generate();
        });
        report("MlDsa65Keypair::generate", n, t, None);
        let (n, t) = bench(|| {
            MlDsa87Keypair::generate();
        });
        report("MlDsa87Keypair::generate", n, t, None);

        let d65 = MlDsa65Keypair::generate();
        let d65_pk = d65.public_key();
        let d65_sig = d65.sign(&payload[..64]);
        let (n, t) = bench(|| {
            d65.sign(&payload[..64]);
        });
        report("MlDsa65::sign(64B)", n, t, None);
        let (n, t) = bench(|| {
            MlDsa65Keypair::verify(&d65_pk, &payload[..64], &d65_sig).unwrap();
        });
        report("MlDsa65::verify(64B)", n, t, None);

        let d87 = MlDsa87Keypair::generate();
        let d87_pk = d87.public_key();
        let d87_sig = d87.sign(&payload[..64]);
        let (n, t) = bench(|| {
            d87.sign(&payload[..64]);
        });
        report("MlDsa87::sign(64B)  <- lingtp handshake auth", n, t, None);
        let (n, t) = bench(|| {
            MlDsa87Keypair::verify(&d87_pk, &payload[..64], &d87_sig).unwrap();
        });
        report("MlDsa87::verify(64B) <- lingtp handshake auth", n, t, None);
    }

    // ── Post-quantum / hybrid KEM ─────────────────────────────────────────
    section("Post-quantum & hybrid key encapsulation");
    {
        let (n, t) = bench(|| {
            MlKem768Keypair::generate();
        });
        report("MlKem768Keypair::generate", n, t, None);
        let kem = MlKem768Keypair::generate();
        let kem_ek = kem.encapsulation_key();
        let (n, t) = bench(|| {
            ling_crypto::mlkem768_encapsulate(&kem_ek).unwrap();
        });
        report("MlKem768::encapsulate", n, t, None);
        let (kem_ct, _) = ling_crypto::mlkem768_encapsulate(&kem_ek).unwrap();
        let (n, t) = bench(|| {
            kem.decapsulate(&kem_ct).unwrap();
        });
        report("MlKem768::decapsulate", n, t, None);

        let (n, t) = bench(|| {
            HybridKeypair::generate();
        });
        report("HybridKeypair::generate", n, t, None);
        let hybrid = HybridKeypair::generate();
        let hybrid_pk = hybrid.public_key();
        let (n, t) = bench(|| {
            ling_crypto::hybrid_encapsulate(&hybrid_pk).unwrap();
        });
        report("Hybrid(X25519+MLKEM768)::encapsulate <- lingtp handshake", n, t, None);
        let (hybrid_ct, _) = ling_crypto::hybrid_encapsulate(&hybrid_pk).unwrap();
        let (n, t) = bench(|| {
            hybrid.decapsulate(&hybrid_ct).unwrap();
        });
        report("Hybrid(X25519+MLKEM768)::decapsulate <- lingtp handshake", n, t, None);
    }

    // ── Misc geometric / VRF / ZKP / secret sharing primitives ──────────
    section("VRF, Schnorr ZKP, Shamir, MandalaHash");
    {
        let (n, t) = bench(|| {
            VrfKeypair::generate();
        });
        report("VrfKeypair::generate", n, t, None);
        let vrf = VrfKeypair::generate();
        let vrf_pk = vrf.public_key();
        let (n, t) = bench(|| {
            vrf.evaluate(&payload[..64]);
        });
        report("Vrf::evaluate(64B)", n, t, None);
        let vrf_proof = vrf.evaluate(&payload[..64]);
        let (n, t) = bench(|| {
            assert!(vrf_verify(&vrf_pk, &payload[..64], &vrf_proof));
        });
        report("vrf_verify(64B)", n, t, None);

        let (n, t) = bench(|| {
            SchnorrKeypair::generate();
        });
        report("SchnorrKeypair::generate", n, t, None);
        let schnorr = SchnorrKeypair::generate();
        let schnorr_pk = schnorr.public_bytes();
        let (n, t) = bench(|| {
            schnorr.prove(&payload[..64]);
        });
        report("Schnorr::prove(64B)", n, t, None);
        let schnorr_proof = schnorr.prove(&payload[..64]);
        let (n, t) = bench(|| {
            assert!(schnorr_verify(&schnorr_pk, &payload[..64], &schnorr_proof));
        });
        report("schnorr_verify(64B)", n, t, None);

        let secret = vec![0x99u8; 32];
        let (n, t) = bench(|| {
            split_secret(&secret, 5, 8);
        });
        report("split_secret(5-of-8, 32B)", n, t, None);
        let shares = split_secret(&secret, 5, 8);
        let (n, t) = bench(|| {
            reconstruct_secret(&shares[..5]);
        });
        report("reconstruct_secret(5 shares)", n, t, None);

        let (n, t) = bench(|| {
            MandalaHash::new(MandalaParams::new(8, 12, 8)).key();
        });
        report("MandalaHash::new(...).key()", n, t, None);
    }

    // ── Summary: what DICE-42 actually costs on the wire ────────────────
    section("Summary — DICE-42 overhead over plain XChaCha20-Poly1305");
    let dice_seal_ns = dice_seal_stats.1.as_secs_f64() / dice_seal_stats.0 as f64;
    let dice_open_ns = dice_open_stats.1.as_secs_f64() / dice_open_stats.0 as f64;
    let cascade_seal_ns = cascade_seal_t.as_secs_f64() / cascade_seal_n as f64;
    let cascade_open_ns = cascade_open_t.as_secs_f64() / cascade_open_n as f64;
    let xchacha_only_ns = {
        let (n, t) = bench(|| {
            xchacha.encrypt(&payload).unwrap();
        });
        t.as_secs_f64() / n as f64
    };
    println!(
        "  XChaCha20 alone:        {:>8.2} us/op",
        xchacha_only_ns * 1e6
    );
    println!("  DICE-42 alone:           {:>8.2} us/op", dice_seal_ns * 1e6);
    println!(
        "  cascade (both, seal):    {:>8.2} us/op  ({:.1}x plain XChaCha20)",
        cascade_seal_ns * 1e6,
        cascade_seal_ns / xchacha_only_ns
    );
    println!("  DICE-42 open alone:      {:>8.2} us/op", dice_open_ns * 1e6);
    println!("  cascade (both, open):    {:>8.2} us/op", cascade_open_ns * 1e6);
    println!(
        "\n  At {PAYLOAD_LEN}-byte records, the DICE-42 cascade adds roughly {:.0} us/direction \
         on top of the already-authenticated XChaCha20-Poly1305 frame — the security argument \
         is defense-in-depth (independent keys, cascade cipher), not raw speed.",
        (cascade_seal_ns - xchacha_only_ns) * 1e6
    );
}
