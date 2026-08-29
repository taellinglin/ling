//! A minimal TLS 1.3 client (RFC 8446) for the kernel, on top of crypto.rs and
//! netstack's TCP: X25519 key exchange, ChaCha20-Poly1305 records, HKDF-SHA256
//! key schedule. Enough to establish an encrypted channel and do an HTTPS GET.
//!
//! Honest scope, stated at the call site too: **certificate validation is not
//! performed** -- this proves the handshake + record crypto (an eavesdropper
//! can't read the traffic), but a full client must verify the server's cert
//! chain against a trust store (X.509 + CA bundle), which is separate large
//! work; without it this is vulnerable to an active MITM. One cipher suite
//! (TLS_CHACHA20_POLY1305_SHA256), X25519 only, no resumption, no 0-RTT.

use crate::crypto::{chachapoly_open, chachapoly_seal, hmac_sha256, random_bytes, sha256, x25519, x25519_public};
use crate::drivers::netstack;
use sha2::{Digest, Sha256};

// ── HKDF key schedule ───────────────────────────────────────────────────────

fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    // HKDF-Extract(salt, IKM) = HMAC-Hash(salt, IKM).
    hmac_sha256(salt, ikm)
}

fn hkdf_expand_label(secret: &[u8; 32], label: &[u8], context: &[u8], out: &mut [u8]) {
    // struct HkdfLabel { uint16 length; opaque label<"tls13 "..>; opaque context; }
    let mut info = [0u8; 320];
    let mut n = 0;
    let l = out.len() as u16;
    info[0] = (l >> 8) as u8;
    info[1] = (l & 0xff) as u8;
    n += 2;
    info[n] = (6 + label.len()) as u8;
    n += 1;
    info[n..n + 6].copy_from_slice(b"tls13 ");
    n += 6;
    info[n..n + label.len()].copy_from_slice(label);
    n += label.len();
    info[n] = context.len() as u8;
    n += 1;
    info[n..n + context.len()].copy_from_slice(context);
    n += context.len();
    let hk = hkdf::Hkdf::<Sha256>::from_prk(secret).expect("32-byte prk");
    hk.expand(&info[..n], out).expect("hkdf expand");
}

fn derive_secret(secret: &[u8; 32], label: &[u8], transcript: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    hkdf_expand_label(secret, label, transcript, &mut out);
    out
}

/// A directional traffic key set + record sequence counter.
struct Keys {
    key: [u8; 32],
    iv: [u8; 12],
    seq: u64,
}

fn traffic_keys(secret: &[u8; 32]) -> Keys {
    let mut key = [0u8; 32];
    let mut iv = [0u8; 12];
    hkdf_expand_label(secret, b"key", &[], &mut key);
    hkdf_expand_label(secret, b"iv", &[], &mut iv);
    Keys { key, iv, seq: 0 }
}

fn record_nonce(iv: &[u8; 12], seq: u64) -> [u8; 12] {
    let mut n = *iv;
    let s = seq.to_be_bytes();
    for i in 0..8 {
        n[4 + i] ^= s[i];
    }
    n
}

/// Decrypt one TLS 1.3 application_data record in place. `hdr` is the 5-byte
/// record header (the AEAD's AAD); `body` is ciphertext||tag(16). On success
/// returns (inner_content_type, plaintext_len) with the plaintext at body[..len].
fn decrypt_record(keys: &mut Keys, hdr: &[u8; 5], body: &mut [u8]) -> Option<(u8, usize)> {
    if body.len() < 16 {
        return None;
    }
    let ct_len = body.len() - 16;
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&body[ct_len..]);
    let n = record_nonce(&keys.iv, keys.seq);
    keys.seq += 1;
    if !chachapoly_open(&keys.key, &n, hdr, &mut body[..ct_len], &tag) {
        return None;
    }
    // Strip zero padding; the last non-zero byte is the real content type.
    let mut end = ct_len;
    while end > 0 && body[end - 1] == 0 {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    Some((body[end - 1], end - 1))
}

