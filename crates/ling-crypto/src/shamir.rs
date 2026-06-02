//! Shamir's Secret Sharing over GF(2⁸).
//!
//! Split a secret byte string into `n` shares where any `k` shares
//! suffice to reconstruct the original. Uses the AES field polynomial
//! (x⁸ + x⁴ + x³ + x + 1) for Galois Field arithmetic.

use rand::Rng;

const POLY: u16 = 0x11b; // x^8 + x^4 + x^3 + x + 1

fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut result = 0u8;
    while b > 0 {
        if b & 1 != 0 { result ^= a; }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 { a ^= (POLY & 0xff) as u8; }
        b >>= 1;
    }
    result
}

fn gf_inv(x: u8) -> u8 {
    if x == 0 { return 0; }
    // Extended Euclidean in GF(2^8)
    let mut t = 0u16; let mut newt = 1u16;
    let mut r = POLY;  let mut newr = x as u16;
    while newr != 0 {
        let q = r / newr; // polynomial long division in GF(2^8) — simplified
        let tmp = r ^ gf_mul_u16(q, newr);
        r = newr; newr = tmp;
        let tmp2 = t ^ gf_mul_u16(q, newt);
        t = newt; newt = tmp2;
    }
    (t & 0xff) as u8
}

fn gf_mul_u16(a: u16, b: u16) -> u16 {
    gf_mul(a as u8, b as u8) as u16
}

fn eval_poly(coeffs: &[u8], x: u8) -> u8 {
    // Horner's method
    let mut result = 0u8;
    for &c in coeffs.iter().rev() {
        result = gf_mul(result, x) ^ c;
    }
    result
}

#[derive(Debug, Clone)]
pub struct Share {
    pub x: u8,          // x-coordinate (1..=255, unique per share)
    pub y: Vec<u8>,     // y-coordinates (one per secret byte)
}

/// Split `secret` into `n` shares, any `threshold` of which can reconstruct.
///
/// Returns shares with x = 1, 2, …, n.
pub fn split_secret(secret: &[u8], threshold: u8, n: u8) -> Vec<Share> {
    assert!(threshold >= 2, "threshold must be >= 2");
    assert!(n >= threshold, "n must be >= threshold");
    assert!(!secret.is_empty(), "secret must be non-empty");

    let mut rng = rand::thread_rng();
    let mut shares: Vec<Share> = (1..=n).map(|x| Share { x, y: Vec::new() }).collect();

    for &byte in secret {
        // Build random polynomial of degree (threshold-1) with constant term = byte
        let mut coeffs = vec![byte];
        for _ in 1..threshold {
            coeffs.push(rng.gen::<u8>());
        }
        for share in &mut shares {
            share.y.push(eval_poly(&coeffs, share.x));
        }
    }
    shares
}

/// Reconstruct the secret from at least `threshold` shares via Lagrange interpolation.
pub fn reconstruct_secret(shares: &[Share]) -> Vec<u8> {
    assert!(!shares.is_empty(), "need at least one share");
    let len = shares[0].y.len();
    let mut secret = vec![0u8; len];

    for i in 0..len {
        let mut val = 0u8;
        for (j, sj) in shares.iter().enumerate() {
            let xj = sj.x;
            let yj = sj.y[i];
            let mut num = 1u8;
            let mut den = 1u8;
            for (k, sk) in shares.iter().enumerate() {
                if k == j { continue; }
                let xk = sk.x;
                num = gf_mul(num, xk);
                den = gf_mul(den, xj ^ xk);
            }
            val ^= gf_mul(yj, gf_mul(num, gf_inv(den)));
        }
        secret[i] = val;
    }
    secret
}
