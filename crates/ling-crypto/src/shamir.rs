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
        if b & 1 != 0 {
            result ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= (POLY & 0xff) as u8;
        }
        b >>= 1;
    }
    result
}

/// Multiplicative inverse in GF(2⁸).
///
/// The nonzero elements form a cyclic group of order 255, so by Fermat's little
/// theorem `x^255 = 1` and therefore `x⁻¹ = x^254`. Computed with
/// square-and-multiply (a fixed 8 squarings + multiplies → no secret-dependent
/// branching on the loop count, unlike an extended-Euclid implementation).
fn gf_inv(x: u8) -> u8 {
    if x == 0 {
        return 0;
    }
    let mut result = 1u8;
    let mut base = x;
    let mut exp = 254u16;
    while exp > 0 {
        if exp & 1 == 1 {
            result = gf_mul(result, base);
        }
        base = gf_mul(base, base);
        exp >>= 1;
    }
    result
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
    pub x: u8,      // x-coordinate (1..=255, unique per share)
    pub y: Vec<u8>, // y-coordinates (one per secret byte)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gf_inverse_is_correct() {
        // x * x⁻¹ == 1 for every nonzero field element.
        for x in 1u16..=255 {
            let x = x as u8;
            assert_eq!(gf_mul(x, gf_inv(x)), 1, "inverse of {x} is wrong");
        }
    }

    #[test]
    fn split_then_reconstruct() {
        let secret = b"ling secret \x00\xff bytes";
        let shares = split_secret(secret, 3, 5);
        assert_eq!(shares.len(), 5);
        // Any 3 of the 5 shares reconstruct the secret.
        let subset = vec![shares[0].clone(), shares[2].clone(), shares[4].clone()];
        assert_eq!(reconstruct_secret(&subset), secret);
    }

    #[test]
    fn fewer_than_threshold_does_not_recover() {
        let secret = b"top secret";
        let shares = split_secret(secret, 3, 5);
        let two = vec![shares[0].clone(), shares[1].clone()];
        // With only 2 of 3 required shares, reconstruction must NOT yield the secret.
        assert_ne!(reconstruct_secret(&two), secret);
    }
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
                if k == j {
                    continue;
                }
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
