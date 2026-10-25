//! ling-wallet runtime additions: wallet cryptography
//! (../ling-wallet/docs/LING_API.md §3), backed by `ling_crypto::lw` and
//! `ling_crypto::hypertorus` (byte-for-byte ports of the wallet's reference
//! crates `lw-crypto` / `lw-pow`; ling-wallet/docs/DESIGN.md §2–§6).
//!
//! Secret keys never cross into Ling: they live in this interpreter's
//! [`LwState`] behind unguessable random 128-bit handles, and are zeroized
//! when dropped (`lw_key_drop`, or when the interpreter/isolate goes away).
//! Every builtin that creates or uses a secret handle requires the `"keys"`
//! capability (`lw_key_drop` excepted, so any isolate can always discard).
//!
//! Error convention as in bytes.rs: `ok/bad`-returning builtins report bad
//! input with `bad(reason)`, plain-valued ones raise a runtime error, and
//! verifiers (`lw_id_verify`, `lw_ring_verify`, `lw_pk_valid`) answer
//! `false` for any malformed input. No input can make these panic.

use std::collections::HashMap;

use ling_crypto::hypertorus::{self, HyperTorus, TracePoint};
use ling_crypto::lw::identity::{IdentityKey, IdentityPublic, IdentitySignature};
use ling_crypto::lw::kem1024::{self, KemCiphertext, KemPublic, KemSecret};
use ling_crypto::lw::lrs::{self, KeyImage, OneTimeSecret, PublicKey, RingSignature, SpendSecret};
use ling_crypto::lw::{aead, hash, kdf, Zeroizing};

use super::bytes::{
    bad_val, hex_arg, hex_arg_n, hex_list_arg, list_val, ok_val, rt_err, str_arg, to_hex, uint_arg, wipe_bytes,
    U256, MAX_SAFE,
};
use super::{EvalResult, Interpreter, Value};

/// Largest output of `lw_random` / `lw_xof` (bytes).
const MAX_RANDOM: u64 = 1 << 20;
/// Argon2id bounds for `lw_password_key` (memory in KiB: 4 GiB).
const MAX_ARGON_M_KIB: u64 = 4 * 1024 * 1024;
const MAX_ARGON_T: u64 = 1024;
const MAX_ARGON_P: u64 = 64;

/// A secret held for Ling code. Every variant zeroizes itself on drop
/// (x25519 `StaticSecret` / ML-KEM / ML-DSA seeds via their `zeroize`
/// features, LRS secrets via their `Drop` impls).
enum LwKey {
    Kem(Box<KemSecret>),
    Id(Box<IdentityKey>),
    Spend(Box<SpendSecret>),
    OneTime(Box<OneTimeSecret>),
}

/// Per-isolate state for this builtin family.
#[derive(Default)]
pub(crate) struct LwState {
    keys: HashMap<String, LwKey>,
    /// HyperTorus hasher with its 16 MiB scratchpad, allocated on first use.
    pow: Option<Box<HyperTorus>>,
}

impl LwState {
    fn insert(&mut self, key: LwKey) -> String {
        loop {
            let h = to_hex(&ling_crypto::rng::random_bytes::<16>());
            if !self.keys.contains_key(&h) {
                self.keys.insert(h.clone(), key);
                return h;
            }
        }
    }
}

/// Outcome of a builtin body: `Ok(v)` is returned as-is; `Err(Plain(m))`
/// becomes a runtime error, `Err(Bad(m))` becomes `bad(m)`.
enum Fail {
    Plain(String),
    Bad(String),
    Cap(super::EvalErr),
}

type R = Result<Value, Fail>;

fn plain<T>(r: Result<T, String>) -> Result<T, Fail> {
    r.map_err(Fail::Plain)
}

fn badr<T>(r: Result<T, String>) -> Result<T, Fail> {
    r.map_err(Fail::Bad)
}

