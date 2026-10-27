//! The messenger embedder: LAN discovery, transport, identity persistence,
//! and the desktop UI for `ling-messenger` (its own package --
//! ../../../ling-messenger). All the actual cryptography (hybrid Ed25519 +
//! ML-DSA-87 identity, hybrid X25519 + ML-KEM-768 handshake, encrypted
//! framing) lives there and is host-tested; this module only decides *when*
//! to send, dispatches arrivals, and draws the sidebar/chat window. Same
//! division of labor as `drivers/browser.rs` and `drivers/horizon.rs`.
//!
//! Transport, LAN-only for v0.1:
//!   * **Discovery** rides UDP: a small beacon carries the short, shareable
//!     `LingId` fingerprint (+ handle) as a *presence* announcement (the full
//!     ~2.6KB post-quantum identity is far too big for a datagram).
//!   * **Handshake + chat** ride TCP (`MSG_TCP_PORT`), now that the netstack has
//!     server sockets and holds several connections at once. The hybrid
//!     handshake messages (1.2KB-6KB) and the identity exchange are length-framed
//!     over the stream. On connect the two sides swap full public identities
//!     **trust-on-first-use** (a beacon only proves presence, not identity), add
//!     each other to the roster, then run the mutual hybrid handshake -- so
//!     "add a nearby peer" and "start a chat" are the same action. Compare the
//!     `LingId` shown in the UI out of band to be sure a first contact is really
//!     them (the crate's documented first-contact caveat).
//!
//! Honest limit: chat only runs while the Messenger window is open on both
//! sides (the accept + receive happen in `tick`, called from `draw`); a
//! background listener that receives while away is future work. Verifying it
//! needs two LingOS machines on a LAN (it's LAN peer-to-peer, mouse-driven).

use crate::arch::timer;
use crate::drivers::netstack;
#[cfg(target_arch = "x86_64")]
use crate::drivers::{font8x8, framebuffer, mixer, theme};
use crate::fs::lingfs;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use ling_messenger::identity::PublicIdentity;
use ling_messenger::{handshake, wire, Identity, LingId, Roster, Session};

/// Fixed UDP port for discovery beacons and the handshake/chat traffic that
/// follows -- one well-known port, same spirit as `bring`'s fixed HTTP
/// assumptions.
const MSG_PORT: u16 = 41337;

// Discovery beacon tag (UDP). The handshake + chat moved to TCP, so the old
// PKT_HS_*/PKT_FRAME datagram tags are gone.
const PKT_BEACON: u8 = 0;

const BEACON_INTERVAL_MS: u64 = 2_000;
const NEARBY_TIMEOUT_MS: u64 = 10_000;

/// TCP port for the chat handshake + messages. Discovery beacons stay on the
/// UDP `MSG_PORT`; the handshake (hybrid keys ~1.2KB, ciphertext + PQ signature
/// up to ~6KB) and identity exchange (~2.6KB) are far too big for one UDP
/// datagram, so they ride TCP -- now that the netstack has server sockets and
/// can hold several connections at once (an SSH session, a browser fetch, and a
/// chat all on their own sockets).
const MSG_TCP_PORT: u16 = 41338;

/// Presence learned from a beacon: we know a `LingId` is reachable at an IP,
/// but NOT its full public identity (too big to broadcast -- see module doc).
struct NearbyPeer {
    ip: [u8; 4],
    ling_id: LingId,
    handle: String,
    last_seen_ms: u64,
}

struct Conversation {
    peer: LingId,
    handle: usize,     // TCP socket for this chat
    rx: Vec<u8>,       // stream-reassembly buffer for length-framed messages
    session: Session,
    lines: Vec<(bool, String)>, // (from_me, text)
    since_ms: u64,     // when this connection was authenticated
    we_dialed: bool,   // we were the initiator (tie-break for simultaneous dials)
    last_ping_ms: u64, // receive-side ping rate limit
    dead: bool,        // send/decrypt failed: drop it, the next send re-dials
}

/// A received text longer than this is dropped (memory / screen abuse).
const MAX_TEXT: usize = 4096;
/// At most this many peers in the nearby list (a beacon flood can't grow it).
const MAX_NEARBY: usize = 64;
/// Unsent messages kept for retry, oldest dropped beyond this.
const MAX_OUTBOX: usize = 200;
/// A whole handshake (every read and write) must finish within this.
const HANDSHAKE_MS: u64 = 8_000;
/// Two connections to the same peer authenticated within this window are a
/// simultaneous dial: the one started by the smaller LingId wins on both sides.
const SIMUL_DIAL_MS: u64 = 5_000;
static mut LAST_DIAL: Vec<(LingId, u64)> = Vec::new();
static mut HISTORY_DIRTY: bool = false;
static mut HISTORY_BLOCKED: bool = false;
static mut LAST_SAVE_MS: u64 = 0;
/// The account whose identity / friends / history the task serves (the last
/// one signed in -- so chats keep arriving while the screen is locked).
static mut OWNER: Option<String> = None;
static mut PENDING_OWNER: Option<String> = None;

static mut IDENTITY: Option<Identity> = None;
static mut ROSTER: Roster = Roster::new();
static mut ROSTER_LOADED: bool = false;
static mut NEARBY: Vec<NearbyPeer> = Vec::new();
static mut CONVERSATIONS: Vec<Conversation> = Vec::new();
static mut LAST_BEACON_MS: u64 = 0;
// The background listener owns all chat network I/O + CONVERSATIONS mutation.
// The UI thread never touches sockets or CONVERSATIONS structurally -- it only
// queues work here (drained by the task) and reads lines for display. This keeps
// the two cooperative tasks from ever holding a `&mut` into the same state
// across a network yield (the only points a context switch can happen).
static mut OUTBOX: Vec<(LingId, String)> = Vec::new(); // (peer, text) to send
static mut CONNECT_REQ: Vec<(LingId, [u8; 4])> = Vec::new(); // (peer, ip) to dial
static mut PING_OUTBOX: Vec<LingId> = Vec::new(); // peers to nudge with a /ping
static mut LAST_PING_MS: u64 = 0; // sender-side rate limit for /ping
const PING_COOLDOWN_MS: u64 = 60_000; // at most one ping per minute
static mut NICK_OUTBOX: Vec<String> = Vec::new(); // new handle to broadcast to all peers

// Encrypted chat history, unlocked at login. CHAT_KEY is derived from the
// account password (see `set_chat_key`, called from `ling_kernel_user_login`);
// while it's None, persistence is simply off. CHAT_HISTORY mirrors every
// conversation's lines so they survive a reboot, encrypted at rest.
static mut CHAT_KEY: Option<[u8; 32]> = None;
static mut CHAT_HISTORY: Vec<(LingId, Vec<(bool, String)>)> = Vec::new();
/// Lines that arrived while no one was signed in: sealed into the history
/// the moment the keyring opens ("the messages while we were gone").
static mut PENDING: Vec<(LingId, bool, String)> = Vec::new();
/// The pre-keyring log at the lingfs root (sealed under SHA-256 of the
/// password): imported into the per-user log once, then removed.
const LEGACY_CHATLOG_FILE: &str = "chatlog.enc";

/// The signed-in user's log, under the keyring.
fn chatlog_path() -> Option<String> {
    crate::keyring::user().map(|u| alloc::format!("home/{u}/.messenger/chatlog.enc"))
}
static mut MSG_TASK_SPAWNED: bool = false;
static mut STATUS: &'static str = "arrows select, Enter chats/adds a friend; type + Enter with nothing selected sets your handle";

// -- UI state (mirrors browser.rs/horizon.rs's WEB_*/HZ_* pattern) ---------
static mut SELECTED: Option<usize> = None; // index into a merged nearby+friends list, see selectable()
const COMPOSE_MAX: usize = 400;
static mut COMPOSE: [u8; COMPOSE_MAX] = [0; COMPOSE_MAX];
static mut COMPOSE_LEN: usize = 0;

/// Where the signed-in (or last signed-in) account keeps its Messenger files.
fn owner() -> Option<String> {
    unsafe {
        if (*(&raw const OWNER)).is_none() {
            let mut b = [0u8; 64];
            if let Ok(Some(n)) = lingfs::read_file_all("messenger_owner", &mut b) {
                if let Ok(s) = core::str::from_utf8(&b[..n]) {
                    if !s.trim().is_empty() {
                        *(&raw mut OWNER) = Some(s.trim().to_string());
                    }
                }
            }
        }
        (*(&raw const OWNER)).clone()
    }
}

/// `home/<owner>/.messenger/<name>`, or the legacy root file with no owner yet.
fn store_path(name: &str, legacy: &str) -> String {
    match owner() {
        Some(u) => alloc::format!("home/{u}/.messenger/{name}"),
        None => String::from(legacy),
    }
}

/// Read a store file of any size; the first owner inherits the old root file.
fn store_read(name: &str, legacy: &str) -> Option<Vec<u8>> {
    let path = store_path(name, legacy);
    let read = |p: &str| -> Option<Vec<u8>> {
        let size = lingfs::file_size(p)?;
        let mut buf = alloc::vec![0u8; size.max(1) + 16];
        match lingfs::read_file_all(p, &mut buf) {
            Ok(Some(n)) => {
                buf.truncate(n);
                Some(buf)
            },
            _ => None,
        }
    };
    if let Some(v) = read(&path) {
        return Some(v);
    }
    if path != legacy {
        if let Some(v) = read(legacy) {
            let _ = lingfs::write_path_any(&path, &v);
            return Some(v);
        }
    }
    None
}

fn store_write(name: &str, legacy: &str, data: &[u8]) {
    let _ = lingfs::write_path_any(&store_path(name, legacy), data);
}