/// Encrypt `data` as a TLS 1.3 record of inner type `ct` into `out` (which must
/// hold 5 + data.len() + 1 + 16 bytes). Returns the total record length.
fn encrypt_record(keys: &mut Keys, ct: u8, data: &[u8], out: &mut [u8]) -> usize {
    let inner_len = data.len() + 1;
    let rec_len = inner_len + 16;
    out[0] = 0x17;
    out[1] = 0x03;
    out[2] = 0x03;
    out[3] = (rec_len >> 8) as u8;
    out[4] = (rec_len & 0xff) as u8;
    let hdr = [out[0], out[1], out[2], out[3], out[4]];
    out[5..5 + data.len()].copy_from_slice(data);
    out[5 + data.len()] = ct;
    let n = record_nonce(&keys.iv, keys.seq);
    keys.seq += 1;
    let tag = chachapoly_seal(&keys.key, &n, &hdr, &mut out[5..5 + inner_len]).expect("seal");
    out[5 + inner_len..5 + inner_len + 16].copy_from_slice(&tag);
    5 + rec_len
}

// ── ClientHello ─────────────────────────────────────────────────────────────

/// Build a ClientHello record into `out`. Returns (record_len, handshake_off,
/// handshake_len) so the caller can feed the handshake bytes to the transcript.
fn build_client_hello(host: &str, pubkey: &[u8; 32], out: &mut [u8]) -> (usize, usize, usize) {
    let mut rnd = [0u8; 32];
    let mut sid = [0u8; 32];
    random_bytes(&mut rnd);
    random_bytes(&mut sid);

    // Handshake body starts at out[9] (out[0..5]=record hdr, out[5..9]=hs hdr).
    let body = 9usize;
    let mut n = body;
    out[n] = 0x03;
    out[n + 1] = 0x03;
    n += 2; // legacy_version
    out[n..n + 32].copy_from_slice(&rnd);
    n += 32;
    out[n] = 32;
    n += 1;
    out[n..n + 32].copy_from_slice(&sid);
    n += 32; // session id
    out[n] = 0x00;
    out[n + 1] = 0x02;
    out[n + 2] = 0x13;
    out[n + 3] = 0x03;
    n += 4; // cipher_suites: TLS_CHACHA20_POLY1305_SHA256
    out[n] = 0x01;
    out[n + 1] = 0x00;
    n += 2; // compression: null

    // extensions (reserve 2 bytes for total length)
    let ext_len_pos = n;
    n += 2;
    let ext_start = n;

    // server_name (SNI)
    let hl = host.len();
    out[n] = 0x00;
    out[n + 1] = 0x00;
    n += 2;
    let sni_data = 2 + 1 + 2 + hl;
    out[n] = (sni_data >> 8) as u8;
    out[n + 1] = (sni_data & 0xff) as u8;
    n += 2;
    let name_entry = 1 + 2 + hl;
    out[n] = (name_entry >> 8) as u8;
    out[n + 1] = (name_entry & 0xff) as u8;
    n += 2;
    out[n] = 0x00;
    n += 1; // host_name
    out[n] = (hl >> 8) as u8;
    out[n + 1] = (hl & 0xff) as u8;
    n += 2;
    out[n..n + hl].copy_from_slice(host.as_bytes());
    n += hl;

    // supported_versions: TLS 1.3
    out[n] = 0x00;
    out[n + 1] = 0x2b;
    out[n + 2] = 0x00;
    out[n + 3] = 0x03;
    out[n + 4] = 0x02;
    out[n + 5] = 0x03;
    out[n + 6] = 0x04;
    n += 7;

    // supported_groups: x25519
    out[n] = 0x00;
    out[n + 1] = 0x0a;
    out[n + 2] = 0x00;
    out[n + 3] = 0x04;
    out[n + 4] = 0x00;
    out[n + 5] = 0x02;
    out[n + 6] = 0x00;
    out[n + 7] = 0x1d;
    n += 8;

    // signature_algorithms (server needs one to sign its CertificateVerify)
    let algos: [u16; 6] = [0x0403, 0x0804, 0x0807, 0x0401, 0x0805, 0x0501];
    out[n] = 0x00;
    out[n + 1] = 0x0d;
    n += 2;
    let sa_data = 2 + algos.len() * 2;
    out[n] = (sa_data >> 8) as u8;
    out[n + 1] = (sa_data & 0xff) as u8;
    n += 2;
    let sa_list = algos.len() * 2;
    out[n] = (sa_list >> 8) as u8;
    out[n + 1] = (sa_list & 0xff) as u8;
    n += 2;
    for a in algos {
        out[n] = (a >> 8) as u8;
        out[n + 1] = (a & 0xff) as u8;
        n += 2;
    }

    // key_share: x25519
    out[n] = 0x00;
    out[n + 1] = 0x33;
    n += 2;
    let ks_entry = 2 + 2 + 32;
    let ks_data = 2 + ks_entry;
    out[n] = (ks_data >> 8) as u8;
    out[n + 1] = (ks_data & 0xff) as u8;
    n += 2;
    out[n] = (ks_entry >> 8) as u8;
    out[n + 1] = (ks_entry & 0xff) as u8;
    n += 2;
    out[n] = 0x00;
    out[n + 1] = 0x1d;
    out[n + 2] = 0x00;
    out[n + 3] = 0x20;
    n += 4;
    out[n..n + 32].copy_from_slice(pubkey);
    n += 32;

    let ext_total = n - ext_start;
    out[ext_len_pos] = (ext_total >> 8) as u8;
    out[ext_len_pos + 1] = (ext_total & 0xff) as u8;

    // handshake header: type client_hello (1), length
    let hs_len = n - body;
    out[5] = 0x01;
    out[6] = (hs_len >> 16) as u8;
    out[7] = (hs_len >> 8) as u8;
    out[8] = (hs_len & 0xff) as u8;

    // record header: handshake (22), legacy 0x0301, length
    let rec_len = 4 + hs_len;
    out[0] = 0x16;
    out[1] = 0x03;
    out[2] = 0x01;
    out[3] = (rec_len >> 8) as u8;
    out[4] = (rec_len & 0xff) as u8;

    (5 + rec_len, 5, 4 + hs_len)
}

