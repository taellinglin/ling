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
use crate::drivers::{font8x8, framebuffer, mixer};
use crate::fs::lingfs;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use ling_messenger::identity::PublicIdentity;
use ling_messenger::{handshake, link, vault, wire, Identity, LingId, Roster, Session};

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
    typing_until_ms: u64, // the peer said "typing" recently: show it until then
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
static mut TYPING_OUTBOX: Vec<LingId> = Vec::new(); // peers to tell "typing..."
static mut LAST_TYPING_MS: u64 = 0; // at most one typing notice every 3 s

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

/// Is the identity usable now? Once it's been sealed (see `unlock_identity`),
/// it can only be opened by signing in -- until then the messenger stays
/// offline: no beacons, no handshakes, so nothing can be signed with it.
fn identity_ready() -> bool {
    unsafe {
        if (*(&raw const IDENTITY)).is_some() {
            return true;
        }
    }
    !store_read("identity", "messenger_identity").map(|v| vault::is_vault(&v)).unwrap_or(false)
}

/// The identity seed's vault "passphrase": a 32-byte key the keyring derives
/// for this purpose (from the account's master key, so it survives password
/// changes, and exists only while the account is signed in).
fn identity_vault_key() -> Option<[u8; 32]> {
    crate::keyring::key("messenger-identity")
}

/// The key is already high-entropy (the keyring stretched the password), so
/// the vault's own Argon2id runs at its lightest setting here.
const IDENTITY_VAULT_COST: vault::Cost = vault::Cost { m_kib: 256, t: 1, lanes: 1 };

/// At sign-in: open the sealed identity, or seal a plaintext one (an older
/// install, or one created before anyone signed in) so the disk never holds
/// the seed in the clear again. Also remembers the public LingId for display
/// while locked.
fn unlock_identity() {
    let Some(key) = identity_vault_key() else { return };
    let Some(stored) = store_read("identity", "messenger_identity") else {
        // first run: make one, sealed from the start
        let id = load_or_create_identity();
        unsafe { *(&raw mut IDENTITY) = Some(id) };
        return;
    };
    if vault::is_vault(&stored) {
        match vault::unwrap_seed(&stored, &key) {
            Some(seed) => {
                let id = Identity::from_seed(seed);
                remember_ling_id(&id);
                unsafe { *(&raw mut IDENTITY) = Some(id) };
            },
            None => crate::console_write(b"messenger: the identity vault did not open with this account's key\n"),
        }
    } else if stored.len() == 32 {
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&stored);
        if seal_seed(&seed, &key) {
            crate::console_write(b"messenger: identity sealed under the account keyring\n");
        }
        let id = Identity::from_seed(seed);
        remember_ling_id(&id);
        unsafe { *(&raw mut IDENTITY) = Some(id) };
    }
}

fn seal_seed(seed: &[u8; 32], key: &[u8; 32]) -> bool {
    let (Some(salt), Some(nonce)) = (fresh_bytes::<{ vault::SALT_LEN }>(), fresh_bytes::<{ vault::NONCE_LEN }>()) else {
        return false;
    };
    store_write("identity", "messenger_identity", &vault::wrap_seed(seed, key, salt, nonce, IDENTITY_VAULT_COST));
    true
}

/// The public LingId, kept in the clear for the UI while the vault is shut.
fn remember_ling_id(id: &Identity) {
    store_write("identity_pub", "messenger_identity_pub", id.ling_id().0.as_bytes());
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
    // sealed straight away when the keyring is open; in the clear only until
    // the first sign-in seals it (`unlock_identity`)
    match identity_vault_key() {
        Some(key) if seal_seed(&seed, &key) => {},
        _ => store_write("identity", "messenger_identity", &seed),
    }
    let id = Identity::from_seed(seed);
    remember_ling_id(&id);
    id
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
    pi.to_bytes()
}

