//! WPA2-PSK supplicant for the CYW43455: the 4-way handshake and group-key
//! rekeys, done by the kernel. The Pi 5's firmware build has no internal
//! supplicant (`sup_wpa` answers BCME_UNSUPPORTED, -23) -- which is also how
//! Linux runs it: wpa_supplicant does the handshake over plain data frames
//! and the keys are pushed in with `wsec_key`. This module is pure
//! computation (no bus, no statics): the driver feeds EAPOL frames in and
//! carries out the [`Action`]s that come back.
//!
//! IEEE 802.11-2020 12.7: AKM 00-0F-AC:2 (PSK), CCMP, key descriptor
//! version 2 (HMAC-SHA1-128 MIC, AES key wrap, SHA1-based PRF).

use alloc::vec::Vec;
use aes::cipher::{BlockDecrypt, KeyInit};
use hmac::{Hmac, Mac};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

/// The RSN IE we associate with (set with the `wpaie` iovar) and repeat in
/// message 2 -- the access point compares the two byte for byte: WPA2,
/// CCMP group and pairwise cipher, PSK, no capabilities.
pub const RSN_IE: [u8; 22] = [
    0x30, 20, 1, 0, 0x00, 0x0F, 0xAC, 4, 1, 0, 0x00, 0x0F, 0xAC, 4, 1, 0, 0x00, 0x0F, 0xAC, 2, 0, 0,
];

/// What the driver must do after feeding in a frame, in order.
pub enum Action {
    /// Send this Ethernet frame (an EAPOL-Key reply) to the access point.
    Send(Vec<u8>),
    /// Install the pairwise temporal key (CCMP, key index 0, the AP's address).
    Pairwise([u8; 16]),
    /// Install a group temporal key.
    Group { index: u8, key: Vec<u8>, tx: bool, rsc: [u8; 8] },
    /// The 4-way handshake is complete.
    Done,
}

const EAPOL: usize = 14; // the EAPOL header's offset in the Ethernet frame
const KEY: usize = 18; // the key descriptor's offset
const KEY_FIXED: usize = 95; // the descriptor's fixed part, before the key data
const KI_PAIRWISE: u16 = 1 << 3;
const KI_MIC: u16 = 1 << 8;
const KI_SECURE: u16 = 1 << 9;
const KI_ENCRYPTED: u16 = 1 << 12;

pub struct Handshake {
    pmk: [u8; 32],
    own: [u8; 6],
    ap: [u8; 6],
    /// The RSN IE message 2 repeats: the one the firmware put in the
    /// association request (read back after associating), else [`RSN_IE`].
    rsn: Vec<u8>,
    snonce: [u8; 32],
    anonce: [u8; 32],
    ptk: [u8; 48],
    have_ptk: bool,
    /// The replay counter of the last protected message we accepted.
    last_replay: Option<u64>,
    done: bool,
}

pub fn hmac_sha1(key: &[u8], parts: &[&[u8]]) -> [u8; 20] {
    let mut m = <HmacSha1 as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    for p in parts {
        m.update(p);
    }
    let out = m.finalize().into_bytes();
    let mut r = [0u8; 20];
    r.copy_from_slice(&out);
    r
}

/// PMK = PBKDF2-HMAC-SHA1(passphrase, ssid, 4096 iterations, 32 bytes).
pub fn pmk_from_passphrase(pass: &[u8], ssid: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for block in 1u32..=2 {
        let mut u = hmac_sha1(pass, &[ssid, &block.to_be_bytes()]);
        let mut t = u;
        for _ in 1..4096 {
            u = hmac_sha1(pass, &[&u]);
            for (a, b) in t.iter_mut().zip(u.iter()) {
                *a ^= b;
            }
        }
        let start = (block as usize - 1) * 20;
        let n = (32 - start).min(20);
        out[start..start + n].copy_from_slice(&t[..n]);
    }
    out
}

/// PRF-384 "Pairwise key expansion": KCK (16) | KEK (16) | TK (16).
pub fn derive_ptk(pmk: &[u8; 32], aa: &[u8; 6], spa: &[u8; 6], anonce: &[u8; 32], snonce: &[u8; 32]) -> [u8; 48] {
    let (lo_a, hi_a) = if aa < spa { (aa, spa) } else { (spa, aa) };
    let (lo_n, hi_n) = if anonce < snonce { (anonce, snonce) } else { (snonce, anonce) };
    let mut out = [0u8; 48];
    for i in 0u8..3 {
        let h = hmac_sha1(pmk, &[b"Pairwise key expansion", &[0u8], lo_a, hi_a, lo_n, hi_n, &[i]]);
        let start = i as usize * 20;
        let n = (48 - start).min(20);
        out[start..start + n].copy_from_slice(&h[..n]);
    }
    out
}