/// Extract the server's X25519 key_share (32 bytes) from a ServerHello
/// handshake message (the bytes after the record header, i.e. hs_type..end).
fn parse_server_key_share(sh: &[u8]) -> Option<[u8; 32]> {
    // sh: type(1)=2, len(3), version(2), random(32), sid_len(1)+sid, suite(2), comp(1), ext_len(2), exts
    let mut p = 4 + 2 + 32;
    if p >= sh.len() {
        return None;
    }
    let sid_len = sh[p] as usize;
    p += 1 + sid_len;
    p += 2 + 1; // cipher suite + compression
    if p + 2 > sh.len() {
        return None;
    }
    let ext_len = ((sh[p] as usize) << 8) | sh[p + 1] as usize;
    p += 2;
    let end = (p + ext_len).min(sh.len());
    while p + 4 <= end {
        let et = ((sh[p] as usize) << 8) | sh[p + 1] as usize;
        let el = ((sh[p + 2] as usize) << 8) | sh[p + 3] as usize;
        p += 4;
        if et == 0x0033 && el >= 4 {
            // KeyShareEntry: group(2) + key_len(2) + key
            let klen = ((sh[p + 2] as usize) << 8) | sh[p + 3] as usize;
            if klen == 32 && p + 4 + 32 <= sh.len() {
                let mut k = [0u8; 32];
                k.copy_from_slice(&sh[p + 4..p + 36]);
                return Some(k);
            }
        }
        p += el;
    }
    None
}

// ── Handshake driver ────────────────────────────────────────────────────────

/// Diagnostic timing of the last https_get: handshake vs. download microseconds.
pub static mut LAST_HS_US: u64 = 0;
pub static mut LAST_DL_US: u64 = 0;

// Receive staging buffer for TLS records. 64KiB so a full ~32KiB advertised
// window's worth of in-flight data (plus records mid-decrypt) fits without
// the response loop having to compact on every read -- large bodies (the
// ~84KiB package avatars, bigger web pages) stream through with far fewer
// round trips.
static mut RX: [u8; 64 * 1024] = [0; 64 * 1024];
static mut TXBUF: [u8; 4096] = [0; 4096];