/// Another account signed in: serve its identity, friends and history.
fn switch_owner(user: &str) {
    if owner().as_deref() == Some(user) {
        return;
    }
    unsafe {
        *(&raw mut OWNER) = Some(user.to_string());
        let convos = &mut *&raw mut CONVERSATIONS;
        for c in convos.drain(..) {
            netstack::tcp_close_h(c.handle);
        }
        (*(&raw mut OUTBOX)).clear();
        (*(&raw mut CONNECT_REQ)).clear();
        *(&raw mut IDENTITY) = None;
        *(&raw mut ROSTER) = Roster::new();
        ROSTER_LOADED = false;
        SELECTED = None;
    }
    let _ = lingfs::write_file_any("messenger_owner", user.as_bytes());
}

fn identity() -> &'static Identity {
    unsafe {
        let slot = &mut *&raw mut IDENTITY;
        if slot.is_none() {
            *slot = Some(load_or_create_identity());
        }
        slot.as_ref().unwrap()
    }
}

fn load_or_create_identity() -> Identity {
    if let Some(v) = store_read("identity", "messenger_identity") {
        if v.len() == 32 {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&v);
            return Identity::from_seed(seed);
        }
    }
    let mut seed = [0u8; 32];
    if !crate::crypto::random_bytes(&mut seed) {
        // no hardware random source (an emulator): never an all-zero seed --
        // mix the clock and the account name (weak, and said so)
        let mut m = Vec::new();
        m.extend_from_slice(b"lingos-messenger-seed");
        m.extend_from_slice(&timer::now_us().to_le_bytes());
        m.extend_from_slice(owner().unwrap_or_default().as_bytes());
        seed = crate::crypto::sha256(&m);
        crate::console_write(b"messenger: no hardware RNG -- identity seeded from the clock\n");
    }
    store_write("identity", "messenger_identity", &seed);
    Identity::from_seed(seed)
}

fn roster() -> &'static mut Roster {
    unsafe {
        if !ROSTER_LOADED {
            ROSTER_LOADED = true;
            load_roster();
        }
        &mut *&raw mut ROSTER
    }
}

// -- Serialization ---------------------------------------------------------

/// A hybrid public identity on the wire / on disk: `ed25519_pk (32B) ‖
/// mldsa87_encoded_vk`. `PublicIdentity`'s fields are public and it carries
/// the ML-DSA key as raw encoded bytes, so this is a straight concatenation;
/// `verify` re-parses/validates the ML-DSA half, so a wrong length simply
/// fails to authenticate later rather than needing a check here.
fn pub_id_to_bytes(pi: &PublicIdentity) -> Vec<u8> {
    let mut v = Vec::with_capacity(32 + pi.mldsa87.len());
    v.extend_from_slice(pi.ed25519.as_bytes());
    v.extend_from_slice(&pi.mldsa87);
    v
}

fn pub_id_from_bytes(b: &[u8]) -> Option<PublicIdentity> {
    // Ed25519 (32) + an ML-DSA-87 verifying key (2592); anything far off is
    // garbage or an attempt to bloat the roster
    if b.len() <= 32 || b.len() > 32 + 4096 {
        return None;
    }
    let mut ed = [0u8; 32];
    ed.copy_from_slice(&b[..32]);
    let ed_vk = ed25519_dalek::VerifyingKey::from_bytes(&ed).ok()?;
    Some(PublicIdentity { ed25519: ed_vk, mldsa87: b[32..].to_vec() })
}

/// One friend per line: `hex(pub_identity)<TAB>handle`. A hybrid identity is
/// large (~2.6KB -> ~5.2KB hex), so the read buffer is sized generously.
fn load_roster() {
    let Some(buf) = store_read("friends", "messenger_friends") else { return };
    let Ok(text) = core::str::from_utf8(&buf) else { return };
    for line in text.lines() {
        let Some((hex, handle)) = line.split_once('\t') else { continue };
        let Some(bytes) = hex_decode(hex) else { continue };
        let Some(pi) = pub_id_from_bytes(&bytes) else { continue };
        unsafe { (&mut *&raw mut ROSTER).add(handle.to_string(), pi) };
    }
}

fn save_roster() {
    let mut out = String::new();
    for f in &roster().friends {
        out.push_str(&hex_encode(&pub_id_to_bytes(&f.public_identity)));
        out.push('\t');
        out.push_str(&f.handle);
        out.push('\n');
    }
    // Multi-block: each friend's hybrid PQ identity (Ed25519 + ML-DSA-87 vk,
    // hex-encoded) is ~5KB, well over the single-block write_file cap -- so
    // write_file here silently dropped the whole roster.
    store_write("friends", "messenger_friends", out.as_bytes());
}

// -- Encrypted chat history -------------------------------------------------

/// Unlock chat persistence with a key derived from the account password.
/// Called from `ling_kernel_user_login` right after a successful sign-in, so
/// history is loaded once you've proven who you are -- and is unreadable
/// (ChaCha20-Poly1305) to anyone who only has the disk. Idempotent.
pub fn unlock_for_session(password: &[u8]) {
    let Some(key) = crate::keyring::key("messenger-chat") else { return };
    let user = crate::keyring::user().unwrap_or_default();
    switch_owner(&user);
    unsafe {
        CHAT_KEY = Some(key);
        HISTORY_BLOCKED = false;
        (*(&raw mut CHAT_HISTORY)).clear();
    }
    load_history();
    import_legacy(password);
    // what came in while the screen was locked / before sign-in -- for this
    // account only
    let mut pending = unsafe { core::mem::take(&mut *(&raw mut PENDING)) };
    if unsafe { (*(&raw const PENDING_OWNER)).as_deref() } != Some(user.as_str()) {
        pending.clear();
    }
    // the open chats show their history again
    unsafe {
        let convos = &mut *&raw mut CONVERSATIONS;
        for c in convos.iter_mut() {
            c.lines = history_for(&c.peer);
        }
    }
    let had = !pending.is_empty();
    for (peer, from_me, text) in pending {
        push_history(&peer, from_me, &text);
        unsafe {
            let convos = &mut *&raw mut CONVERSATIONS;
            if let Some(c) = convos.iter_mut().find(|c| c.peer == peer) {
                c.lines.push((from_me, text.clone()));
            }
        }
    }
    if had {
        save_history();
    }
    let n: usize = unsafe { (*(&raw const CHAT_HISTORY)).iter().map(|(_, l)| l.len()).sum() };
    crate::console_write(b"messenger: history unlocked (");
    crate::print_decimal(n as u64);
    crate::console_write(b" lines under the keyring)\n");
}

/// Screen lock / log out: forget the key and the decrypted history.
pub fn lock_session() {
    if unsafe { HISTORY_DIRTY } {
        save_history();
    }
    unsafe {
        CHAT_KEY = None;
        (*(&raw mut CHAT_HISTORY)).clear();
        // the screen shows nothing of the chats while locked; the sockets stay
        // up, so messages keep arriving (held for this account until sign-in)
        let convos = &mut *&raw mut CONVERSATIONS;
        for c in convos.iter_mut() {
            c.lines.clear();
        }
        *(&raw mut PENDING_OWNER) = owner();
        SELECTED = None;
    }
}

/// The old root-level log, opened with the old password-only key and folded
/// into the per-user log; then removed from the disk.
fn import_legacy(password: &[u8]) {
    let mut raw = alloc::vec![0u8; lingfs::file_size(LEGACY_CHATLOG_FILE).unwrap_or(0).max(64) + 16];
    let n = match lingfs::read_file_all(LEGACY_CHATLOG_FILE, &mut raw) {
        Ok(Some(n)) if n >= 28 => n,
        _ => return,
    };
    let mut ikm = Vec::with_capacity(12 + password.len());
    ikm.extend_from_slice(b"lingchat-v1:");
    ikm.extend_from_slice(password);
    let old_key = crate::crypto::sha256(&ikm);
    let nonce: [u8; 12] = raw[0..12].try_into().unwrap();
    let tag: [u8; 16] = raw[12..28].try_into().unwrap();
    let mut buf = raw[28..n].to_vec();
    if !crate::crypto::chachapoly_open(&old_key, &nonce, &[], &mut buf, &tag) {
        return; // another user's, or the Pi's serial-keyed log: leave it
    }
    for (peer, lines) in parse_history(&buf) {
        for (from_me, text) in lines {
            push_history(&peer, from_me, &text);
        }
    }
    save_history();
    let _ = lingfs::remove_path(LEGACY_CHATLOG_FILE);
    crate::console_write(b"messenger: older chat log moved under the keyring\n");
}

fn push_history(peer: &LingId, from_me: bool, text: &str) {
    unsafe {
        let hist = &mut *&raw mut CHAT_HISTORY;
        match hist.iter_mut().find(|(l, _)| l == peer) {
            Some(e) => e.1.push((from_me, text.to_string())),
            None => hist.push((peer.clone(), alloc::vec![(from_me, text.to_string())])),
        }
    }
}

fn serialize_history() -> Vec<u8> {
    let mut out = Vec::new();
    for (lid, lines) in unsafe { (&*&raw const CHAT_HISTORY).iter() } {
        let idb = lid.0.as_bytes();
        let idl = idb.len().min(255);
        out.push(idl as u8);
        out.extend_from_slice(&idb[..idl]);
        out.extend_from_slice(&(lines.len().min(u16::MAX as usize) as u16).to_be_bytes());
        for (from_me, text) in lines.iter().take(u16::MAX as usize) {
            out.push(*from_me as u8);
            let tb = text.as_bytes();
            let tl = tb.len().min(u16::MAX as usize);
            out.extend_from_slice(&(tl as u16).to_be_bytes());
            out.extend_from_slice(&tb[..tl]);
        }
    }
    out
}