impl Interpreter {
    pub(super) fn lwcrypto_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        let r = match name {
            "lw_h256" => lw_h256(args),
            "lw_xof" => lw_xof(args),
            "lw_random" => lw_random(args),
            "lw_kem_keypair" | "lw_kem_public" | "lw_kem_decaps" | "lw_id_keypair" | "lw_id_public"
            | "lw_id_sign" | "lw_spend_keypair" | "lw_spend_public" | "lw_onetime_secret"
            | "lw_onetime_key_image" | "lw_onetime_public_of" | "lw_ring_sign" => {
                match self.require_cap("keys") {
                    Ok(()) => self.lw_keyed(name, args),
                    Err(e) => Err(Fail::Cap(e)),
                }
            },
            "lw_key_drop" => Ok(Value::Bool(match args.first() {
                Some(Value::Str(h)) => self.lw_state.keys.remove(h.as_str()).is_some(),
                _ => false,
            })),
            "lw_kem_encaps" => lw_kem_encaps(args),
            "lw_id_verify" => Ok(Value::Bool(lw_id_verify(args).unwrap_or(false))),
            "lw_id_fingerprint" => {
                plain(hex_arg(args, 0)).and_then(|pk| {
                    let pk = plain(IdentityPublic::from_bytes(&pk).map_err(|_| "public key must be 2592 bytes".into()))?;
                    Ok(Value::Str(pk.fingerprint()))
                })
            },
            "lw_onetime_public" => lw_onetime_public(args),
            "lw_ring_verify" => Ok(Value::Bool(lw_ring_verify(args).unwrap_or(false))),
            "lw_pk_valid" => Ok(Value::Bool(
                hex_arg(args, 0).ok().is_some_and(|b| PublicKey::from_bytes(&b).is_ok()),
            )),
            "lw_aead_seal" => lw_aead_seal(args),
            "lw_aead_open" => lw_aead_open(args),
            "lw_password_key" => lw_password_key(args),
            "lw_mnemonic_from" => lw_mnemonic_from(args),
            "lw_mnemonic_to" => lw_mnemonic_to(args),
            "lw_pow_hash" | "lw_pow_trace" => self.lw_pow(name, args),
            "lw_meets_difficulty" => lw_meets_difficulty(args),
            "lw_qr" => lw_qr(args),
            _ => return None,
        };
        Some(match r {
            Ok(v) => Ok(v),
            Err(Fail::Plain(m)) => Err(rt_err(name, &m)),
            Err(Fail::Bad(m)) => Ok(bad_val(&m)),
            Err(Fail::Cap(e)) => Err(e),
        })
    }

    fn key(&self, args: &[Value], i: usize) -> Result<&LwKey, String> {
        let h = str_arg(args, i)?;
        self.lw_state.keys.get(h).ok_or_else(|| "unknown key handle".to_string())
    }

    /// Builtins that create or use a secret handle (caller checked `"keys"`).
    fn lw_keyed(&mut self, name: &str, args: &[Value]) -> R {
        match name {
            "lw_kem_keypair" => {
                let seed = Zeroizing::new(badr(hex_arg_n::<{ kem1024::SEED_LEN }>(args, 0))?);
                let k = KemSecret::from_seed(&seed);
                Ok(ok_val(Value::Str(self.lw_state.insert(LwKey::Kem(Box::new(k))))))
            },
            "lw_id_keypair" => {
                let seed = Zeroizing::new(badr(hex_arg_n::<32>(args, 0))?);
                let k = IdentityKey::from_seed(&seed);
                Ok(ok_val(Value::Str(self.lw_state.insert(LwKey::Id(Box::new(k))))))
            },
            "lw_spend_keypair" => {
                let seed = Zeroizing::new(badr(hex_arg_n::<32>(args, 0))?);
                let k = SpendSecret::from_seed(&seed);
                Ok(ok_val(Value::Str(self.lw_state.insert(LwKey::Spend(Box::new(k))))))
            },
            "lw_kem_public" => match plain(self.key(args, 0))? {
                LwKey::Kem(k) => Ok(Value::Str(to_hex(k.public().as_bytes()))),
                _ => Err(Fail::Plain("handle is not a KEM key".into())),
            },
            "lw_id_public" => match plain(self.key(args, 0))? {
                LwKey::Id(k) => Ok(Value::Str(to_hex(&k.public().0))),
                _ => Err(Fail::Plain("handle is not an identity key".into())),
            },
            "lw_spend_public" => match plain(self.key(args, 0))? {
                LwKey::Spend(k) => Ok(Value::Str(to_hex(&k.public().0))),
                _ => Err(Fail::Plain("handle is not a spend key".into())),
            },
            "lw_onetime_key_image" | "lw_onetime_public_of" => match plain(self.key(args, 0))? {
                LwKey::OneTime(k) => Ok(Value::Str(to_hex(if name == "lw_onetime_key_image" {
                    &k.key_image().0
                } else {
                    &k.public().0
                }))),
                _ => Err(Fail::Plain("handle is not a one-time key".into())),
            },
            "lw_id_sign" => {
                let context = plain(str_arg(args, 1))?;
                let msg = plain(hex_arg(args, 2))?;
                match plain(self.key(args, 0))? {
                    LwKey::Id(k) => Ok(Value::Str(to_hex(&k.sign(context, &msg).0))),
                    _ => Err(Fail::Plain("handle is not an identity key".into())),
                }
            },
            "lw_kem_decaps" => {
                let mut ct = badr(hex_arg(args, 1))?;
                let context = badr(hex_arg(args, 2))?;
                let ct_ok = KemCiphertext::from_bytes(&ct);
                wipe_bytes(&mut ct);
                let ct = ct_ok.map_err(|_| Fail::Bad("ciphertext must be 1600 bytes".into()))?;
                match badr(self.key(args, 0))? {
                    LwKey::Kem(k) => {
                        let ss = Zeroizing::new(
                            k.decapsulate(&ct, &context).map_err(|_| Fail::Bad("malformed ciphertext".into()))?,
                        );
                        Ok(ok_val(Value::Str(to_hex(&ss[..]))))
                    },
                    _ => Err(Fail::Bad("handle is not a KEM key".into())),
                }
            },
            "lw_onetime_secret" => {
                let ss = Zeroizing::new(badr(hex_arg_n::<32>(args, 1))?);
                let index = badr(uint_arg(args, 2, u32::MAX as u64))? as u32;
                let ot = match badr(self.key(args, 0))? {
                    LwKey::Spend(k) => lrs::derive_onetime_secret(k, &ss, index),
                    _ => return Err(Fail::Bad("handle is not a spend key".into())),
                };
                Ok(ok_val(Value::Str(self.lw_state.insert(LwKey::OneTime(Box::new(ot))))))
            },
            "lw_ring_sign" => {
                let msg = badr(hex_arg_n::<32>(args, 0))?;
                let ring = badr(parse_ring(args, 1))?;
                let signer = badr(uint_arg(args, 2, lrs::MAX_RING_LEN as u64))? as usize;
                match badr(self.key(args, 3))? {
                    LwKey::OneTime(k) => {
                        let sig = lrs::try_sign(&msg, &ring, signer, k).map_err(|_| {
                            Fail::Bad(
                                "cannot sign: ring size must be 1..=64, signer index in range, and the \
                                 one-time key must match ring[signer]"
                                    .into(),
                            )
                        })?;
                        Ok(ok_val(Value::Str(to_hex(&sig.to_bytes()))))
                    },
                    _ => Err(Fail::Bad("handle is not a one-time key".into())),
                }
            },
            _ => Err(Fail::Plain("unknown builtin".into())),
        }
    }

    fn lw_pow(&mut self, name: &str, args: &[Value]) -> R {
        let header = plain(hex_arg(args, 0))?;
        let ht = self.lw_state.pow.get_or_insert_with(|| Box::new(HyperTorus::new()));
        if name == "lw_pow_hash" {
            return Ok(Value::Str(to_hex(&ht.hash(&header))));
        }
        let every = plain(uint_arg(args, 1, MAX_SAFE - 1))? as usize;
        let mut trace: Vec<TracePoint> = Vec::new();
        let h = ht.hash_traced(&header, every, &mut trace);
        let pts = trace
            .iter()
            .map(|t| {
                list_val(vec![
                    Value::Number(t.p[0] as f64),
                    Value::Number(t.p[1] as f64),
                    Value::Number(t.p[2] as f64),
                    Value::Number(t.p[3] as f64),
                    Value::Number(t.twist as f64),
                ])
            })
            .collect();
        Ok(list_val(vec![Value::Str(to_hex(&h)), list_val(pts)]))
    }
}