/// Format `n` as decimal ASCII into `buf`, returning the number of bytes.
fn u_to_dec(mut n: usize, buf: &mut [u8]) -> usize {
    if n == 0 {
        if !buf.is_empty() {
            buf[0] = b'0';
        }
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    let mut k = 0;
    while i > 0 && k < buf.len() {
        i -= 1;
        buf[k] = tmp[i];
        k += 1;
    }
    k
}

/// Public decimal formatter for diagnostics in other modules.
pub fn u_to_dec_pub(n: usize, buf: &mut [u8]) -> usize {
    u_to_dec(n, buf)
}

/// Emit a "tls: <label> rtype=.. rlen=.. rxlen=.. pos=..\n" diagnostic line.
fn log_trunc(log: &mut dyn FnMut(&[u8]), label: &[u8], rtype: usize, rlen: usize, rxlen: usize, pos: usize) {
    let mut m = [0u8; 96];
    let mut k = 0;
    let mut push = |src: &[u8]| {
        for &b in src {
            if k < m.len() {
                m[k] = b;
                k += 1;
            }
        }
    };
    push(b"tls: ");
    push(label);
    push(b" rtype=");
    let mut d = [0u8; 20];
    let n = u_to_dec(rtype, &mut d);
    push(&d[..n]);
    push(b" rlen=");
    let n = u_to_dec(rlen, &mut d);
    push(&d[..n]);
    push(b" rxlen=");
    let n = u_to_dec(rxlen, &mut d);
    push(&d[..n]);
    push(b" pos=");
    let n = u_to_dec(pos, &mut d);
    push(&d[..n]);
    push(b"\n");
    log(&m[..k]);
}

/// One-shot: TLS 1.3 handshake to `host:port`, GET `path` with `Connection:
/// close`, and write the full decrypted HTTP response (headers + body) into
/// `out`. `log(msg)` receives progress/errors. For fetching several resources
/// from one host, prefer the keep-alive session API (`https_open` /
/// `https_next` / `https_close`) -- it pays the ~190ms handshake once.
pub fn https_get(host: &str, port: u16, path: &str, out: &mut [u8], log: &mut dyn FnMut(&[u8])) -> Result<usize, &'static str> {
    // Serialize network use across cooperative tasks (background icon fetcher
    // vs. task 0). Re-entrant, so tls_handshake's dns_resolve re-acquires
    // harmlessly. Released on every return via Drop.
    let _net = netstack::NetGuard::new();
    let t0 = crate::arch::timer::now_us();
    let (mut c_app, mut s_app, pos, rxlen) = tls_handshake(host, port, log)?;
    let tx = unsafe { &mut *&raw mut TXBUF };
    let rx = unsafe { &mut *&raw mut RX };
    let mut req = [0u8; 512];
    let rn = build_get(host, path, false, &mut req);
    let m = encrypt_record(&mut c_app, 0x17, &req[..rn], tx);
    if !netstack::tcp_write(&tx[..m]) {
        return Err("write GET failed");
    }
    log(b"tls: GET sent (encrypted)\n");
    let t_get = crate::arch::timer::now_us();
    unsafe { LAST_HS_US = t_get.wrapping_sub(t0) };
    let (total, _p, _r) = read_response(&mut s_app, rx, pos, rxlen, out, false);
    unsafe { LAST_DL_US = crate::arch::timer::now_us().wrapping_sub(t_get) };
    Ok(total)
}