fn save_history() {
    let Some(key) = (unsafe { CHAT_KEY }) else { return };
    if unsafe { HISTORY_BLOCKED } {
        return; // the saved log did not open: never write over it
    }
    unsafe {
        HISTORY_DIRTY = false;
        LAST_SAVE_MS = timer::now_ms();
    }
    let mut buf = serialize_history();
    let mut nonce = [0u8; 12];
    crate::crypto::random_bytes(&mut nonce);
    let Some(tag) = crate::crypto::chachapoly_seal(&key, &nonce, &[], &mut buf) else { return };
    let mut file = Vec::with_capacity(28 + buf.len());
    file.extend_from_slice(&nonce);
    file.extend_from_slice(&tag);
    file.extend_from_slice(&buf);
    let Some(path) = chatlog_path() else { return };
    let _ = lingfs::write_path_any(&path, &file);
}

fn load_history() {
    let Some(key) = (unsafe { CHAT_KEY }) else { return };
    let Some(path) = chatlog_path() else { return };
    let mut raw = alloc::vec![0u8; lingfs::file_size(&path).unwrap_or(0).max(64) + 16];
    let n = match lingfs::read_file_all(&path, &mut raw) {
        Ok(Some(n)) if n >= 28 => n,
        _ => return,
    };
    let nonce: [u8; 12] = raw[0..12].try_into().unwrap();
    let tag: [u8; 16] = raw[12..28].try_into().unwrap();
    let mut buf = raw[28..n].to_vec();
    if !crate::crypto::chachapoly_open(&key, &nonce, &[], &mut buf, &tag) {
        // wrong key or a damaged file: keep it on disk untouched
        unsafe {
            HISTORY_BLOCKED = true;
            STATUS = "the saved chat history could not be decrypted -- it is kept, new lines are not saved";
        }
        return;
    }
    unsafe { CHAT_HISTORY = parse_history(&buf) };
}

fn parse_history(b: &[u8]) -> Vec<(LingId, Vec<(bool, String)>)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let idlen = b[i] as usize;
        i += 1;
        if i + idlen + 2 > b.len() {
            break;
        }
        let lid = LingId(core::str::from_utf8(&b[i..i + idlen]).unwrap_or("").to_string());
        i += idlen;
        let nlines = u16::from_be_bytes([b[i], b[i + 1]]) as usize;
        i += 2;
        let mut lines = Vec::with_capacity(nlines);
        for _ in 0..nlines {
            if i + 3 > b.len() {
                break;
            }
            let from_me = b[i] != 0;
            let tl = u16::from_be_bytes([b[i + 1], b[i + 2]]) as usize;
            i += 3;
            if i + tl > b.len() {
                break;
            }
            lines.push((from_me, core::str::from_utf8(&b[i..i + tl]).unwrap_or("").to_string()));
            i += tl;
        }
        out.push((lid, lines));
    }
    out
}

/// Append one line to a peer's persisted history and re-save (encrypted).
fn remember(peer: &LingId, from_me: bool, text: &str) {
    if unsafe { CHAT_KEY.is_none() } {
        // nobody signed in: keep it for the moment the keyring opens
        unsafe {
            let p = &mut *(&raw mut PENDING);
            if p.len() < 2000 {
                p.push((peer.clone(), from_me, text.to_string()));
            }
            if (*(&raw const PENDING_OWNER)).is_none() {
                *(&raw mut PENDING_OWNER) = owner();
            }
        }
        return;
    }
    push_history(peer, from_me, text);
    unsafe { HISTORY_DIRTY = true };
}

/// A peer's saved messages, for prefilling a freshly opened conversation.
fn history_for(peer: &LingId) -> Vec<(bool, String)> {
    unsafe {
        (&*&raw const CHAT_HISTORY)
            .iter()
            .find(|(l, _)| l == peer)
            .map(|(_, lines)| lines.clone())
            .unwrap_or_default()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let hex = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(hex[(b >> 4) as usize] as char);
        s.push(hex[(b & 0xF) as usize] as char);
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let nibble = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i < b.len() {
        out.push((nibble(b[i])? << 4) | nibble(b[i + 1])?);
        i += 2;
    }
    Some(out)
}

pub fn status() -> &'static str {
    unsafe { STATUS }
}

/// `N` bytes of fresh RDRAND randomness, or `None` if the CPU lacks RDRAND.
/// Callers must NOT proceed with a zeroed/predictable seed in that case --
/// that would silently destroy the forward secrecy the handshake exists to
/// provide, not just degrade gracefully.
fn fresh_bytes<const N: usize>() -> Option<[u8; N]> {
    let mut s = [0u8; N];
    if crate::crypto::random_bytes(&mut s) {
        Some(s)
    } else {
        None
    }
}

pub fn my_ling_id() -> LingId {
    identity().ling_id()
}

// -- Handshake wire framing (u16-length-prefixed for the variable fields) ---

fn put_lp(out: &mut Vec<u8>, field: &[u8]) {
    out.extend_from_slice(&(field.len() as u16).to_be_bytes());
    out.extend_from_slice(field);
}

fn take_lp<'a>(body: &'a [u8], off: &mut usize) -> Option<&'a [u8]> {
    if *off + 2 > body.len() {
        return None;
    }
    let len = u16::from_be_bytes([body[*off], body[*off + 1]]) as usize;
    *off += 2;
    if *off + len > body.len() {
        return None;
    }
    let s = &body[*off..*off + len];
    *off += len;
    Some(s)
}

fn take_nonce(body: &[u8], off: &mut usize) -> Option<[u8; handshake::NONCE_LEN]> {
    if *off + handshake::NONCE_LEN > body.len() {
        return None;
    }
    let mut n = [0u8; handshake::NONCE_LEN];
    n.copy_from_slice(&body[*off..*off + handshake::NONCE_LEN]);
    *off += handshake::NONCE_LEN;
    Some(n)
}

// -- Discovery + dispatch (call every frame the window is open) ------------

/// Non-blocking: send a beacon if due, process at most one incoming datagram,
/// prune stale `NEARBY` entries. Safe to call every frame.
/// UI-side per-frame work (called from `draw`): make sure the background
/// listener is running, receive discovery beacons into `NEARBY` for the peer
/// list, and prune stale peers. All chat sockets + `CONVERSATIONS` are handled
/// by the background task; this only reads beacons and never touches a chat
/// socket, so the two tasks don't collide.
pub fn tick() {
    ensure_task();
    let now = timer::now_ms();
    // The background task is the SOLE consumer of the NIC receive queue; its
    // pump captures discovery beacons into the netstack UDP inbox. The UI just
    // reads that inbox (no NIC access -> no race with the task that was
    // dropping frames) and prunes stale peers.
    let mut buf = [0u8; 512];
    if let Some((src_ip, n)) = netstack::udp_recv_beacon(&mut buf) {
        handle_packet(src_ip, &buf[..n]);
    }
    unsafe {
        (&mut *&raw mut NEARBY).retain(|p| now.wrapping_sub(p.last_seen_ms) < NEARBY_TIMEOUT_MS);
    }
}

/// The background chat listener: a cooperative task (spawned once by `tick`)
/// that owns everything network for chat -- beaconing presence, accepting
/// inbound connections, dialing outbound ones the UI queued, sending queued
/// messages, and receiving frames -- so chats arrive even when the Messenger
/// window is closed. Discipline: network calls (which yield) run on LOCAL data;
/// every `CONVERSATIONS`/queue mutation is a non-yielding burst, so no `&mut`
/// into shared state is ever live across a context switch.
extern "C" fn messenger_task() {
    loop {
        task_step();
        crate::proc::sched::yield_now();
    }
}

/// Chat lines (and pings) received since the Messenger window last drew:
/// the desktop's dock badge.
static mut UNREAD: u32 = 0;

pub fn unread() -> u32 {
    unsafe { UNREAD }
}

pub fn clear_unread() {
    unsafe { UNREAD = 0 };
}

/// One round of the chat listener's work (what the background task loops on).
fn task_step() {
    {
        let now = timer::now_ms();
        if now.wrapping_sub(unsafe { LAST_BEACON_MS }) >= BEACON_INTERVAL_MS {
            send_beacon();
            unsafe { LAST_BEACON_MS = now };
        }
        // Capture a discovery beacon the pump routed into the inbox, so NEARBY
        // stays fresh even with the Messenger window closed.
        {
            let mut bbuf = [0u8; 512];
            if let Some((src_ip, n)) = netstack::udp_recv_beacon(&mut bbuf) {
                handle_packet(src_ip, &bbuf[..n]);
            }
        }
        // Accept one inbound connection (brief window; its own socket).
        if let Some(h) = netstack::tcp_listen_accept(MSG_TCP_PORT, 50_000) {
            responder_handshake(h);
        }
        // Dial any connections the UI queued.
        let reqs = unsafe { core::mem::take(&mut *&raw mut CONNECT_REQ) };
        for (lid, ip) in reqs {
            connect_and_chat(lid, ip);
        }
        // Send queued messages; whatever can't go now waits (and re-dials).
        let out = unsafe { core::mem::take(&mut *&raw mut OUTBOX) };
        let mut keep: Vec<(LingId, String)> = Vec::new();
        for (lid, text) in out {
            let connected = unsafe {
                (&*&raw const CONVERSATIONS).iter().any(|c| c.peer == lid && !c.dead && netstack::tcp_established_h(c.handle))
            };
            if connected && send_to(&lid, &text) {
                continue;
            }
            redial(&lid);
            keep.push((lid, text));
        }
        unsafe {
            let o = &mut *&raw mut OUTBOX;
            // anything the UI queued meanwhile goes after the retries
            let newer = core::mem::take(o);
            keep.extend(newer);
            let drop_n = keep.len().saturating_sub(MAX_OUTBOX);
            keep.drain(..drop_n);
            *o = keep;
        }
        // Send any pings the UI queued.
        let pings = unsafe { core::mem::take(&mut *&raw mut PING_OUTBOX) };
        for lid in pings {
            send_ping_to(&lid);
        }
        // Broadcast any nick changes the UI queued (to every open peer).
        let nicks = unsafe { core::mem::take(&mut *&raw mut NICK_OUTBOX) };
        for new in nicks {
            broadcast_nick(&new);
        }
        // Receive frames on open conversations.
        recv_all_conversations();
        reap_conversations();
        if unsafe { HISTORY_DIRTY } && timer::now_ms().wrapping_sub(unsafe { LAST_SAVE_MS }) >= 2_000 {
            save_history();
        }
    }
}