/// RFC 3394 key unwrap with a 128-bit KEK; None when the integrity check fails.
pub fn aes_unwrap(kek: &[u8; 16], data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 24 || data.len() % 8 != 0 {
        return None;
    }
    let cipher = aes::Aes128::new(kek.into());
    let n = data.len() / 8 - 1;
    let mut a = [0u8; 8];
    a.copy_from_slice(&data[..8]);
    let mut r: Vec<[u8; 8]> = (1..=n)
        .map(|i| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[i * 8..i * 8 + 8]);
            b
        })
        .collect();
    for j in (0..6usize).rev() {
        for i in (1..=n).rev() {
            let t = ((n * j + i) as u64).to_be_bytes();
            let mut block = [0u8; 16];
            for k in 0..8 {
                block[k] = a[k] ^ t[k];
            }
            block[8..].copy_from_slice(&r[i - 1]);
            cipher.decrypt_block((&mut block).into());
            a.copy_from_slice(&block[..8]);
            r[i - 1].copy_from_slice(&block[8..]);
        }
    }
    if a != [0xA6; 8] {
        return None;
    }
    Some(r.concat())
}

fn be16(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}

/// The GTK KDE (OUI 00-0F-AC type 1) in unwrapped key data: (index, tx, key).
fn parse_gtk(plain: &[u8]) -> Option<(u8, bool, Vec<u8>)> {
    let mut i = 0;
    while i + 2 <= plain.len() {
        let (id, l) = (plain[i], plain[i + 1] as usize);
        if id == 0xDD && l == 0 {
            break; // padding
        }
        if i + 2 + l > plain.len() {
            break;
        }
        if id == 0xDD && l >= 8 && plain[i + 2..i + 6] == [0x00, 0x0F, 0xAC, 1] {
            let flags = plain[i + 6];
            return Some((flags & 3, flags & 4 != 0, plain[i + 8..i + 2 + l].to_vec()));
        }
        i += 2 + l;
    }
    None
}

impl Handshake {
    pub fn new(pmk: [u8; 32], own: [u8; 6], snonce: [u8; 32]) -> Self {
        Handshake { pmk, own, ap: [0; 6], rsn: RSN_IE.to_vec(), snonce, anonce: [0; 32], ptk: [0; 48], have_ptk: false, last_replay: None, done: false }
    }

    /// The RSN IE the association request carried (so message 2 matches it).
    pub fn set_rsn_ie(&mut self, ie: &[u8]) {
        if ie.len() >= 4 && ie[0] == 0x30 && ie[1] as usize + 2 == ie.len() {
            self.rsn = ie.to_vec();
        }
    }

    pub fn done(&self) -> bool {
        self.done
    }

    /// The access point's address (valid once message 1 has arrived).
    pub fn ap(&self) -> [u8; 6] {
        self.ap
    }

    fn kck(&self) -> &[u8] {
        &self.ptk[..16]
    }

    fn kek(&self) -> [u8; 16] {
        let mut k = [0u8; 16];
        k.copy_from_slice(&self.ptk[16..32]);
        k
    }

    /// An EAPOL-Key frame to the access point: `info` is the key information
    /// field, the MIC is filled in last over the whole EAPOL part.
    fn build(&self, ver: u8, info: u16, replay: u64, nonce: &[u8; 32], data: &[u8]) -> Vec<u8> {
        let body = KEY_FIXED + data.len();
        let mut f = Vec::with_capacity(KEY + body);
        f.extend_from_slice(&self.ap);
        f.extend_from_slice(&self.own);
        f.extend_from_slice(&[0x88, 0x8E]);
        f.push(ver);
        f.push(3); // EAPOL-Key
        f.extend_from_slice(&(body as u16).to_be_bytes());
        f.push(2); // RSN key descriptor
        f.extend_from_slice(&info.to_be_bytes());
        f.extend_from_slice(&0u16.to_be_bytes()); // key length: 0 from the supplicant
        f.extend_from_slice(&replay.to_be_bytes());
        f.extend_from_slice(nonce);
        f.extend_from_slice(&[0u8; 16]); // key IV
        f.extend_from_slice(&[0u8; 8]); // key RSC
        f.extend_from_slice(&[0u8; 8]); // key ID (reserved)
        f.extend_from_slice(&[0u8; 16]); // MIC, computed below
        f.extend_from_slice(&(data.len() as u16).to_be_bytes());
        f.extend_from_slice(data);
        let mic = hmac_sha1(self.kck(), &[&f[EAPOL..]]);
        f[KEY + 77..KEY + 93].copy_from_slice(&mic[..16]);
        f
    }