/// Run a full TLS 1.3 handshake to `host:port` and derive application traffic
/// keys. Returns `(client_app_keys, server_app_keys, pos, rxlen)` where
/// pos/rxlen mark bytes already buffered in the static RX after the handshake.
/// The caller holds the net lock and then sends its GET(s).
fn tls_handshake(host: &str, port: u16, log: &mut dyn FnMut(&[u8])) -> Result<(Keys, Keys, usize, usize), &'static str> {
    let ip = netstack::dns_resolve(host).ok_or("could not resolve host")?;
    if !netstack::tcp_connect(ip, port) {
        return Err("connect failed");
    }

    let mut secret = [0u8; 32];
    if !random_bytes(&mut secret) {
        return Err("no entropy (RDRAND)");
    }
    let pubkey = x25519_public(secret);

    let tx = unsafe { &mut *&raw mut TXBUF };
    let (ch_rec, ch_off, ch_len) = build_client_hello(host, &pubkey, tx);
    if !netstack::tcp_write(&tx[..ch_rec]) {
        return Err("write ClientHello failed");
    }
    log(b"tls: ClientHello sent\n");

    // Transcript hash: running SHA-256 over handshake messages.
    let mut transcript = Sha256::new();
    transcript.update(&tx[ch_off..ch_off + ch_len]);

    // Read the server's first flight into RX.
    let rx = unsafe { &mut *&raw mut RX };
    let mut rxlen = netstack::tcp_read_some(rx, 8_000_000);
    if rxlen == 0 {
        return Err("no ServerHello");
    }

    // Parse records. First record must be ServerHello (plaintext handshake).
    let mut pos = 0usize;
    let mut server_hs: Option<Keys> = None;
    let mut client_hs: Option<Keys> = None;
    let mut handshake_secret = [0u8; 32];
    let mut c_hs_secret = [0u8; 32];
    let mut th_ch_sf = [0u8; 32];
    let mut got_server_finished = false;

    // For decrypted handshake, plaintext may hold several messages; we buffer.
    let mut hs_acc = [0u8; 16 * 1024];
    let mut hs_accn = 0usize;

    let empty_hash = sha256(b"");

    let mut guard = 0;
    while !got_server_finished {
        guard += 1;
        if guard > 4000 {
            return Err("handshake stalled");
        }
        // Ensure a full record is available (5-byte header + body).
        if pos + 5 > rxlen {
            if rxlen >= rx.len() && pos > 0 {
                rx.copy_within(pos..rxlen, 0);
                rxlen -= pos;
                pos = 0;
            }
            let n = netstack::tcp_read_some(&mut rx[rxlen..], 6_000_000);
            if n == 0 {
                log_trunc(log, b"trunc-hdr", 0, 0, rxlen, pos);
                return Err("truncated handshake");
            }
            rxlen += n;
            continue;
        }
        let rtype = rx[pos];
        let rlen = ((rx[pos + 1 + 2] as usize) << 8) | rx[pos + 4] as usize;
        if pos + 5 + rlen > rxlen {
            if rxlen >= rx.len() {
                // Compact consumed handshake bytes to make room for a large
                // cert flight instead of failing outright.
                if pos > 0 {
                    rx.copy_within(pos..rxlen, 0);
                    rxlen -= pos;
                    pos = 0;
                } else {
                    log_trunc(log, b"too-large", rtype as usize, rlen, rxlen, pos);
                    return Err("record too large");
                }
            }
            let n = netstack::tcp_read_some(&mut rx[rxlen..], 6_000_000);
            if n == 0 {
                log_trunc(log, b"trunc-rec", rtype as usize, rlen, rxlen, pos);
                return Err("truncated record");
            }
            rxlen += n;
            continue;
        }
        let mut hdr = [0u8; 5];
        hdr.copy_from_slice(&rx[pos..pos + 5]);
        let body_start = pos + 5;
        let body_end = body_start + rlen;

        if rtype == 0x14 {
            // ChangeCipherSpec -- ignore in TLS 1.3.
            pos = body_end;
            continue;
        }

        if rtype == 0x16 {
            // Plaintext handshake -- the ServerHello.
            let sh = &rx[body_start..body_end];
            transcript.update(sh);
            let server_pub = parse_server_key_share(sh).ok_or("no server key_share")?;
            log(b"tls: ServerHello parsed\n");

            let th: [u8; 32] = transcript.clone().finalize().into();
            let ecdhe = x25519(secret, server_pub);
            let zero = [0u8; 32];
            let early = hkdf_extract(&zero, &zero);
            let derived = derive_secret(&early, b"derived", &empty_hash);
            handshake_secret = hkdf_extract(&derived, &ecdhe);
            let c_hs = derive_secret(&handshake_secret, b"c hs traffic", &th);
            let s_hs = derive_secret(&handshake_secret, b"s hs traffic", &th);
            server_hs = Some(traffic_keys(&s_hs));
            client_hs = Some(traffic_keys(&c_hs));
            c_hs_secret = c_hs;
            pos = body_end;
            continue;
        }

        if rtype == 0x17 {
            // Encrypted record under server handshake keys.
            let keys = server_hs.as_mut().ok_or("encrypted record before keys")?;
            let mut tmp = [0u8; 18 * 1024];
            if rlen > tmp.len() {
                return Err("enc record too big");
            }
            tmp[..rlen].copy_from_slice(&rx[body_start..body_end]);
            let (ctype, plen) = decrypt_record(keys, &hdr, &mut tmp[..rlen]).ok_or("decrypt failed (bad keys?)")?;
            pos = body_end;
            if ctype != 0x16 {
                continue; // alerts / app data during handshake: skip
            }
            // Accumulate handshake messages; process complete ones.
            if hs_accn + plen > hs_acc.len() {
                return Err("handshake too large");
            }
            hs_acc[hs_accn..hs_accn + plen].copy_from_slice(&tmp[..plen]);
            hs_accn += plen;
            // Walk complete handshake messages in hs_acc.
            let mut hp = 0usize;
            while hp + 4 <= hs_accn {
                let mlen = ((hs_acc[hp + 1] as usize) << 8 << 8) | ((hs_acc[hp + 2] as usize) << 8) | hs_acc[hp + 3] as usize;
                if hp + 4 + mlen > hs_accn {
                    break;
                }
                let mtype = hs_acc[hp];
                let msg = &hs_acc[hp..hp + 4 + mlen];
                // Server Finished is type 0x14.
                if mtype == 0x14 {
                    // th up to and including server Finished -> app secrets.
                    transcript.update(msg);
                    th_ch_sf = transcript.clone().finalize().into();
                    got_server_finished = true;
                } else {
                    transcript.update(msg);
                }
                hp += 4 + mlen;
                if got_server_finished {
                    break;
                }
            }
            // Shift any leftover partial message to the front.
            if hp > 0 {
                let rem = hs_accn - hp;
                for i in 0..rem {
                    hs_acc[i] = hs_acc[hp + i];
                }
                hs_accn = rem;
            }
            continue;
        }

        // Unknown record type.
        pos = body_end;
    }

    log(b"tls: server Finished; keys derived\n");

    // Application secrets from the master secret.
    let zero = [0u8; 32];
    let derived2 = derive_secret(&handshake_secret, b"derived", &empty_hash);
    let master = hkdf_extract(&derived2, &zero);
    let c_ap = derive_secret(&master, b"c ap traffic", &th_ch_sf);
    let s_ap = derive_secret(&master, b"s ap traffic", &th_ch_sf);

    // Client Finished: verify_data = HMAC(finished_key, Transcript(CH..server
    // Finished)); finished_key = HKDF-Expand-Label(c hs traffic, "finished", "").
    {
        let c_hs_keys = client_hs.as_mut().ok_or("no client hs keys")?;
        let mut fkey = [0u8; 32];
        hkdf_expand_label(&c_hs_secret, b"finished", &[], &mut fkey);
        let verify = hmac_sha256(&fkey, &th_ch_sf);
        let mut finmsg = [0u8; 36];
        finmsg[0] = 0x14;
        finmsg[3] = 32;
        finmsg[4..36].copy_from_slice(&verify);
        // Dummy ChangeCipherSpec (middlebox compatibility), then the Finished.
        let _ = netstack::tcp_write(&[0x14, 0x03, 0x03, 0x00, 0x01, 0x01]);
        let m = encrypt_record(c_hs_keys, 0x16, &finmsg, tx);
        if !netstack::tcp_write(&tx[..m]) {
            return Err("write Finished failed");
        }
    }
    log(b"tls: client Finished sent\n");

    let c_app = traffic_keys(&c_ap);
    let s_app = traffic_keys(&s_ap);
    Ok((c_app, s_app, pos, rxlen))
}