/// Drop conversations whose socket died or whose session broke; their
/// queued messages stay in OUTBOX and the next send re-dials.
fn reap_conversations() {
    unsafe {
        let convos = &mut *&raw mut CONVERSATIONS;
        convos.retain(|c| {
            let alive = !c.dead && netstack::tcp_established_h(c.handle);
            if !alive {
                netstack::tcp_close_h(c.handle);
            }
            alive
        });
    }
}

/// Dial a peer again if it is on the LAN and we haven't tried in 10 s.
fn redial(lid: &LingId) {
    let now = timer::now_ms();
    unsafe {
        let ld = &mut *&raw mut LAST_DIAL;
        if let Some((_, t)) = ld.iter().find(|(l, _)| l == lid) {
            if now.wrapping_sub(*t) < 10_000 {
                return;
            }
        }
        ld.retain(|(l, _)| l != lid);
        ld.push((lid.clone(), now));
        let ip = (&*&raw const NEARBY).iter().find(|p| &p.ling_id == lid).map(|p| p.ip);
        if let Some(ip) = ip {
            if !(&*&raw const CONNECT_REQ).iter().any(|(l, _)| l == lid) {
                (&mut *&raw mut CONNECT_REQ).push((lid.clone(), ip));
            }
        }
    }
}

/// Spawn the background chat listener once (idempotent). Called both at boot
/// (so LingOS is reachable for chat without opening the window) and from the
/// first Messenger draw (belt and suspenders).
pub fn start() {
    unsafe {
        if MSG_TASK_SPAWNED {
            return;
        }
        MSG_TASK_SPAWNED = true;
    }
    // Capture discovery beacons via the netstack UDP inbox (filled by the RX
    // pump), so the desktop's draw never has to poll the NIC itself.
    netstack::udp_listen(MSG_PORT);
    crate::console_write(b"messenger: background listener online (chat port 41338)\n");
    crate::proc::sched::spawn(messenger_task as *const () as usize as u64);
    // The history stays sealed until someone signs in (the keyring opens it);
    // lines that arrive before that wait in memory.
}

fn ensure_task() {
    start();
}

/// Receive at most one frame per open conversation. Takes the socket handle +
/// reassembly buffer out (non-yield), reads off the wire (yields on locals),
/// then re-attaches + decrypts + appends (non-yield) -- so the UI can read
/// `lines` safely during the network wait.
fn recv_all_conversations() {
    let n = unsafe { (&*&raw const CONVERSATIONS).len() };
    for i in 0..n {
        let (handle, mut rx) = unsafe {
            let convos = &mut *&raw mut CONVERSATIONS;
            if i >= convos.len() {
                return;
            }
            (convos[i].handle, core::mem::take(&mut convos[i].rx))
        };
        let frame = recv_framed(handle, &mut rx, 10_000);
        // A line to persist, captured under the CONVERSATIONS borrow and
        // written out after it's released (remember() touches CHAT_HISTORY +
        // the disk, not CONVERSATIONS).
        let mut remembered: Option<(LingId, bool, String)> = None;
        // A peer rename to apply after the borrow (rename_peer touches ROSTER +
        // the disk, not CONVERSATIONS).
        let mut renamed: Option<(LingId, String)> = None;
        unsafe {
            let convos = &mut *&raw mut CONVERSATIONS;
            if i < convos.len() && convos[i].handle == handle {
                convos[i].rx = rx;
                if let Some(f) = frame {
                    let opened = wire::open(&mut convos[i].session, &f);
                    if opened.is_none() {
                        // out of step (a lost frame, a replay, tampering):
                        // this session is over, a fresh handshake follows
                        convos[i].dead = true;
                    }
                    if let Some((kind, msg)) = opened {
                        let peer = convos[i].peer.clone();
                        match kind {
                            wire::KIND_TEXT => {
                                if msg.len() <= MAX_TEXT {
                                    if let Ok(text) = core::str::from_utf8(&msg) {
                                        convos[i].lines.push((false, text.to_string()));
                                        remembered = Some((peer, false, text.to_string()));
                                        UNREAD = UNREAD.saturating_add(1);
                                    }
                                }
                            },
                            wire::KIND_PING => {
                                // Attention nudge: note it in the log and chime,
                                // even if the window is closed -- at most one
                                // per 10 s from a peer.
                                let now = timer::now_ms();
                                if now.wrapping_sub(convos[i].last_ping_ms) >= 10_000 {
                                    convos[i].last_ping_ms = now;
                                    convos[i].lines.push((false, "*ping*".to_string()));
                                    UNREAD = UNREAD.saturating_add(1);
                                    #[cfg(target_arch = "x86_64")]
                                    mixer::jingle(mixer::EVENT_PING);
                                    remembered = Some((peer, false, "*ping*".to_string()));
                                }
                            },
                            wire::KIND_NICK => {
                                // Peer renamed themselves. Re-label them in the
                                // roster (keyed by this session's stable LingId,
                                // so messages keep routing) + note it in-stream.
                                if let Ok(new) = core::str::from_utf8(&msg) {
                                    let new = new.trim();
                                    if !new.is_empty() && new.len() <= 64 {
                                        let mut note = String::from("* now known as ");
                                        note.push_str(new);
                                        note.push_str(" *");
                                        convos[i].lines.push((false, note));
                                        renamed = Some((peer, new.to_string()));
                                    }
                                }
                            },
                            _ => {},
                        }
                    }
                }
            }
        }
        if let Some((peer, from_me, text)) = remembered {
            remember(&peer, from_me, &text);
        }
        if let Some((peer, new)) = renamed {
            // a friend may not take another friend's name
            let taken = roster().friends.iter().any(|f| f.ling_id != peer && f.handle.eq_ignore_ascii_case(&new));
            let label = if taken { alloc::format!("{new} ({})", &peer.0[..peer.0.len().min(6)]) } else { new };
            rename_peer(&peer, &label);
        }
    }
}

/// Seal + send one queued message to a peer, appending the local echo on
/// success. Seals under a non-yield burst, sends on the local frame (yields),
/// then appends non-yield.
fn send_to(lid: &LingId, text: &str) -> bool {
    let sealed = unsafe {
        let convos = &mut *&raw mut CONVERSATIONS;
        convos
            .iter_mut()
            .find(|c| &c.peer == lid && !c.dead)
            .map(|c| (c.handle, wire::seal(&mut c.session, wire::KIND_TEXT, text.as_bytes())))
    };
    let Some((handle, frame)) = sealed else { return false };
    let ok = send_framed(handle, &frame);
    if !ok {
        // the counter moved on but the frame did not arrive whole: this
        // session can't continue; mark it, the caller keeps the text
        unsafe {
            let convos = &mut *&raw mut CONVERSATIONS;
            if let Some(c) = convos.iter_mut().find(|c| &c.peer == lid) {
                c.dead = true;
            }
        }
        return false;
    }
    if ok {
        unsafe {
            let convos = &mut *&raw mut CONVERSATIONS;
            if let Some(c) = convos.iter_mut().find(|c| &c.peer == lid) {
                c.lines.push((true, text.to_string()));
            }
        }
        remember(lid, true, text);
    }
    true
}

/// Presence beacon: `PKT_BEACON, ling_id_len (1B), ling_id, handle`. Small on
/// purpose -- the full hybrid identity is too big to broadcast (module doc).
fn send_beacon() {
    let lid = my_ling_id().0;
    let handle = local_handle();
    let mut payload = Vec::with_capacity(2 + lid.len() + handle.len());
    payload.push(PKT_BEACON);
    payload.push(lid.len().min(255) as u8);
    payload.extend_from_slice(lid.as_bytes());
    payload.extend_from_slice(handle.as_bytes());
    netstack::udp_broadcast(MSG_PORT, MSG_PORT, &payload);
}