/// Ring members: a list of hex public keys, each canonical (2944 B, < q).
fn parse_ring(args: &[Value], i: usize) -> Result<Vec<PublicKey>, String> {
    let raw = hex_list_arg(args, i)?;
    if raw.is_empty() || raw.len() > lrs::MAX_RING_LEN {
        return Err(format!("ring must have 1..={} members", lrs::MAX_RING_LEN));
    }
    raw.iter()
        .enumerate()
        .map(|(k, b)| PublicKey::from_bytes(b).map_err(|_| format!("ring member {k} is not a valid public key")))
        .collect()
}

fn domain_parts(args: &[Value]) -> Result<(&str, Vec<Vec<u8>>), String> {
    Ok((str_arg(args, 0)?, hex_list_arg(args, 1)?))
}

fn lw_h256(args: &[Value]) -> R {
    let (domain, parts) = plain(domain_parts(args))?;
    let refs: Vec<&[u8]> = parts.iter().map(|p| p.as_slice()).collect();
    Ok(Value::Str(to_hex(&hash::h256(domain, &refs))))
}

fn lw_xof(args: &[Value]) -> R {
    let (domain, parts) = plain(domain_parts(args))?;
    let len = plain(uint_arg(args, 2, MAX_RANDOM))? as usize;
    let refs: Vec<&[u8]> = parts.iter().map(|p| p.as_slice()).collect();
    Ok(Value::Str(to_hex(&hash::xof_vec(domain, &refs, len))))
}