/// Read + decrypt the HTTP response into `out`, returning `(bytes_written,
/// new_pos, new_rxlen)`. With `keep_alive`, stops once the Content-Length body
/// is fully received (leaving the connection open for the next request);
/// otherwise reads until the peer closes. Post-handshake NewSessionTickets
/// (inner type 0x16) are skipped; application data (0x17) is collected; an
/// alert (0x15) ends the stream. rx is compacted before each read so the
/// receive sink stays large (avoids partial-take/retransmit stalls on big
/// bodies -- the ~84KiB package avatars).
fn read_response(s_app: &mut Keys, rx: &mut [u8], mut pos: usize, mut rxlen: usize, out: &mut [u8], keep_alive: bool) -> (usize, usize, usize) {
    let mut total = 0usize;
    let mut want: Option<usize> = None; // headers_end + content_length
    let mut guard2 = 0;
    loop {
        guard2 += 1;
        if guard2 > 8000 {
            break;
        }
        if let Some(w) = want {
            if total >= w {
                break; // keep-alive: full body received
            }
        }
        if pos + 5 > rxlen {
            if pos > 0 {
                rx.copy_within(pos..rxlen, 0);
                rxlen -= pos;
                pos = 0;
            }
            if rxlen >= rx.len() {
                break;
            }
            let n = netstack::tcp_read_some(&mut rx[rxlen..], 6_000_000);
            if n == 0 {
                break;
            }
            rxlen += n;
            continue;
        }
        let rlen = ((rx[pos + 3] as usize) << 8) | rx[pos + 4] as usize;
        if pos + 5 + rlen > rxlen {
            if pos > 0 {
                rx.copy_within(pos..rxlen, 0);
                rxlen -= pos;
                pos = 0;
            }
            if rxlen >= rx.len() {
                break;
            }
            let n = netstack::tcp_read_some(&mut rx[rxlen..], 6_000_000);
            if n == 0 {
                break;
            }
            rxlen += n;
            continue;
        }
        if rx[pos] == 0x14 {
            pos += 5 + rlen;
            continue; // ChangeCipherSpec
        }
        let mut hdr = [0u8; 5];
        hdr.copy_from_slice(&rx[pos..pos + 5]);
        let mut tmp = [0u8; 18 * 1024];
        if rlen > tmp.len() {
            break;
        }
        tmp[..rlen].copy_from_slice(&rx[pos + 5..pos + 5 + rlen]);
        pos += 5 + rlen;
        let Some((ctype, plen)) = decrypt_record(s_app, &hdr, &mut tmp[..rlen]) else {
            break;
        };
        if ctype == 0x17 {
            let take = plen.min(out.len() - total);
            out[total..total + take].copy_from_slice(&tmp[..take]);
            total += take;
            // Once headers are complete, learn Content-Length for keep-alive framing.
            if keep_alive && want.is_none() {
                let he = http_body_offset(&out[..total]);
                if he > 0 {
                    want = Some(he + parse_content_length(&out[..he]).unwrap_or(0));
                }
            }
            if total >= out.len() {
                break;
            }
        } else if ctype == 0x15 {
            break; // alert (close_notify)
        }
    }
    (total, pos, rxlen)
}