fn local_handle() -> String {
    // A custom handle, if one was ever set (see `set_local_handle`).
    if let Some(b) = store_read("handle", "messenger_handle") {
        if let Ok(s) = core::str::from_utf8(&b) {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
    }
    // Otherwise default to the logged-in account name -- so signing in as
    // "Sanny" shows you to peers as "Sanny" with no extra setup. Falls back to
    // the short LingId fingerprint only if there's somehow no session name.
    let user = lingfs::current_user();
    if !user.is_empty() {
        return user.to_string();
    }
    identity().ling_id().0
}

/// Set and persist a custom display handle (shown to nearby peers in
/// discovery beacons instead of the raw LingId).
pub fn set_local_handle(handle: &str) {
    store_write("handle", "messenger_handle", handle.as_bytes());
}

fn handle_packet(src_ip: [u8; 4], pkt: &[u8]) {
    if pkt.is_empty() {
        return;
    }
    // Only discovery beacons ride UDP now; the handshake + chat are over TCP.
    if pkt[0] == PKT_BEACON {
        handle_beacon(src_ip, &pkt[1..]);
    }
}

fn handle_beacon(src_ip: [u8; 4], body: &[u8]) {
    if body.is_empty() {
        return;
    }
    let idlen = body[0] as usize;
    if body.len() < 1 + idlen {
        return;
    }
    let Ok(lid_s) = core::str::from_utf8(&body[1..1 + idlen]) else { return };
    let ling_id = LingId(lid_s.to_string());
    let handle = core::str::from_utf8(&body[1 + idlen..]).unwrap_or("?").to_string();
    let now = timer::now_ms();
    unsafe {
        // If this peer is already a friend and just advertised a new nick, keep
        // their in-memory roster label current so the sidebar/chat show it even
        // before any KIND_NICK frame (persisted when that frame arrives). This
        // also covers a peer who renamed while offline and has now reconnected.
        // Beacons are unauthenticated: they never rename a friend (only an
        // authenticated KIND_NICK does) and the list is capped.
        let handle: String = handle.chars().take(64).collect();
        let nearby = &mut *&raw mut NEARBY;
        if let Some(p) = nearby.iter_mut().find(|p| p.ling_id == ling_id) {
            p.ip = src_ip;
            p.handle = handle;
            p.last_seen_ms = now;
        } else {
            if nearby.len() >= MAX_NEARBY {
                if let Some(k) = (0..nearby.len()).min_by_key(|&k| nearby[k].last_seen_ms) {
                    nearby.remove(k);
                }
            }
            nearby.push(NearbyPeer { ip: src_ip, ling_id, handle, last_seen_ms: now });
        }
    }
}

// -- TCP chat transport (handshake + framed messages) -------------------------

/// Send `data` as one length-framed message (u32 big-endian length + bytes),
/// segmented to fit the netstack's single-segment write.
fn send_framed(handle: usize, data: &[u8]) -> bool {
    let mut buf = Vec::with_capacity(4 + data.len());
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    buf.extend_from_slice(data);
    let mut off = 0usize;
    while off < buf.len() {
        let end = (off + 1400).min(buf.len());
        if !netstack::tcp_write_h(handle, &buf[off..end]) {
            return false;
        }
        off = end;
    }
    true
}

/// Read one length-framed message, accumulating stream bytes in `rx` across
/// calls. None if the peer is silent this budget (a partial message stays in
/// `rx` for the next call, so this is safe to poll incrementally).
/// `recv_framed` against an absolute deadline (a handshake's whole budget).
fn recv_framed_by(handle: usize, rx: &mut Vec<u8>, deadline_ms: u64) -> Option<Vec<u8>> {
    let now = timer::now_ms();
    if now >= deadline_ms {
        return None;
    }
    recv_framed(handle, rx, (deadline_ms - now) * 1000)
}

fn recv_framed(handle: usize, rx: &mut Vec<u8>, budget_us: u64) -> Option<Vec<u8>> {
    while rx.len() < 4 {
        let mut tmp = [0u8; 2048];
        let n = netstack::tcp_read_h(handle, &mut tmp, budget_us);
        if n == 0 {
            return None;
        }
        rx.extend_from_slice(&tmp[..n]);
    }
    let len = u32::from_be_bytes([rx[0], rx[1], rx[2], rx[3]]) as usize;
    if len > 1 << 20 {
        return None;
    }
    while rx.len() < 4 + len {
        let mut tmp = [0u8; 2048];
        let n = netstack::tcp_read_h(handle, &mut tmp, budget_us);
        if n == 0 {
            return None;
        }
        rx.extend_from_slice(&tmp[..n]);
    }
    let msg = rx[4..4 + len].to_vec();
    rx.drain(0..4 + len);
    Some(msg)
}

/// Trust-on-first-use: add a peer's full public identity to the roster if new,
/// so future sessions authenticate against it. The `LingId` shown in the UI is
/// what the user compares out of band to confirm it's really them -- the same
/// first-contact caveat the crate documents.
fn roster_add_tofu(peer: &PublicIdentity) {
    if roster().find(peer).is_some() {
        return;
    }
    let lid = LingId::from_public(peer);
    let handle = unsafe {
        (&*&raw const NEARBY)
            .iter()
            .find(|p| p.ling_id == lid)
            .map(|p| p.handle.clone())
    }
    .unwrap_or_else(|| lid.0.clone());
    roster().add(handle, peer.clone());
    save_roster();
}

/// Responder half of a chat handshake over an accepted TCP connection: exchange
/// identities (TOFU), run the hybrid handshake, verify the initiator's finish,
/// then keep the socket for the conversation.
fn responder_handshake(handle: usize) {
    let deadline = timer::now_ms() + HANDSHAKE_MS;
    let mut rx = Vec::new();
    let Some(peer_bytes) = recv_framed_by(handle, &mut rx, deadline) else {
        netstack::tcp_close_h(handle);
        return;
    };
    let Some(peer_id) = pub_id_from_bytes(&peer_bytes) else {
        netstack::tcp_close_h(handle);
        return;
    };
    if !send_framed(handle, &pub_id_to_bytes(&identity().public())) {
        netstack::tcp_close_h(handle);
        return;
    }
    let Some(initmsg) = recv_framed_by(handle, &mut rx, deadline) else {
        netstack::tcp_close_h(handle);
        return;
    };
    let mut off = 0usize;
    let (Some(hybrid_pub), Some(nonce)) = (
        take_lp(&initmsg, &mut off).map(|s| s.to_vec()),
        take_nonce(&initmsg, &mut off),
    ) else {
        netstack::tcp_close_h(handle);
        return;
    };
    let init = handshake::HandshakeInit { hybrid_pub, nonce };
    let (Some(eph), Some(rnonce)) =
        (fresh_bytes::<64>(), fresh_bytes::<{ handshake::NONCE_LEN }>())
    else {
        netstack::tcp_close_h(handle);
        return;
    };
    let Some((resp, session)) = handshake::respond(identity(), &init, eph, rnonce) else {
        netstack::tcp_close_h(handle);
        return;
    };
    let mut respmsg = Vec::new();
    put_lp(&mut respmsg, &resp.hybrid_ct);
    respmsg.extend_from_slice(&resp.nonce);
    put_lp(&mut respmsg, &resp.sig);
    if !send_framed(handle, &respmsg) {
        netstack::tcp_close_h(handle);
        return;
    }
    let Some(finmsg) = recv_framed_by(handle, &mut rx, deadline) else {
        netstack::tcp_close_h(handle);
        return;
    };
    let mut fo = 0usize;
    let Some(sig) = take_lp(&finmsg, &mut fo).map(|s| s.to_vec()) else {
        netstack::tcp_close_h(handle);
        return;
    };
    let finish = handshake::HandshakeFinish { sig };
    if !handshake::finish_responder(&peer_id, &init, &resp, &finish) {
        unsafe { STATUS = "a chat handshake failed authentication -- rejected" };
        netstack::tcp_close_h(handle);
        return;
    }
    roster_add_tofu(&peer_id); // only now: the peer proved it holds this key
    let peer_lid = LingId::from_public(&peer_id);
    open_conversation(peer_lid, handle, session, rx, false);
    unsafe { STATUS = "friend connected" };
}

/// Initiator half: dial the peer, exchange identities (TOFU), run the hybrid
/// handshake, and keep the socket for the conversation.
fn connect_and_chat(peer_lid: LingId, ip: [u8; 4]) {
    if unsafe { (&*&raw const CONVERSATIONS).iter().any(|c| c.peer == peer_lid) } {
        return; // already connected
    }
    unsafe { STATUS = "connecting..." };
    let Some(handle) = netstack::tcp_connect_h(ip, MSG_TCP_PORT, 3_000_000) else {
        unsafe { STATUS = "couldn't reach that peer -- is Messenger open on their side?" };
        return;
    };
    let deadline = timer::now_ms() + HANDSHAKE_MS;
    let fail = |msg: &'static str| {
        netstack::tcp_close_h(handle);
        unsafe { STATUS = msg };
    };
    let mut rx = Vec::new();
    if !send_framed(handle, &pub_id_to_bytes(&identity().public())) {
        return fail("the connection dropped during the handshake");
    }
    let Some(peer_bytes) = recv_framed_by(handle, &mut rx, deadline) else {
        return fail("no reply from peer");
    };
    let Some(peer_id) = pub_id_from_bytes(&peer_bytes) else {
        return fail("the peer sent an invalid identity");
    };
    if LingId::from_public(&peer_id) != peer_lid {
        // someone else answered at that address (a spoofed beacon, or the IP
        // now belongs to another machine): not who was asked for
        return fail("a different identity answered at that address -- not connected");
    }
    let (Some(eph), Some(nonce)) = (fresh_bytes::<96>(), fresh_bytes::<{ handshake::NONCE_LEN }>())
    else {
        return fail("no randomness for the handshake");
    };
    let (kp, init) = handshake::initiate(eph, nonce);
    let mut initmsg = Vec::new();
    put_lp(&mut initmsg, &init.hybrid_pub);
    initmsg.extend_from_slice(&init.nonce);
    if !send_framed(handle, &initmsg) {
        return fail("the connection dropped during the handshake");
    }
    let Some(respmsg) = recv_framed_by(handle, &mut rx, deadline) else {
        return fail("handshake timed out");
    };
    let mut ro = 0usize;
    let (Some(hybrid_ct), Some(rnonce), Some(sig)) = (
        take_lp(&respmsg, &mut ro).map(|s| s.to_vec()),
        take_nonce(&respmsg, &mut ro),
        take_lp(&respmsg, &mut ro).map(|s| s.to_vec()),
    ) else {
        return fail("the peer's handshake reply was malformed");
    };
    let resp = handshake::HandshakeResp { hybrid_ct, nonce: rnonce, sig };
    let Some((finish, session)) =
        handshake::finish_initiator(identity(), &kp, &init, &peer_id, &resp)
    else {
        unsafe { STATUS = "handshake auth failed -- wrong key answered" };
        netstack::tcp_close_h(handle);
        return;
    };
    let mut finmsg = Vec::new();
    put_lp(&mut finmsg, &finish.sig);
    if !send_framed(handle, &finmsg) {
        return fail("the connection dropped during the handshake");
    }
    roster_add_tofu(&peer_id); // verified: the key answered for itself
    let real_lid = LingId::from_public(&peer_id);
    open_conversation(real_lid, handle, session, rx, true);
    unsafe { STATUS = "connected" };
}