fn pub_id_from_bytes(b: &[u8]) -> Option<PublicIdentity> {
    PublicIdentity::from_bytes(b)
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
    unlock_identity();
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
    if !identity_ready() {
        if let Some(b) = store_read("identity_pub", "messenger_identity_pub") {
            if let Ok(s) = core::str::from_utf8(&b) {
                return LingId(s.to_string());
            }
        }
        return LingId("locked".into());
    }
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
    if !identity_ready() {
        return;
    }
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
        // Typing notices the UI queued.
        let typing = unsafe { core::mem::take(&mut *&raw mut TYPING_OUTBOX) };
        for lid in typing {
            send_kind_to(&lid, wire::KIND_TYPING, &[]);
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
                                        convos[i].typing_until_ms = 0;
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
                            wire::KIND_TYPING => {
                                convos[i].typing_until_ms = timer::now_ms() + 4_500;
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
    my_ling_id().0
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
        convos.push(Conversation { peer, handle, rx, session, lines, since_ms: now, we_dialed, last_ping_ms: 0, dead: false, typing_until_ms: 0 });
    }
}

// -- Actions driven by the UI -----------------------------------------------

/// Your own `lingmsg://` friend link: your handle, your public identity and,
/// when the network is up, the address friends can dial.
fn my_link() -> String {
    let ip = netstack::self_ip();
    let addr = if ip == [0, 0, 0, 0] { String::new() } else { alloc::format!("{}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], MSG_TCP_PORT) };
    link::encode(&local_handle(), &addr, &identity().public())
}

/// Add a friend from a `lingmsg://` link (trust is whoever sent you the link --
/// compare the LingId shown, as for any new friend) and dial them if the link
/// carries an address.
fn add_from_link(text: &str) {
    let Some(f) = link::decode(text) else {
        unsafe { STATUS = "that isn't a valid lingmsg:// link (copy it whole and try /add again)" };
        return;
    };
    if f.public_identity == identity().public() {
        unsafe { STATUS = "that's your own link" };
        return;
    }
    let lid = f.ling_id();
    if roster().find_by_ling_id(&lid).is_none() {
        let mut handle = f.handle.trim().to_string();
        if handle.is_empty() || roster().friends.iter().any(|x| x.handle.eq_ignore_ascii_case(&handle)) {
            handle = alloc::format!("{} {}", if handle.is_empty() { "friend" } else { handle.as_str() }, &lid.0[..lid.0.len().min(6)]);
        }
        roster().add(handle, f.public_identity.clone());
        save_roster();
    }
    select_peer(&lid);
    match parse_ipv4(&f.addr) {
        Some(ip) => queue_connect(lid, ip),
        None => unsafe { STATUS = "friend added - they'll connect when they're on your LAN" },
    }
}

/// `a.b.c.d` or `a.b.c.d:port` (the port is always the chat port here).
fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let host = s.split(':').next()?;
    let mut out = [0u8; 4];
    let mut n = 0;
    for part in host.split('.') {
        if n == 4 {
            return None;
        }
        out[n] = part.parse().ok()?;
        n += 1;
    }
    if n == 4 { Some(out) } else { None }
}

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
        "link" => {
            crate::drivers::clipboard::set_str(&my_link());
            unsafe { STATUS = "your lingmsg:// friend link is on the clipboard - send it to a friend" };
        },
        "add" if arg.is_empty() || arg.starts_with(link::SCHEME) => {
            let text = if arg.is_empty() { core::str::from_utf8(crate::drivers::clipboard::get()).unwrap_or("").to_string() } else { arg.to_string() };
            add_from_link(&text);
        },
        "add" => {
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
        _ => unsafe { STATUS = "unknown command (try /nick /add /link /ping)" },
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

/// Task-side: one sealed frame of `kind` to an open conversation, if there is one.
fn send_kind_to(lid: &LingId, kind: u8, body: &[u8]) {
    let send = unsafe {
        let convos = &mut *&raw mut CONVERSATIONS;
        convos.iter_mut().find(|c| &c.peer == lid && !c.dead).map(|c| (c.handle, wire::seal(&mut c.session, kind, body)))
    };
    if let Some((handle, frame)) = send {
        let _ = send_framed(handle, &frame);
    }
}

/// UI-side: the user is typing to the selected friend -- tell them (at most every 3 s).
fn note_typing() {
    let Some(sel) = (unsafe { SELECTED }) else { return };
    let Some(lid) = roster().friends.get(sel).map(|f| f.ling_id.clone()) else { return };
    let now = timer::now_ms();
    unsafe {
        if now.wrapping_sub(LAST_TYPING_MS) < 3_000 {
            return;
        }
        if !(&*&raw const CONVERSATIONS).iter().any(|c| c.peer == lid && !c.dead) {
            return;
        }
        LAST_TYPING_MS = now;
        (&mut *&raw mut TYPING_OUTBOX).push(lid);
    }
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
        0x20..=0x7E => {
            unsafe {
                if COMPOSE_LEN < COMPOSE_MAX {
                    (&mut *&raw mut COMPOSE)[COMPOSE_LEN] = k;
                    COMPOSE_LEN += 1;
                }
            }
            if k != b'/' && unsafe { (&*&raw const COMPOSE)[0] } != b'/' {
                note_typing();
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
// -- Drawing ------------------------------------------------------------------
//
// Laid out like Discord -- a rail of friend tiles, a conversations column with
// your own panel at the foot, the chat (grouped by sender, a typing line, the
// composer), and a profile panel when the window is wide -- and skinned like
// LingStationX: square corners, hairline outlines, 45-degree hatching and
// corner brackets on what's selected, uppercase tile lettering for headings,
// and its palette (ROYGBIV for people, blue for "selected / primary", green for
// "on", red for danger).

#[cfg(target_arch = "x86_64")]
mod skin {
    pub const BG: u32 = 0x1B_1B_1B; // C_BG_PANEL
    pub const SIDE: u32 = 0x16_17_1A; // C_BG_SIDE
    pub const DEEP: u32 = 0x0A_0A_0A; // C_BG_DEEP
    pub const LINE: u32 = 0x3C_3C_3C; // C_LINE
    pub const LINE_SOFT: u32 = 0x2A_2C_32; // C_LINE_SOFT
    pub const TEXT: u32 = 0xCD_D0_D8; // C_TEXT
    pub const DIM: u32 = 0x8C_90_98; // C_TEXT_DIM
    pub const STRONG: u32 = 0xF5_F5_F5; // C_TEXT_STRONG
    pub const SEL_ROW: u32 = 0x26_34_4C; // C_SEL_ROW
    pub const BLUE: u32 = 0x3B_AA_EB; // C_BLUE: primary / selected
    pub const GREEN: u32 = 0x45_C8_70; // C_GREEN: on / online
    pub const RED: u32 = 0xED_4A_37; // C_RED
    pub const ROYGBIV: [u32; 7] = [0xED_4A_37, 0xF7_94_1E, 0xF7_D8_46, 0x45_C8_70, 0x3B_AA_EB, 0x4A_64_D8, 0x9A_53_D6];
}

#[cfg(target_arch = "x86_64")]
const RAIL_W: u32 = 56;
#[cfg(target_arch = "x86_64")]
const SIDEBAR_W: u32 = 208;
#[cfg(target_arch = "x86_64")]
const SIDE_HDR_H: u32 = 34; // the "DIRECT MESSAGES" header bar
#[cfg(target_arch = "x86_64")]
const SECTION_H: u32 = 20; // a "FRIENDS - 3" label row
#[cfg(target_arch = "x86_64")]
const ROW_H: u32 = 40; // a conversation row
#[cfg(target_arch = "x86_64")]
const SELF_H: u32 = 46; // your panel at the foot of the column
#[cfg(target_arch = "x86_64")]
const HEADER_H: u32 = 40; // the chat header
#[cfg(target_arch = "x86_64")]
const PROFILE_W: u32 = 220; // the right-hand profile panel (wide windows)
#[cfg(target_arch = "x86_64")]
const LINE_H: u32 = 12; // one text line (8px glyph + 4px leading)

/// Mix `a` toward `b` by `t`/256.
#[cfg(target_arch = "x86_64")]
fn mix(a: u32, b: u32, t: u32) -> u32 {
    let ch = |s: u32| -> u32 {
        let x = (a >> s) & 255;
        let y = (b >> s) & 255;
        ((x * (256 - t) + y * t) >> 8) & 255
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

#[cfg(target_arch = "x86_64")]
fn outline(x: u32, y: u32, w: u32, h: u32, c: u32) {
    if w == 0 || h == 0 {
        return;
    }
    framebuffer::back_fill_rect(x, y, w, 1, c);
    framebuffer::back_fill_rect(x, y + h - 1, w, 1, c);
    framebuffer::back_fill_rect(x, y, 1, h, c);
    framebuffer::back_fill_rect(x + w - 1, y, 1, h, c);
}

/// 45-degree hatching (LingStationX's `ds_hatch`): 1px lines every `sp` px,
/// `col` mixed into `bg` by `a`/256.
#[cfg(target_arch = "x86_64")]
fn hatch(x: u32, y: u32, w: u32, h: u32, bg: u32, col: u32, sp: u32, a: u32) {
    let c = mix(bg, col, a);
    for yy in 0..h {
        let mut xx = (sp - (yy % sp)) % sp;
        while xx < w {
            framebuffer::back_set_pixel(x + xx, y + yy, c);
            xx += sp;
        }
    }
}

/// Corner brackets (`ds_brackets`): an L of `n` px at each corner.
#[cfg(target_arch = "x86_64")]
fn brackets(x: u32, y: u32, w: u32, h: u32, n: u32, c: u32) {
    if w < 2 || h < 2 {
        return;
    }
    let n = n.min(w / 2).min(h / 2);
    let (r, b) = (x + w - 1, y + h - 1);
    framebuffer::back_fill_rect(x, y, n, 1, c);
    framebuffer::back_fill_rect(x, y, 1, n, c);
    framebuffer::back_fill_rect(r + 1 - n, y, n, 1, c);
    framebuffer::back_fill_rect(r, y, 1, n, c);
    framebuffer::back_fill_rect(x, b, n, 1, c);
    framebuffer::back_fill_rect(x, b + 1 - n, 1, n, c);
    framebuffer::back_fill_rect(r + 1 - n, b, n, 1, c);
    framebuffer::back_fill_rect(r, b + 1 - n, 1, n, c);
}

/// Uppercase tile lettering (`ds_word`): each letter in its own outlined square.
#[cfg(target_arch = "x86_64")]
fn tile_word(x: u32, y: u32, s: &str, col: u32, bg: u32) -> u32 {
    let mut cx = x;
    for b in s.bytes() {
        if b == b' ' {
            cx += 6;
            continue;
        }
        outline(cx, y, 12, 12, mix(bg, col, 110));
        font8x8::draw_char(cx + 2, y + 2, b.to_ascii_uppercase(), col, bg);
        cx += 13;
    }
    cx
}

/// Plain text cut to `max_w` px (8 px a character, at a character boundary).
#[cfg(target_arch = "x86_64")]
fn text_fit(x: u32, y: u32, s: &str, max_w: u32, fg: u32, bg: u32) {
    let maxch = (max_w / 8) as usize;
    let b = s.as_bytes();
    if b.len() <= maxch {
        font8x8::draw_str(x, y, b, fg, bg);
    } else if maxch > 2 {
        let mut end = maxch - 2;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        font8x8::draw_str(x, y, &b[..end], fg, bg);
        font8x8::draw_str(x + end as u32 * 8, y, b"..", fg, bg);
    }
}

/// A person's colour: their handle hashed into the ROYGBIV palette.
#[cfg(target_arch = "x86_64")]
fn person_color(seed: &[u8]) -> u32 {
    let mut hsh: u32 = 2166136261;
    for &b in seed {
        hsh = (hsh ^ b as u32).wrapping_mul(16777619);
    }
    skin::ROYGBIV[(hsh as usize) % skin::ROYGBIV.len()]
}

/// A square avatar: the person's colour, hatched, outlined, their initial.
#[cfg(target_arch = "x86_64")]
fn draw_avatar(x: u32, y: u32, size: u32, name: &[u8], bg: u32) {
    let col = person_color(name);
    let fill = mix(bg, col, 70);
    framebuffer::back_fill_rect(x, y, size, size, fill);
    hatch(x + 1, y + 1, size - 2, size - 2, fill, col, 4, 90);
    outline(x, y, size, size, col);
    let initial = name.iter().copied().find(|c| c.is_ascii_alphanumeric()).unwrap_or(b'?').to_ascii_uppercase();
    let scale = if size >= 28 { 2 } else { 1 };
    let g = 4 * scale;
    framebuffer::back_fill_rect(x + size / 2 - g - 1, y + size / 2 - g - 1, 2 * g + 2, 2 * g + 2, fill);
    font8x8::draw_char_scaled(x + size / 2 - g, y + size / 2 - g, initial, skin::STRONG, fill, scale);
}

/// A presence pip at a corner of an avatar.
#[cfg(target_arch = "x86_64")]
fn presence(x: u32, y: u32, online: bool, bg: u32) {
    framebuffer::back_fill_rect(x, y, 8, 8, bg);
    if online {
        framebuffer::back_fill_rect(x + 1, y + 1, 6, 6, skin::GREEN);
    } else {
        outline(x + 1, y + 1, 6, 6, skin::DIM);
    }
}

/// Strip the leading presence marker (`*`/`+`/space) a row label carries.
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

/// A system line (a ping, a rename) rather than something someone typed.
fn is_system_line(s: &str) -> bool {
    s == "*ping*" || (s.starts_with("* ") && s.ends_with(" *"))
}

/// Where each conversation row sits in the column: `(row index, y offset from
/// the column top)`, friends under a FRIENDS label, nearby peers under NEARBY.
#[cfg(target_arch = "x86_64")]
fn row_layout(rows: &[(bool, usize, String)]) -> Vec<(usize, u32)> {
    let mut out = Vec::new();
    let mut y = SIDE_HDR_H + 6;
    let friends = rows.iter().filter(|r| r.0).count();
    if friends > 0 {
        y += SECTION_H;
    }
    let mut nearby_started = false;
    for (i, r) in rows.iter().enumerate() {
        if !r.0 && !nearby_started {
            nearby_started = true;
            y += 6 + SECTION_H;
        }
        out.push((i, y));
        y += ROW_H;
    }
    out
}

/// A click in the Messenger window, relative to the content's top-left (the
/// same origin `draw` gets). Rail tiles and conversation rows select.
pub fn click(rel_x: i64, rel_y: i64) {
    #[cfg(target_arch = "x86_64")]
    {
        if rel_x < 0 || rel_y < 0 {
            return;
        }
        let (rx, ry) = (rel_x as u32, rel_y as u32);
        let rows = selectable_rows();
        if rx < RAIL_W {
            // the rail: home at the top, then one tile per friend
            if ry < 52 {
                unsafe { SELECTED = None };
                return;
            }
            let i = ((ry - 60) / 44) as usize;
            if (ry - 60) % 44 < 36 && i < roster().friends.len() {
                click_row(i);
            }
            return;
        }
        if rx < RAIL_W + SIDEBAR_W {
            for (i, top) in row_layout(&rows) {
                if ry >= top && ry < top + ROW_H {
                    click_row(i);
                    return;
                }
            }
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (rel_x, rel_y);
}

/// The last thing said in a conversation, for the row's preview line.
#[cfg(target_arch = "x86_64")]
fn last_line(lid: &LingId) -> Option<(bool, String)> {
    let live = unsafe { (&*&raw const CONVERSATIONS).iter().find(|c| &c.peer == lid).and_then(|c| c.lines.last().cloned()) };
    live.or_else(|| history_for(lid).last().cloned())
}

#[cfg(target_arch = "x86_64")]
pub fn draw(x: u32, y: u32, w: u32, h: u32) {
    use skin::*;
    tick();
    clear_unread();
    let now = timer::now_ms();
    framebuffer::back_fill_rect(x, y, w, h, BG);
    let rows = selectable_rows();
    let sel = unsafe { SELECTED };
    let me_name = local_handle();
    let my_id = my_ling_id();
    let n_friends = roster().friends.len();

    // ---- the rail ----
    framebuffer::back_fill_rect(x, y, RAIL_W, h, DEEP);
    framebuffer::back_fill_rect(x + RAIL_W - 1, y, 1, h, LINE_SOFT);
    {
        // home: the direct-messages view
        let (tx, ty) = (x + 10, y + 10);
        let home = sel.is_none();
        framebuffer::back_fill_rect(tx, ty, 36, 36, mix(DEEP, BLUE, 60));
        hatch(tx + 1, ty + 1, 34, 34, mix(DEEP, BLUE, 60), BLUE, 4, if home { 150 } else { 70 });
        outline(tx, ty, 36, 36, BLUE);
        if home {
            brackets(tx - 4, ty - 4, 44, 44, 6, BLUE);
        }
        framebuffer::back_fill_rect(tx + 9, ty + 13, 18, 10, mix(DEEP, BLUE, 60));
        font8x8::draw_str(tx + 10, ty + 14, b"LM", STRONG, mix(DEEP, BLUE, 60));
        framebuffer::back_fill_rect(x + 14, y + 54, RAIL_W - 28, 1, LINE);
        // one tile per friend
        let mut ty = y + 60;
        for (i, f) in roster().friends.iter().enumerate() {
            if ty + 36 > (y + h).saturating_sub(48) {
                break;
            }
            let online = unsafe { (&*&raw const NEARBY).iter().any(|p| p.ling_id == f.ling_id) };
            draw_avatar(x + 10, ty, 36, f.handle.as_bytes(), DEEP);
            presence(x + 38, ty + 28, online, DEEP);
            if sel == Some(i) {
                framebuffer::back_fill_rect(x, ty + 4, 3, 28, STRONG);
                brackets(x + 6, ty - 4, 44, 44, 6, BLUE);
            }
            ty += 44;
        }
        // add a friend: the "+" tile (the hint says how)
        let ay = (y + h).saturating_sub(46);
        outline(x + 10, ay, 36, 36, GREEN);
        hatch(x + 11, ay + 1, 34, 34, DEEP, GREEN, 5, 60);
        framebuffer::back_fill_rect(x + 23, ay + 13, 10, 10, DEEP);
        font8x8::draw_char(x + 24, ay + 14, b'+', GREEN, DEEP);
    }

    // ---- the conversations column ----
    let sx = x + RAIL_W;
    framebuffer::back_fill_rect(sx, y, SIDEBAR_W, h, SIDE);
    framebuffer::back_fill_rect(sx + SIDEBAR_W - 1, y, 1, h, LINE_SOFT);
    framebuffer::back_fill_rect(sx, y + SIDE_HDR_H - 1, SIDEBAR_W, 1, LINE);
    tile_word(sx + 12, y + 11, "DIRECT MESSAGES", TEXT, SIDE);
    let list_bottom = (y + h).saturating_sub(SELF_H);
    let n_near = rows.len() - n_friends.min(rows.len());
    let layout = row_layout(&rows);
    if n_friends > 0 {
        let ly = y + SIDE_HDR_H + 6;
        font8x8::draw_str(sx + 12, ly + 6, alloc::format!("FRIENDS - {n_friends}").as_bytes(), DIM, SIDE);
    }
    if n_near > 0 {
        if let Some(&(_, top)) = layout.iter().find(|(i, _)| !rows[*i].0) {
            let ly = y + top - SECTION_H;
            if ly + 14 < list_bottom {
                font8x8::draw_str(sx + 12, ly + 6, alloc::format!("NEARBY - {n_near}").as_bytes(), DIM, SIDE);
            }
        }
    }
    if rows.is_empty() {
        let ly = y + SIDE_HDR_H + 16;
        font8x8::draw_str(sx + 12, ly, b"no one here yet", DIM, SIDE);
        font8x8::draw_str(sx + 12, ly + 14, b"peers on this LAN show", DIM, SIDE);
        font8x8::draw_str(sx + 12, ly + 26, b"up under NEARBY; or /add", DIM, SIDE);
        font8x8::draw_str(sx + 12, ly + 38, b"a lingmsg:// link", DIM, SIDE);
    }
    for (i, top) in &layout {
        let (is_friend, idx, label) = &rows[*i];
        let ry = y + top;
        if ry + ROW_H > list_bottom {
            break;
        }
        let selected = sel == Some(*i);
        let rbg = if selected { SEL_ROW } else { SIDE };
        if selected {
            framebuffer::back_fill_rect(sx + 6, ry + 2, SIDEBAR_W - 12, ROW_H - 4, SEL_ROW);
            hatch(sx + 7, ry + 3, SIDEBAR_W - 14, ROW_H - 6, SEL_ROW, BLUE, 5, 40);
            outline(sx + 6, ry + 2, SIDEBAR_W - 12, ROW_H - 4, mix(SEL_ROW, BLUE, 150));
            brackets(sx + 6, ry + 2, SIDEBAR_W - 12, ROW_H - 4, 5, BLUE);
        }
        let name = strip_marker(label);
        let online = label.starts_with('*') || !*is_friend;
        draw_avatar(sx + 14, ry + 8, 24, name.as_bytes(), rbg);
        presence(sx + 33, ry + 27, online, rbg);
        text_fit(sx + 46, ry + 9, name, SIDEBAR_W - 56, if selected { STRONG } else { TEXT }, rbg);
        // the second line: the last thing said, or how they're reachable
        let sub: String = if *is_friend {
            match roster().friends.get(*idx).and_then(|f| last_line(&f.ling_id)) {
                Some((mine, t)) if !is_system_line(&t) => if mine { alloc::format!("you: {t}") } else { t },
                _ => String::from(if online { "online" } else { "offline" }),
            }
        } else {
            String::from("on your LAN - Enter adds")
        };
        let sub_col = if !*is_friend { GREEN } else { DIM };
        text_fit(sx + 46, ry + 22, &sub, SIDEBAR_W - 56, sub_col, rbg);
    }

    // ---- you, at the foot of the column ----
    {
        let py = (y + h).saturating_sub(SELF_H);
        framebuffer::back_fill_rect(sx, py, SIDEBAR_W - 1, SELF_H, DEEP);
        framebuffer::back_fill_rect(sx, py, SIDEBAR_W - 1, 1, LINE);
        draw_avatar(sx + 10, py + 9, 28, me_name.as_bytes(), DEEP);
        presence(sx + 31, py + 30, identity_ready(), DEEP);
        text_fit(sx + 46, py + 10, &me_name, SIDEBAR_W - 56, STRONG, DEEP);
        let short = &my_id.0[..my_id.0.len().min(12)];
        font8x8::draw_str(sx + 46, py + 24, short.as_bytes(), DIM, DEEP);
        outline(sx + SIDEBAR_W - 46, py + 11, 34, 24, LINE);
        font8x8::draw_str(sx + SIDEBAR_W - 41, py + 19, b"/lnk", BLUE, DEEP);
    }

    // ---- the chat ----
    let cx = sx + SIDEBAR_W;
    let total_w = w.saturating_sub(RAIL_W + SIDEBAR_W);
    let show_profile = total_w >= 640;
    let cw = if show_profile { total_w - PROFILE_W } else { total_w };
    let friend = match sel {
        Some(s) if s < n_friends => Some(s),
        _ => None,
    };

    // header
    framebuffer::back_fill_rect(cx, y + HEADER_H - 1, cw, 1, LINE);
    let typing_name: Option<String>;
    match friend {
        Some(s) => {
            let f = &roster().friends[s];
            let online = unsafe { (&*&raw const NEARBY).iter().any(|p| p.ling_id == f.ling_id) };
            let live = unsafe { (&*&raw const CONVERSATIONS).iter().any(|c| c.peer == f.ling_id && !c.dead) };
            font8x8::draw_str(cx + 14, y + 16, b"@", DIM, BG);
            text_fit(cx + 28, y + 16, &f.handle, cw / 3, STRONG, BG);
            let after = cx + 28 + (f.handle.len().min((cw / 3 / 8) as usize) as u32) * 8 + 12;
            framebuffer::back_fill_rect(after, y + 12, 1, 16, LINE);
            let state = if live { "connected - end-to-end encrypted" } else if online { "online - press Enter to connect" } else { "offline" };
            text_fit(after + 10, y + 16, state, cw.saturating_sub(after - cx + 24), if live { GREEN } else { DIM }, BG);
            typing_name = unsafe {
                (&*&raw const CONVERSATIONS).iter().find(|c| c.peer == f.ling_id && c.typing_until_ms > now).map(|_| f.handle.clone())
            };
        },
        None => {
            tile_word(cx + 14, y + 14, "LING MESSENGER", TEXT, BG);
            typing_name = None;
        },
    }

    // the composer, at the foot
    let comp_h = 36u32;
    let comp_y = (y + h).saturating_sub(comp_h + 14);
    let comp_x = cx + 14;
    let comp_w = cw.saturating_sub(28);
    framebuffer::back_fill_rect(comp_x, comp_y, comp_w, comp_h, DEEP);
    outline(comp_x, comp_y, comp_w, comp_h, LINE);
    brackets(comp_x, comp_y, comp_w, comp_h, 6, if friend.is_some() { BLUE } else { LINE });
    framebuffer::back_fill_rect(comp_x + 10, comp_y + 9, 18, 18, mix(DEEP, GREEN, 50));
    outline(comp_x + 10, comp_y + 9, 18, 18, GREEN);
    font8x8::draw_char(comp_x + 15, comp_y + 14, b'+', GREEN, mix(DEEP, GREEN, 50));
    let inner_x = comp_x + 38;
    let inner_w = comp_w.saturating_sub(52);
    let buf = unsafe { &(&*&raw const COMPOSE)[..COMPOSE_LEN] };
    if buf.is_empty() {
        let hint = match friend {
            Some(s) => alloc::format!("Message @{}   /ping  /nick  /link", roster().friends[s].handle),
            None => String::from("/nick <name>   /add <name | lingmsg:// link>   /link"),
        };
        text_fit(inner_x, comp_y + 14, &hint, inner_w, DIM, DEEP);
    } else {
        let maxch = (inner_w / 8) as usize;
        let shown = buf.len().min(maxch.saturating_sub(1));
        font8x8::draw_str(inner_x, comp_y + 14, &buf[buf.len() - shown..], STRONG, DEEP);
        if (now / 500) % 2 == 0 {
            framebuffer::back_fill_rect(inner_x + shown as u32 * 8 + 1, comp_y + 12, 2, 12, BLUE);
        }
    }
    // the typing line just above it
    let typing_y = comp_y.saturating_sub(16);
    if let Some(n) = &typing_name {
        let dots = [b".  ", b".. ", b"..."][((now / 400) % 3) as usize];
        let mut s = alloc::format!("{n} is typing");
        s.push_str(core::str::from_utf8(dots).unwrap_or(""));
        text_fit(cx + 16, typing_y, &s, cw.saturating_sub(32), BLUE, BG);
    }

    // the messages: grouped by sender, newest at the bottom
    let msg_top = y + HEADER_H + 10;
    let msg_bottom = typing_y.saturating_sub(6);
    let msg_x = cx + 16;
    let body_x = msg_x + 40;
    let body_w = cw.saturating_sub(40 + 40);
    match friend {
        Some(s) => {
            let f_lid = roster().friends[s].ling_id.clone();
            let f_name = roster().friends[s].handle.clone();
            let lines = match unsafe { (&*&raw const CONVERSATIONS).iter().find(|c| c.peer == f_lid) } {
                Some(c) => c.lines.clone(),
                None => history_for(&f_lid),
            };
            if lines.is_empty() {
                // the start of the conversation, Discord-style
                draw_avatar(msg_x, msg_top + 8, 56, f_name.as_bytes(), BG);
                font8x8::draw_char_scaled(msg_x, msg_top + 76, b'@', STRONG, BG, 2);
                text_fit(msg_x + 18, msg_top + 80, &f_name, cw.saturating_sub(60), STRONG, BG);
                font8x8::draw_str(msg_x, msg_top + 104, b"This is the beginning of your direct messages.", DIM, BG);
                font8x8::draw_str(msg_x, msg_top + 118, b"Press Enter with an empty message to connect.", DIM, BG);
            } else {
                // the height of each line as drawn (a header row when the sender changes)
                let avail = msg_bottom.saturating_sub(msg_top);
                let mut need = 0u32;
                let mut start = 0usize;
                for i in (0..lines.len()).rev() {
                    let (mine, t) = &lines[i];
                    let head = i == 0 || lines[i - 1].0 != *mine || is_system_line(&lines[i - 1].1) || is_system_line(t);
                    let hgt = if is_system_line(t) { LINE_H + 8 } else { wrapped_line_count(t, body_w) as u32 * LINE_H + if head { LINE_H + 10 } else { 2 } };
                    if need + hgt > avail {
                        start = i + 1;
                        break;
                    }
                    need += hgt;
                }
                let mut yy = msg_top + avail.saturating_sub(need);
                for i in start..lines.len() {
                    let (mine, t) = &lines[i];
                    if is_system_line(t) {
                        // a ping or a rename: a quiet centred note between hairlines
                        let mid = yy + 6;
                        framebuffer::back_fill_rect(msg_x, mid + 3, cw.saturating_sub(32), 1, LINE_SOFT);
                        let label = if t == "*ping*" { String::from("pinged you") } else { t.trim_matches('*').trim().to_string() };
                        let tw = (label.len() as u32 * 8 + 16).min(cw.saturating_sub(40));
                        let lx = cx + (cw - tw) / 2;
                        framebuffer::back_fill_rect(lx, mid - 2, tw, 12, BG);
                        text_fit(lx + 8, mid, &label, tw - 8, if t == "*ping*" { RED } else { DIM }, BG);
                        yy += LINE_H + 8;
                        continue;
                    }
                    let head = i == 0 || lines[i - 1].0 != *mine || is_system_line(&lines[i - 1].1);
                    let who = if *mine { me_name.as_str() } else { f_name.as_str() };
                    if head {
                        yy += 8;
                        draw_avatar(msg_x, yy, 28, who.as_bytes(), BG);
                        let ncol = if *mine { BLUE } else { person_color(who.as_bytes()) };
                        text_fit(body_x, yy + 1, who, body_w, ncol, BG);
                        yy += LINE_H + 2;
                    }
                    yy = draw_wrapped(body_x, yy, body_w, msg_bottom + LINE_H, t, TEXT, BG);
                    yy += 2;
                }
            }
        },
        None => {
            // nothing selected: a framed welcome card with your identity and the commands
            let fw = cw.saturating_sub(60).min(520);
            let fx = cx + (cw - fw) / 2;
            let fy = msg_top + 20;
            let fh = 190u32;
            outline(fx, fy, fw, fh, LINE);
            outline(fx + 3, fy + 3, fw - 6, fh - 6, LINE_SOFT);
            brackets(fx, fy, fw, fh, 10, BLUE);
            framebuffer::back_fill_rect(fx + 16, fy - 7, 13 * 7 + 10, 14, BG);
            tile_word(fx + 20, fy - 6, "WELCOME", BLUE, BG);
            font8x8::draw_str(fx + 20, fy + 22, b"Your LingId -- compare it with friends out of band:", DIM, BG);
            text_fit(fx + 20, fy + 38, &my_id.0, fw - 40, STRONG, BG);
            let tips: [&[u8]; 6] = [
                b"Up / Down + Enter   chat with a friend, or add a nearby peer",
                b"/link               copy your lingmsg:// friend link",
                b"/add                add the friend link on the clipboard",
                b"/nick <name>        change your name     /ping   nudge them",
                b"post-quantum hybrid crypto; a fresh key for every message",
                b"",
            ];
            let mut ty = fy + 62;
            for t in tips {
                let n = t.len().min(((fw - 40) / 8) as usize);
                font8x8::draw_str(fx + 20, ty, &t[..n], TEXT, BG);
                ty += 16;
            }
            text_fit(fx + 20, fy + fh - 22, status(), fw - 40, DIM, BG);
        },
    }

    // ---- the profile panel ----
    if show_profile {
        let px = cx + cw;
        framebuffer::back_fill_rect(px, y, PROFILE_W, h, SIDE);
        framebuffer::back_fill_rect(px, y, 1, h, LINE_SOFT);
        match friend {
            Some(s) => {
                let f = &roster().friends[s];
                let online = unsafe { (&*&raw const NEARBY).iter().any(|p| p.ling_id == f.ling_id) };
                let col = person_color(f.handle.as_bytes());
                // a banner in their colour, the avatar overlapping it
                framebuffer::back_fill_rect(px + 1, y, PROFILE_W - 1, 56, mix(SIDE, col, 60));
                hatch(px + 1, y, PROFILE_W - 1, 56, mix(SIDE, col, 60), col, 6, 70);
                framebuffer::back_fill_rect(px + 14, y + 30, 60, 60, SIDE);
                draw_avatar(px + 16, y + 32, 56, f.handle.as_bytes(), SIDE);
                presence(px + 66, y + 82, online, SIDE);
                text_fit(px + 16, y + 100, &f.handle, PROFILE_W - 32, STRONG, SIDE);
                font8x8::draw_str(px + 16, y + 114, if online { b"online" as &[u8] } else { b"offline" }, if online { GREEN } else { DIM }, SIDE);
                framebuffer::back_fill_rect(px + 16, y + 132, PROFILE_W - 32, 1, LINE);
                tile_word(px + 16, y + 142, "LINGID", TEXT, SIDE);
                draw_wrapped(px + 16, y + 162, PROFILE_W - 32, y + 200, &f.ling_id.0, DIM, SIDE);
                framebuffer::back_fill_rect(px + 16, y + 206, PROFILE_W - 32, 1, LINE);
                tile_word(px + 16, y + 216, "SECURITY", TEXT, SIDE);
                let lines: [&[u8]; 5] = [b"Ed25519 + ML-DSA-87 id", b"X25519 + ML-KEM-768", b"new key every message", b"late frames: 256 ahead", b"history sealed at rest"];
                let mut ly = y + 236;
                for l in lines {
                    framebuffer::back_fill_rect(px + 16, ly + 2, 4, 4, GREEN);
                    font8x8::draw_str(px + 26, ly, l, TEXT, SIDE);
                    ly += 14;
                }
            },
            None => {
                tile_word(px + 16, y + 14, "ACTIVE NOW", TEXT, SIDE);
                let mut ly = y + 40;
                let mut any = false;
                for f in roster().friends.iter() {
                    let online = unsafe { (&*&raw const NEARBY).iter().any(|p| p.ling_id == f.ling_id) };
                    if !online || ly + 30 > y + h {
                        continue;
                    }
                    any = true;
                    draw_avatar(px + 16, ly, 24, f.handle.as_bytes(), SIDE);
                    presence(px + 35, ly + 19, true, SIDE);
                    text_fit(px + 48, ly + 8, &f.handle, PROFILE_W - 64, TEXT, SIDE);
                    ly += 32;
                }
                if !any {
                    font8x8::draw_str(px + 16, ly, b"It's quiet for now...", DIM, SIDE);
                    font8x8::draw_str(px + 16, ly + 14, b"friends online on your", DIM, SIDE);
                    font8x8::draw_str(px + 16, ly + 26, b"LAN show up here.", DIM, SIDE);
                }
            },
        }
    }
}