// -- Keep-alive session: one handshake, many GETs -------------------------
// For fetching several resources from one host (the package manager's avatar
// icons; a web page's sub-resources) without paying the ~190ms TLS handshake
// each time. The net lock is held for the whole session (open..close) so no
// other cooperative task disturbs the shared TCP connection mid-session.
struct Session {
    active: bool,
    c_app: Keys,
    s_app: Keys,
    pos: usize,
    rxlen: usize,
}
static mut SESSION: Session = Session {
    active: false,
    c_app: Keys { key: [0; 32], iv: [0; 12], seq: 0 },
    s_app: Keys { key: [0; 32], iv: [0; 12], seq: 0 },
    pos: 0,
    rxlen: 0,
};

/// Open a keep-alive TLS session to `host:port` (one handshake). Acquires the
/// net lock for the whole session. Returns false (and releases) on failure.
pub fn https_open(host: &str, port: u16, log: &mut dyn FnMut(&[u8])) -> bool {
    netstack::net_acquire();
    match tls_handshake(host, port, log) {
        Ok((c_app, s_app, pos, rxlen)) => {
            unsafe {
                let s = &mut *&raw mut SESSION;
                s.c_app = c_app;
                s.s_app = s_app;
                s.pos = pos;
                s.rxlen = rxlen;
                s.active = true;
            }
            true
        },
        Err(_) => {
            netstack::net_release();
            false
        },
    }
}