fn open_conversation(peer: LingId, handle: usize, session: Session, rx: Vec<u8>, we_dialed: bool) {
    let now = timer::now_ms();
    unsafe {
        let convos = &mut *&raw mut CONVERSATIONS;
        let mut carried: Option<Vec<(bool, String)>> = None;
        if let Some(k) = convos.iter().position(|c| c.peer == peer) {
            let old = &convos[k];
            let old_live = !old.dead && netstack::tcp_established_h(old.handle);
            if old_live && now.wrapping_sub(old.since_ms) < SIMUL_DIAL_MS && old.we_dialed != we_dialed {
                // both sides dialled at once: keep the connection started by
                // the smaller LingId -- both machines reach the same answer
                let we_are_smaller = my_ling_id().0 < peer.0;
                let keep_new = we_dialed == we_are_smaller;
                if !keep_new {
                    netstack::tcp_close_h(handle);
                    return;
                }
            }
            // otherwise the newest authenticated connection wins: the old one
            // is dead or half-open (the peer rebooted / changed address)
            let old = convos.remove(k);
            netstack::tcp_close_h(old.handle);
            carried = Some(old.lines);
        }
        let lines = carried.unwrap_or_else(|| history_for(&peer));
        convos.push(Conversation { peer, handle, rx, session, lines, since_ms: now, we_dialed, last_ping_ms: 0, dead: false });
    }
}

// -- Actions driven by the UI -----------------------------------------------

/// Connect to a nearby peer (Enter on a `+handle` row). Connecting runs the
/// TOFU identity exchange + hybrid handshake, which adds them to the roster on
/// success -- so "add" and "chat" are the same action now (compare the LingId
/// shown out of band to be sure it's really them).
/// Queue a dial to a nearby peer (Enter on a `+handle` row). The background
/// task runs the TOFU identity exchange + hybrid handshake, which adds them to
/// the roster on success -- so "add nearby" and "start chat" are one action.
/// Compare the LingId shown out of band to be sure a first contact is them.
pub fn add_nearby_as_friend(nearby_index: usize) {
    let Some((lid, ip)) = (unsafe {
        (&*&raw const NEARBY)
            .get(nearby_index)
            .map(|p| (p.ling_id.clone(), p.ip))
    }) else {
        return;
    };
    queue_connect(lid, ip);
}

/// Queue a chat with roster friend `friend_index` (the background task dials).
pub fn start_chat(friend_index: usize) {
    let Some(friend_lid) = roster().friends.get(friend_index).map(|f| f.ling_id.clone()) else {
        return;
    };
    let Some(ip) =
        (unsafe { (&*&raw const NEARBY).iter().find(|p| p.ling_id == friend_lid).map(|p| p.ip) })
    else {
        unsafe { STATUS = "friend not seen on this LAN recently -- ask them to open Messenger too" };
        return;
    };
    queue_connect(friend_lid, ip);
}

fn queue_connect(lid: LingId, ip: [u8; 4]) {
    unsafe {
        if (&*&raw const CONVERSATIONS).iter().any(|c| c.peer == lid) {
            return; // already connected
        }
        if (&*&raw const CONNECT_REQ).iter().any(|(l, _)| *l == lid) {
            return; // already queued
        }
        (&mut *&raw mut CONNECT_REQ).push((lid, ip));
        STATUS = "connecting...";
    }
}

/// Queue the composed line for delivery (the background task seals + sends and
/// echoes it locally once sent).
pub fn send_current_compose() {
    let Some(sel) = (unsafe { SELECTED }) else { return };
    let Some(friend_lid) = roster().friends.get(sel).map(|f| f.ling_id.clone()) else { return };
    let text =
        unsafe { core::str::from_utf8(&(&*&raw const COMPOSE)[..COMPOSE_LEN]).unwrap_or("").to_string() };
    if text.is_empty() {
        return;
    }
    if text.len() > MAX_TEXT {
        unsafe { STATUS = "that message is too long (4 KB at most)" };
        return;
    }
    let connected = unsafe { (&*&raw const CONVERSATIONS).iter().any(|c| c.peer == friend_lid && !c.dead) };
    unsafe {
        (&mut *&raw mut OUTBOX).push((friend_lid.clone(), text));
        COMPOSE_LEN = 0;
        if !connected {
            STATUS = "queued -- it is sent as soon as your friend is reachable";
        }
    }
    if !connected {
        redial(&friend_lid);
    }
}

// -- Slash commands ---------------------------------------------------------

/// Handle a `/command arg` typed into the compose box.
fn run_command(cmd: &str, arg: &str) {
    match cmd {
        "nick" => unsafe {
            if arg.is_empty() {
                STATUS = "usage: /nick <name>";
            } else if nick_in_use(arg) {
                STATUS = "nick already in use online";
            } else {
                set_local_handle(arg);
                // Tell every open peer right away (over the live session) so
                // they re-label us without waiting for a beacon; the periodic
                // beacon carries the new handle to not-yet-connected peers.
                (&mut *&raw mut NICK_OUTBOX).push(arg.to_string());
                STATUS = "handle updated";
            }
        },
        "add" => {
            if arg.is_empty() {
                unsafe { STATUS = "usage: /add <name>" };
                return;
            }
            // Match a nearby peer by handle and add + start chatting with them.
            let found = unsafe {
                (&*&raw const NEARBY)
                    .iter()
                    .position(|p| p.handle.eq_ignore_ascii_case(arg))
                    .map(|i| (i, (&*&raw const NEARBY)[i].ling_id.clone()))
            };
            match found {
                Some((i, lid)) => {
                    add_nearby_as_friend(i);
                    // Select them so what you type next goes to THIS chat (not
                    // the set-your-handle path).
                    select_peer(&lid);
                    unsafe { STATUS = "adding peer..." };
                },
                None => unsafe { STATUS = "no nearby peer by that name" },
            }
        },
        "ping" => send_ping(),
        _ => unsafe { STATUS = "unknown command (try /nick /add /ping)" },
    }
}

/// Queue a ping to the selected friend (the background task seals + sends it).
/// Rate-limited to one per minute so it can't be used to spam a sound.
fn send_ping() {
    let Some(friend_lid) =
        (unsafe { SELECTED }).and_then(|s| roster().friends.get(s).map(|f| f.ling_id.clone()))
    else {
        unsafe { STATUS = "select a friend to ping" };
        return;
    };
    unsafe {
        if !(&*&raw const CONVERSATIONS).iter().any(|c| c.peer == friend_lid) {
            STATUS = "not connected yet";
            return;
        }
        let now = timer::now_ms();
        if now.wrapping_sub(LAST_PING_MS) < PING_COOLDOWN_MS {
            STATUS = "ping on cooldown (60s)";
            return;
        }
        LAST_PING_MS = now;
        (&mut *&raw mut PING_OUTBOX).push(friend_lid);
        STATUS = "ping sent";
    }
}

/// Task-side: seal a `KIND_PING` frame to `lid`, send it, and echo it locally.
fn send_ping_to(lid: &LingId) {
    let sealed = unsafe {
        let convos = &mut *&raw mut CONVERSATIONS;
        convos
            .iter_mut()
            .find(|c| &c.peer == lid)
            .map(|c| (c.handle, wire::seal(&mut c.session, wire::KIND_PING, &[])))
    };
    let Some((handle, frame)) = sealed else { return };
    let ok = send_framed(handle, &frame);
    if ok {
        unsafe {
            let convos = &mut *&raw mut CONVERSATIONS;
            if let Some(c) = convos.iter_mut().find(|c| &c.peer == lid) {
                c.lines.push((true, "*ping*".to_string()));
            }
        }
        remember(lid, true, "*ping*");
    }
}

/// Is `name` already taken by a *different* peer we can see -- a nearby peer or
/// an existing friend? Blocks `/nick` from creating a confusing duplicate. We
/// can only judge peers we know about ("online"/discovered), which is exactly
/// what the user asked to guard against.
fn nick_in_use(name: &str) -> bool {
    let me = my_ling_id();
    let nearby_hit = unsafe {
        (&*&raw const NEARBY)
            .iter()
            .any(|p| p.ling_id != me && p.handle.eq_ignore_ascii_case(name))
    };
    nearby_hit || roster().handle_taken_by_other(name, &me)
}

/// Task-side: broadcast our new handle to every open conversation (KIND_NICK).
/// Collects (socket, sealed frame) under a non-yield borrow, then sends on
/// locals -- same discipline as `send_ping_to` (never hold a `&mut` into
/// CONVERSATIONS across the network yield).
fn broadcast_nick(new: &str) {
    let sends: Vec<(usize, Vec<u8>)> = unsafe {
        let convos = &mut *&raw mut CONVERSATIONS;
        convos
            .iter_mut()
            .map(|c| (c.handle, wire::seal(&mut c.session, wire::KIND_NICK, new.as_bytes())))
            .collect()
    };
    for (handle, frame) in sends {
        let _ = send_framed(handle, &frame);
    }
}

/// Apply a peer's nick change to the roster (keyed by their stable LingId, so
/// routing is unaffected) and persist it -- the sidebar, chat header, and
/// message author labels all read the handle from the roster at draw time, so
/// this retroactively renames them everywhere. Also updates the nearby list.
fn rename_peer(peer: &LingId, new: &str) {
    unsafe {
        if (&mut *&raw mut ROSTER).rename_by_ling_id(peer, new) {
            save_roster();
        }
        if let Some(p) = (&mut *&raw mut NEARBY).iter_mut().find(|p| &p.ling_id == peer) {
            p.handle = new.to_string();
        }
    }
}