fn lw_random(args: &[Value]) -> R {
    let n = plain(uint_arg(args, 0, MAX_RANDOM))? as usize;
    let mut buf = vec![0u8; n];
    kdf::random_bytes(&mut buf);
    let h = to_hex(&buf);
    wipe_bytes(&mut buf);
    Ok(Value::Str(h))
}

fn lw_kem_encaps(args: &[Value]) -> R {
    let pk = badr(hex_arg(args, 0))?;
    let context = badr(hex_arg(args, 1))?;
    let pk = KemPublic::from_bytes(&pk).map_err(|_| Fail::Bad("invalid KEM public key".into()))?;
    let (ct, ss) = kem1024::try_encapsulate(&pk, &context).map_err(|_| Fail::Bad("invalid KEM public key".into()))?;
    let ss = Zeroizing::new(ss);
    Ok(ok_val(list_val(vec![Value::Str(to_hex(ct.as_bytes())), Value::Str(to_hex(&ss[..]))])))
}

fn lw_id_verify(args: &[Value]) -> Option<bool> {
    let pk = IdentityPublic::from_bytes(&hex_arg(args, 0).ok()?).ok()?;
    let context = str_arg(args, 1).ok()?;
    let msg = hex_arg(args, 2).ok()?;
    let sig = IdentitySignature::from_bytes(&hex_arg(args, 3).ok()?).ok()?;
    Some(pk.verify(context, &msg, &sig))
}

fn lw_onetime_public(args: &[Value]) -> R {
    let spk = badr(hex_arg(args, 0))?;
    let ss = Zeroizing::new(badr(hex_arg_n::<32>(args, 1))?);
    let index = badr(uint_arg(args, 2, u32::MAX as u64))? as u32;
    let spk = PublicKey::from_bytes(&spk).map_err(|_| Fail::Bad("invalid spend public key".into()))?;
    let t = lrs::try_derive_onetime_public(&spk, &ss, index)
        .map_err(|_| Fail::Bad("invalid spend public key".into()))?;
    Ok(ok_val(Value::Str(to_hex(&t.0))))
}

fn lw_ring_verify(args: &[Value]) -> Option<bool> {
    let msg = hex_arg_n::<32>(args, 0).ok()?;
    let ring = parse_ring(args, 1).ok()?;
    let ki = KeyImage::from_bytes(&hex_arg(args, 2).ok()?).ok()?;
    let sig = RingSignature::from_bytes(&hex_arg(args, 3).ok()?, ring.len()).ok()?;
    Some(lrs::verify(&msg, &ring, &ki, &sig))
}

fn lw_aead_seal(args: &[Value]) -> R {
    let key = Zeroizing::new(plain(hex_arg_n::<32>(args, 0))?);
    let nonce = plain(hex_arg_n::<24>(args, 1))?;
    let aad = plain(hex_arg(args, 2))?;
    let mut pt = plain(hex_arg(args, 3))?;
    let ct = aead::seal(&key, &nonce, &aad, &pt);
    wipe_bytes(&mut pt);
    Ok(Value::Str(to_hex(&ct)))
}