    /// True when the frame's MIC is HMAC-SHA1-128 under our KCK.
    fn mic_ok(&self, eapol: &[u8]) -> bool {
        let mut copy = eapol.to_vec();
        let m0 = KEY - EAPOL + 77;
        let want: [u8; 16] = copy[m0..m0 + 16].try_into().unwrap();
        copy[m0..m0 + 16].fill(0);
        let mic = hmac_sha1(self.kck(), &[&copy]);
        mic[..16] == want
    }

    /// One EAPOL frame from the access point (an Ethernet frame, ethertype
    /// 0x888E). `Err` names what was wrong with it.
    pub fn rx(&mut self, frame: &[u8]) -> Result<Vec<Action>, &'static str> {
        if frame.len() < KEY + KEY_FIXED || be16(frame, 12) != 0x888E {
            return Err("not an EAPOL-Key frame");
        }
        if frame[EAPOL + 1] != 3 {
            return Ok(Vec::new()); // EAP / Start / Logoff: nothing for us
        }
        let total = EAPOL + 4 + be16(frame, EAPOL + 2) as usize;
        if total > frame.len() || total < KEY + KEY_FIXED {
            return Err("truncated EAPOL-Key frame");
        }
        let k = &frame[KEY..total];
        if k[0] != 2 {
            return Err("key descriptor is not RSN (a WPA1-only access point?)");
        }
        let info = be16(k, 1);
        let ver = frame[EAPOL];
        if info & 7 != 2 {
            return Err("key descriptor version is not HMAC-SHA1/AES (TKIP, or a SHA-256 AKM)");
        }
        let replay = u64::from_be_bytes(k[5..13].try_into().unwrap());
        let data_len = be16(k, 93) as usize;
        if KEY_FIXED + data_len > k.len() {
            return Err("key data runs past the frame");
        }
        let data = &k[KEY_FIXED..KEY_FIXED + data_len];
        let mut nonce = [0u8; 32];
        nonce.copy_from_slice(&k[13..45]);

        if info & KI_PAIRWISE != 0 && info & KI_MIC == 0 {
            // Message 1: the AP's nonce. Derive the PTK, answer with ours and
            // the RSN IE we associated with. (Also how a PTK rekey starts.)
            self.ap.copy_from_slice(&frame[6..12]);
            self.anonce = nonce;
            self.ptk = derive_ptk(&self.pmk, &self.ap, &self.own, &self.anonce, &self.snonce);
            self.have_ptk = true;
            self.last_replay = None;
            let rsn = self.rsn.clone();
            let reply = self.build(ver, 2 | KI_PAIRWISE | KI_MIC, replay, &self.snonce, &rsn);
            return Ok(alloc::vec![Action::Send(reply)]);
        }
        if !self.have_ptk {
            return Err("a protected key message arrived before message 1");
        }
        if info & KI_MIC == 0 {
            return Err("an unprotected key message after message 1");
        }
        if frame[6..12] != self.ap {
            return Err("key message from another address");
        }
        if self.last_replay.is_some_and(|last| replay <= last) {
            return Err("replayed key message");
        }
        if !self.mic_ok(&frame[EAPOL..total]) {
            return Err("message integrity check failed (wrong passphrase?)");
        }
        self.last_replay = Some(replay);
        let plain = if info & KI_ENCRYPTED != 0 {
            aes_unwrap(&self.kek(), data).ok_or("could not unwrap the key data")?
        } else {
            data.to_vec()
        };
        let gtk = parse_gtk(&plain);
        let mut rsc = [0u8; 8];
        rsc.copy_from_slice(&k[61..69]);
        let mut actions = Vec::new();
        if info & KI_PAIRWISE != 0 {
            // Message 3: the AP repeats its nonce and hands over the GTK.
            if nonce != self.anonce {
                return Err("message 3 carries a different nonce than message 1");
            }
            let reply = self.build(ver, 2 | KI_PAIRWISE | KI_MIC | KI_SECURE, replay, &[0u8; 32], &[]);
            actions.push(Action::Send(reply));
            let mut tk = [0u8; 16];
            tk.copy_from_slice(&self.ptk[32..48]);
            actions.push(Action::Pairwise(tk));
            if let Some((index, tx, key)) = gtk {
                actions.push(Action::Group { index, key, tx, rsc });
            }
            actions.push(Action::Done);
            self.done = true;
        } else {
            // Group key handshake, message 1: a new GTK.
            let (index, tx, key) = gtk.ok_or("group key message without a GTK")?;
            let reply = self.build(ver, 2 | KI_MIC | KI_SECURE, replay, &[0u8; 32], &[]);
            actions.push(Action::Send(reply));
            actions.push(Action::Group { index, key, tx, rsc });
        }
        Ok(actions)
    }
}