// -- Input ------------------------------------------------------------------

/// A flat, selectable list: friends first (each can be chatted with), then
/// nearby-but-not-yet-added peers (Enter tries to add them). Returns
/// `(is_friend, index_into_that_list, label)`.
fn selectable_rows() -> Vec<(bool, usize, String)> {
    let mut rows = Vec::new();
    for (i, f) in roster().friends.iter().enumerate() {
        let online = unsafe { (&*&raw const NEARBY).iter().any(|p| p.ling_id == f.ling_id) };
        let mark = if online { "*" } else { " " };
        rows.push((true, i, alloc::format!("{}{}", mark, f.handle)));
    }
    unsafe {
        for (i, p) in (&*&raw const NEARBY).iter().enumerate() {
            if roster().find_by_ling_id(&p.ling_id).is_none() {
                rows.push((false, i, alloc::format!("+{}", p.handle)));
            }
        }
    }
    rows
}

/// UTF-8 typed into the message being written (from the input method).
pub fn insert_text(b: &[u8]) {
    unsafe {
        if COMPOSE_LEN + b.len() <= COMPOSE_MAX {
            (&mut *&raw mut COMPOSE)[COMPOSE_LEN..COMPOSE_LEN + b.len()].copy_from_slice(b);
            COMPOSE_LEN += b.len();
        }
    }
}

pub fn key(k: u8) {
    match k {
        10 => {
            let compose = unsafe {
                core::str::from_utf8(&(&*&raw const COMPOSE)[..COMPOSE_LEN]).unwrap_or("").to_string()
            };
            // Slash commands work from ANYWHERE (even with a chat open):
            //   /nick <name>  rename yourself (beacons out to peers)
            //   /add <name>   add + start chatting with a nearby peer by name
            //   /ping         nudge the selected friend with a sound (60s cooldown)
            if let Some(rest) = compose.strip_prefix('/') {
                let (cmd, arg) = match rest.find(' ') {
                    Some(i) => (&rest[..i], rest[i + 1..].trim()),
                    None => (rest, ""),
                };
                run_command(cmd, arg);
                unsafe { COMPOSE_LEN = 0 };
                return;
            }
            match unsafe { SELECTED } {
                Some(sel) if sel < roster().friends.len() => {
                    // A friend is selected: typed text sends, empty compose
                    // (re)starts the handshake.
                    if unsafe { COMPOSE_LEN > 0 } {
                        send_current_compose();
                    } else {
                        start_chat(sel);
                    }
                },
                Some(sel) => {
                    // A nearby-but-not-yet-friend row is selected: Enter tries add.
                    if let Some((false, idx, _)) = selectable_rows().get(sel) {
                        add_nearby_as_friend(*idx);
                    }
                },
                None => {
                    // Nothing selected: say how to chat / rename, keep the text.
                    if !compose.is_empty() {
                        unsafe { STATUS = "select a friend to send this (or type /nick <name> to rename yourself)" };
                    }
                },
            }
        },
        0x1B => unsafe { SELECTED = None }, // Esc deselects, back to the no-chat view
        0x11 => move_selection(-1),
        0x12 => move_selection(1),
        0x08 => unsafe {
            COMPOSE_LEN = crate::drivers::ime::utf8_pop(&(&*&raw const COMPOSE)[..], COMPOSE_LEN);
        },
        0x20..=0x7E => unsafe {
            if COMPOSE_LEN < COMPOSE_MAX {
                (&mut *&raw mut COMPOSE)[COMPOSE_LEN] = k;
                COMPOSE_LEN += 1;
            }
        },
        _ => {},
    }
}

/// Point `SELECTED` at the row for peer `lid` (friend or nearby), if present.
fn select_peer(lid: &LingId) {
    let rows = selectable_rows();
    for (row, (is_friend, idx, _)) in rows.iter().enumerate() {
        let matches = if *is_friend {
            roster().friends.get(*idx).map(|f| &f.ling_id == lid).unwrap_or(false)
        } else {
            unsafe { (&*&raw const NEARBY).get(*idx).map(|p| &p.ling_id == lid).unwrap_or(false) }
        };
        if matches {
            unsafe { SELECTED = Some(row) };
            return;
        }
    }
}

fn move_selection(delta: i32) {
    let rows = selectable_rows();
    if rows.is_empty() {
        unsafe { SELECTED = None };
        return;
    }
    let cur = unsafe { SELECTED }.unwrap_or(0) as i32;
    let next = (cur + delta).clamp(0, rows.len() as i32 - 1) as usize;
    unsafe { SELECTED = Some(next) };
}

pub fn click_row(row: usize) {
    let rows = selectable_rows();
    let Some((is_friend, idx, _)) = rows.get(row) else { return };
    if *is_friend {
        unsafe { SELECTED = Some(*idx) };
        start_chat(*idx);
    } else {
        add_nearby_as_friend(*idx);
    }
}

// -- A Ling window's view (LingOS/apps/messenger on the Pi) --------------------

/// Sidebar rows: (is_friend, index into that list, label without marker, online).
pub fn rows() -> Vec<(bool, usize, String, bool)> {
    selectable_rows()
        .into_iter()
        .map(|(f, i, label)| {
            let online = label.starts_with('*') || !f;
            (f, i, strip_marker(&label).to_string(), online)
        })
        .collect()
}

pub fn selected_row() -> Option<usize> {
    unsafe { SELECTED }
}

pub fn select_row(row: i64) {
    unsafe { SELECTED = if row < 0 { None } else { Some(row as usize) } };
}

/// Enter / click on a row: a friend starts (or restarts) the chat, a nearby
/// peer is added and dialed.
pub fn activate_row(row: usize) {
    click_row(row);
}

/// The selected friend's conversation (live lines, else the saved history).
pub fn selected_lines() -> Vec<(bool, String)> {
    let Some(sel) = (unsafe { SELECTED }) else { return Vec::new() };
    let Some(lid) = roster().friends.get(sel).map(|f| f.ling_id.clone()) else { return Vec::new() };
    let mut lines = unsafe {
        match (&*&raw const CONVERSATIONS).iter().find(|c| c.peer == lid) {
            Some(c) => c.lines.clone(),
            None => history_for(&lid),
        }
    };
    unsafe {
        for (l, text) in (&*&raw const OUTBOX).iter() {
            if *l == lid {
                lines.push((true, alloc::format!("{text}   (sending...)")));
            }
        }
    }
    lines
}

pub fn selected_connected() -> bool {
    let Some(sel) = (unsafe { SELECTED }) else { return false };
    let Some(lid) = roster().friends.get(sel).map(|f| f.ling_id.clone()) else { return false };
    unsafe { (&*&raw const CONVERSATIONS).iter().any(|c| c.peer == lid && netstack::tcp_established_h(c.handle)) }
}

/// A line typed into the compose box: `/commands`, a message to the selected
/// friend, or (nothing selected) a new display handle.
pub fn submit(text: &str) {
    let b = text.as_bytes();
    let n = b.len().min(COMPOSE_MAX);
    unsafe {
        (&mut *&raw mut COMPOSE)[..n].copy_from_slice(&b[..n]);
        COMPOSE_LEN = n;
    }
    key(10);
}

/// (display handle, LingId fingerprint) of this machine.
pub fn me() -> (String, String) {
    (local_handle(), my_ling_id().0)
}

// -- Drawing ------------------------------------------------------------------
// A Discord-style two-pane chat: a conversations sidebar with round avatars on
// the left, a header + word-wrapped message stream + compose box on the right.
// `wm.rs::window_click` mirrors SIDEBAR_W / LIST_TOP / ROW_H for the sidebar
// hit-test -- keep those three in sync there.
#[cfg(target_arch = "x86_64")]
const SIDEBAR_W: u32 = 190;
#[cfg(target_arch = "x86_64")]
const ROW_H: u32 = 40; // avatar row height in the sidebar
#[cfg(target_arch = "x86_64")]
const LIST_TOP: u32 = 34; // sidebar list starts below the "DIRECT MESSAGES" header
#[cfg(target_arch = "x86_64")]
const HEADER_H: u32 = 40; // chat-pane header height
#[cfg(target_arch = "x86_64")]
const LINE_H: u32 = 12; // one text line (8px glyph + 4px leading)
#[cfg(target_arch = "x86_64")]
const ONLINE_DOT: u32 = 0x3B_A5_5D; // Discord-ish presence green

/// A deterministic avatar color for `seed` (a handle), from a small palette --
/// the same handle always gets the same color, like Discord's default avatars.
#[cfg(target_arch = "x86_64")]
fn avatar_color(seed: &[u8]) -> u32 {
    const PAL: [u32; 6] = [0x58_65_F2, 0x3B_A5_5D, 0xFA_A6_1A, 0xED_42_45, 0x9B_59_B6, 0x1A_BC_9C];
    let mut hsh: u32 = 2166136261;
    for &b in seed {
        hsh = (hsh ^ b as u32).wrapping_mul(16777619);
    }
    PAL[(hsh as usize) % PAL.len()]
}

/// Draw a round default avatar: a color-hashed disc with the handle's first
/// letter centered. `scale` is the glyph scale (1 = 8px, 2 = 16px).
#[cfg(target_arch = "x86_64")]
fn draw_avatar(cx: u32, cy: u32, r: u32, scale: u32, name: &[u8]) {
    let initial = name
        .iter()
        .copied()
        .find(|c| c.is_ascii_alphanumeric())
        .unwrap_or(b'?')
        .to_ascii_uppercase();
    let col = avatar_color(name);
    framebuffer::back_fill_circle(cx, cy, r, col);
    let g = 4 * scale;
    font8x8::draw_char_scaled(cx - g, cy - g, initial, 0xFF_FF_FF, col, scale);
}