/// Fetch `path` over the open session (HTTP/1.1 keep-alive), writing the full
/// response (headers + body) to `out`. Returns the byte count, or None if
/// there's no active session or the request failed (the session is then
/// marked closed). Framed by Content-Length so the connection stays open.
pub fn https_next(host: &str, path: &str, out: &mut [u8]) -> Option<usize> {
    unsafe {
        let s = &mut *&raw mut SESSION;
        if !s.active {
            return None;
        }
        let tx = &mut *&raw mut TXBUF;
        let rx = &mut *&raw mut RX;
        let mut req = [0u8; 512];
        let rn = build_get(host, path, true, &mut req);
        let m = encrypt_record(&mut s.c_app, 0x17, &req[..rn], tx);
        if !netstack::tcp_write(&tx[..m]) {
            s.active = false;
            return None;
        }
        let (spos, srxlen) = (s.pos, s.rxlen);
        let (total, pos, rxlen) = read_response(&mut s.s_app, rx, spos, srxlen, out, true);
        s.pos = pos;
        s.rxlen = rxlen;
        if total == 0 {
            s.active = false;
            return None;
        }
        Some(total)
    }
}

/// Close the keep-alive session and release the net lock. Idempotent.
pub fn https_close() {
    unsafe {
        let s = &mut *&raw mut SESSION;
        if s.active {
            s.active = false;
            netstack::net_release();
        }
    }
}

/// Offset of the HTTP body within a raw response (past the CRLFCRLF header
/// separator), or 0 if no header terminator is found. `https_get` returns the
/// full response (headers included); callers wanting body-only use this.
pub fn http_body_offset(resp: &[u8]) -> usize {
    let mut i = 0;
    while i + 4 <= resp.len() {
        if &resp[i..i + 4] == b"\r\n\r\n" {
            return i + 4;
        }
        i += 1;
    }
    0
}

fn build_get(host: &str, path: &str, keep_alive: bool, out: &mut [u8]) -> usize {
    let mut n = 0;
    let mut put = |s: &[u8]| {
        out[n..n + s.len()].copy_from_slice(s);
        n += s.len();
    };
    put(b"GET ");
    put(path.as_bytes());
    put(b" HTTP/1.1\r\nHost: ");
    put(host.as_bytes());
    if keep_alive {
        put(b"\r\nConnection: keep-alive\r\nUser-Agent: LingOS\r\n\r\n");
    } else {
        put(b"\r\nConnection: close\r\nUser-Agent: LingOS\r\n\r\n");
    }
    n
}

/// Parse a `Content-Length:` value out of HTTP response headers (case-
/// insensitive), or None if absent. Used for keep-alive response framing.
fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let key = b"content-length:";
    let n = headers.len();
    let mut i = 0;
    while i + key.len() <= n {
        let mut m = true;
        for j in 0..key.len() {
            if headers[i + j].to_ascii_lowercase() != key[j] {
                m = false;
                break;
            }
        }
        if m {
            let mut k = i + key.len();
            while k < n && (headers[k] == b' ' || headers[k] == b'\t') {
                k += 1;
            }
            let mut v = 0usize;
            let mut any = false;
            while k < n && headers[k].is_ascii_digit() {
                v = v * 10 + (headers[k] - b'0') as usize;
                any = true;
                k += 1;
            }
            if any {
                return Some(v);
            }
        }
        i += 1;
    }
    None
}