fn lw_aead_open(args: &[Value]) -> R {
    let key = Zeroizing::new(badr(hex_arg_n::<32>(args, 0))?);
    let nonce = badr(hex_arg_n::<24>(args, 1))?;
    let aad = badr(hex_arg(args, 2))?;
    let ct = badr(hex_arg(args, 3))?;
    let mut pt = aead::open(&key, &nonce, &aad, &ct).map_err(|_| Fail::Bad("authentication failed".into()))?;
    let h = to_hex(&pt);
    wipe_bytes(&mut pt);
    Ok(ok_val(Value::Str(h)))
}

fn lw_password_key(args: &[Value]) -> R {
    let password = badr(str_arg(args, 0))?;
    let salt = badr(hex_arg_n::<16>(args, 1))?;
    let m = badr(uint_arg(args, 2, MAX_ARGON_M_KIB))? as u32;
    let t = badr(uint_arg(args, 3, MAX_ARGON_T))? as u32;
    let p = badr(uint_arg(args, 4, MAX_ARGON_P))? as u32;
    let params = kdf::PasswordParams { m_cost_kib: m, t_cost: t, p_cost: p };
    let key = kdf::password_key(password.as_bytes(), &salt, params)
        .map_err(|_| Fail::Bad("invalid Argon2id parameters".into()))?;
    Ok(ok_val(Value::Str(to_hex(&key[..]))))
}

fn lw_mnemonic_from(args: &[Value]) -> R {
    let entropy = Zeroizing::new(plain(hex_arg_n::<32>(args, 0))?);
    let m = bip39::Mnemonic::from_entropy_in(bip39::Language::English, &entropy[..])
        .map_err(|_| Fail::Plain("invalid entropy".into()))?;
    Ok(list_val(m.words().map(|w| Value::Str(w.to_string())).collect()))
}

fn lw_mnemonic_to(args: &[Value]) -> R {
    let phrase = badr(str_arg(args, 0))?;
    let words: Vec<String> = phrase.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.len() != 24 {
        return Err(Fail::Bad("a recovery phrase has exactly 24 words".into()));
    }
    let normalized = Zeroizing::new(words.join(" "));
    let m = bip39::Mnemonic::parse_in_normalized(bip39::Language::English, &normalized).map_err(|e| {
        Fail::Bad(match e {
            bip39::Error::UnknownWord(i) => format!("word {} is not in the BIP-39 English list", i + 1),
            bip39::Error::InvalidChecksum => "checksum mismatch (check each word and the order)".into(),
            _ => "invalid recovery phrase".into(),
        })
    })?;
    let mut entropy = m.to_entropy();
    let h = to_hex(&entropy);
    wipe_bytes(&mut entropy);
    if h.len() != 64 {
        return Err(Fail::Bad("a recovery phrase has exactly 24 words".into()));
    }
    Ok(ok_val(Value::Str(h)))
}

fn lw_meets_difficulty(args: &[Value]) -> R {
    let hash = plain(hex_arg_n::<32>(args, 0))?;
    let d = match args.get(1) {
        Some(Value::Str(s)) => U256::parse_decimal(s),
        Some(Value::Number(_)) => uint_arg(args, 1, MAX_SAFE - 1).ok().map(|n| U256([n, 0, 0, 0])),
        _ => None,
    }
    .filter(|d| d.0[2] == 0 && d.0[3] == 0)
    .ok_or_else(|| Fail::Plain("difficulty must be an unsigned 128-bit decimal".into()))?;
    let d = (d.0[1] as u128) << 64 | d.0[0] as u128;
    Ok(Value::Bool(hypertorus::meets_difficulty(&hash, d)))
}

fn lw_qr(args: &[Value]) -> R {
    let text = plain(str_arg(args, 0))?;
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .map_err(|_| Fail::Plain("text too long for a QR code".into()))?;
    let n = code.width();
    let colors = code.to_colors();
    let rows = colors
        .chunks(n.max(1))
        .map(|row| Value::Str(row.iter().map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' }).collect()))
        .collect();
    Ok(list_val(rows))
}