/// Strip the leading presence marker (`*`/`+`/space) a sidebar label carries.
fn strip_marker(label: &str) -> &str {
    match label.as_bytes().first() {
        Some(b'*') | Some(b'+') | Some(b' ') => &label[1..],
        _ => label,
    }
}

/// Word-wrap `s` into `max_w` pixels, drawing each line; returns the next `y`.
/// Breaks on spaces, hard-splits any single word longer than the line.
#[cfg(target_arch = "x86_64")]
fn draw_wrapped(x: u32, mut yy: u32, max_w: u32, y_limit: u32, s: &str, fg: u32, bg: u32) -> u32 {
    let maxch = (max_w / 8).max(1) as usize;
    let mut line = String::new();
    for word in s.split(' ') {
        if word.len() > maxch {
            if !line.is_empty() && yy + 8 <= y_limit {
                font8x8::draw_str(x, yy, line.as_bytes(), fg, bg);
                yy += LINE_H;
            }
            line.clear();
            let wb = word.as_bytes();
            let mut o = 0;
            while o < wb.len() {
                if yy + 8 > y_limit {
                    return yy;
                }
                let e = (o + maxch).min(wb.len());
                font8x8::draw_str(x, yy, &wb[o..e], fg, bg);
                yy += LINE_H;
                o = e;
            }
            continue;
        }
        let cand = if line.is_empty() { word.len() } else { line.len() + 1 + word.len() };
        if cand > maxch && !line.is_empty() {
            if yy + 8 <= y_limit {
                font8x8::draw_str(x, yy, line.as_bytes(), fg, bg);
                yy += LINE_H;
            }
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() && yy + 8 <= y_limit {
        font8x8::draw_str(x, yy, line.as_bytes(), fg, bg);
        yy += LINE_H;
    }
    yy
}

/// How many wrapped lines `s` needs at `max_w` -- for scrolling to the newest.
#[cfg(target_arch = "x86_64")]
fn wrapped_line_count(s: &str, max_w: u32) -> usize {
    let maxch = (max_w / 8).max(1) as usize;
    let mut lines = 0usize;
    let mut len = 0usize;
    for word in s.split(' ') {
        if word.len() > maxch {
            if len > 0 {
                lines += 1;
                len = 0;
            }
            lines += word.len().div_ceil(maxch);
            continue;
        }
        let cand = if len == 0 { word.len() } else { len + 1 + word.len() };
        if cand > maxch && len > 0 {
            lines += 1;
            len = 0;
        }
        len = if len == 0 { word.len() } else { len + 1 + word.len() };
    }
    if len > 0 {
        lines += 1;
    }
    lines.max(1)
}

#[cfg(target_arch = "x86_64")]
pub fn draw(x: u32, y: u32, w: u32, h: u32) {
    tick();
    clear_unread();
    let bg = theme::color(theme::SLOT_BG);
    let panel = theme::color(theme::SLOT_PANEL);
    let border = theme::color(theme::SLOT_PANEL_BORDER);
    let text = theme::color(theme::SLOT_TEXT);
    let dim = theme::color(theme::SLOT_DIM);
    let accent = theme::color(theme::SLOT_ACCENT);

    framebuffer::back_fill_rect(x, y, w, h, panel);

    // ---- Sidebar: conversations with round avatars ----
    framebuffer::back_fill_rect(x, y, SIDEBAR_W, h, bg);
    framebuffer::back_fill_rect(x + SIDEBAR_W, y, 1, h, border);
    font8x8::draw_str(x + 12, y + 12, b"DIRECT MESSAGES", dim, bg);

    // Bottom strip is the "you" panel (below), so stop the list above it.
    let self_h = 34u32;
    let list_bottom = (y + h).saturating_sub(self_h);
    let rows = selectable_rows();
    let sel = unsafe { SELECTED };
    for (i, (_is_friend, _idx, label)) in rows.iter().enumerate() {
        let ry = y + LIST_TOP + i as u32 * ROW_H;
        if ry + ROW_H > list_bottom {
            break;
        }
        let online = label.starts_with('*');
        let selected = sel == Some(i);
        let row_bg = if selected { border } else { bg };
        if selected {
            framebuffer::back_fill_rounded_rect(x + 4, ry + 3, SIDEBAR_W - 8, ROW_H - 6, 6, border);
        }
        let name = strip_marker(label);
        let acx = x + 24;
        let acy = ry + ROW_H / 2;
        draw_avatar(acx, acy, 13, 2, name.as_bytes());
        if online {
            framebuffer::back_fill_circle(acx + 10, acy + 10, 4, row_bg);
            framebuffer::back_fill_circle(acx + 10, acy + 10, 3, ONLINE_DOT);
        }
        let maxch = ((SIDEBAR_W - 48) / 8) as usize;
        let nb = name.as_bytes();
        font8x8::draw_str(
            x + 44,
            ry + ROW_H / 2 - 4,
            &nb[..nb.len().min(maxch)],
            if selected { text } else { dim },
            row_bg,
        );
    }

    // ---- "You" panel (bottom-left, like Discord's user area): shows your own
    // avatar + current handle, so `/nick` changes are visible at a glance. ----
    let me_name = local_handle();
    let sp_y = (y + h).saturating_sub(self_h);
    framebuffer::back_fill_rect(x, sp_y, SIDEBAR_W, self_h, border);
    draw_avatar(x + 20, sp_y + self_h / 2, 11, 2, me_name.as_bytes());
    let me_max = ((SIDEBAR_W - 44) / 8) as usize;
    let mnb = me_name.as_bytes();
    font8x8::draw_str(x + 38, sp_y + self_h / 2 - 8, &mnb[..mnb.len().min(me_max)], text, border);
    font8x8::draw_str(x + 38, sp_y + self_h / 2 + 2, b"you", dim, border);

    // ---- Chat pane ----
    let px = x + SIDEBAR_W + 1;
    let pw = w.saturating_sub(SIDEBAR_W + 1);

    // Header: selected peer's avatar + handle + status.
    framebuffer::back_fill_rect(px, y + HEADER_H, pw, 1, border);
    match sel {
        Some(s) if s < roster().friends.len() => {
            let fname = roster().friends[s].handle.clone();
            draw_avatar(px + 22, y + HEADER_H / 2, 12, 2, fname.as_bytes());
            font8x8::draw_str(px + 42, y + 10, fname.as_bytes(), text, panel);
            font8x8::draw_str(px + 42, y + 24, status().as_bytes(), dim, panel);
        },
        _ => {
            font8x8::draw_str(px + 16, y + HEADER_H / 2 - 4, b"Select a conversation to start chatting", dim, panel);
        },
    }

    // Message stream (word-wrapped, newest-anchored) + compose box.
    let compose_h = 30u32;
    let msg_x = px + 14;
    let msg_top = y + HEADER_H + 10;
    let msg_w = pw.saturating_sub(28);
    let msg_bottom = (y + h).saturating_sub(compose_h + 8);

    if let Some(s) = sel {
        if s < roster().friends.len() {
            let friend_lid = roster().friends[s].ling_id.clone();
            let friend_name = roster().friends[s].handle.clone();
            let me_name = local_handle();
            // Prefer the live conversation; otherwise fall back to the saved
            // (password-decrypted) history so past messages still show when the
            // peer is offline -- e.g. right after a reboot + sign-in.
            let lines = match unsafe {
                (&*&raw const CONVERSATIONS).iter().find(|c| c.peer == friend_lid)
            } {
                Some(convo) => convo.lines.clone(),
                None => history_for(&friend_lid),
            };
            if lines.is_empty() {
                font8x8::draw_str(msg_x, msg_top, b"Not connected -- press Enter to start a chat.", dim, panel);
            } else {
                // Anchor to the newest: count from the end until the window fills.
                let avail = ((msg_bottom.saturating_sub(msg_top)) / LINE_H) as usize;
                let body_w = msg_w.saturating_sub(26);
                let mut used = 0usize;
                let mut start = 0usize;
                for (i, (_, line)) in lines.iter().enumerate().rev() {
                    used += 1 + wrapped_line_count(line, body_w) + 1; // name + body + gap
                    if used > avail {
                        start = i + 1;
                        break;
                    }
                }
                let mut yy = msg_top;
                for (from_me, line) in lines[start..].iter() {
                    if yy + 8 > msg_bottom {
                        break;
                    }
                    let who = if *from_me { me_name.as_bytes() } else { friend_name.as_bytes() };
                    draw_avatar(msg_x + 9, yy + 8, 9, 1, who);
                    font8x8::draw_str(msg_x + 26, yy, who, accent, panel);
                    yy += LINE_H;
                    yy = draw_wrapped(msg_x + 26, yy, body_w, msg_bottom, line, text, panel);
                    yy += 6;
                }
            }
        }
    }

    // Compose box: rounded input, placeholder when empty.
    let cbx = px + 12;
    let cby = (y + h).saturating_sub(compose_h);
    let cbw = pw.saturating_sub(24);
    framebuffer::back_fill_rounded_rect(cbx, cby, cbw, compose_h - 6, 8, bg);
    let inner_w = ((cbw.saturating_sub(16)) / 8) as usize;
    let buf = unsafe { &(&*&raw const COMPOSE)[..COMPOSE_LEN] };
    if buf.is_empty() {
        let hint: &[u8] = match sel {
            Some(s) if s < roster().friends.len() => b"Message...   /ping  /nick <name>",
            _ => b"/nick <name>  /add <name>  -- or arrows + Enter to add a peer",
        };
        font8x8::draw_str(cbx + 8, cby + 8, &hint[..hint.len().min(inner_w)], dim, bg);
    } else {
        let shown = buf.len().min(inner_w);
        font8x8::draw_str(cbx + 8, cby + 8, &buf[buf.len() - shown..], text, bg);
    }
}
