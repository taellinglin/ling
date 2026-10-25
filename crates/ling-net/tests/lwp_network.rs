#![cfg(feature = "lwp")]
#[path = "lwp_common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use common::*;
use ling_net::lwp::crypto::hash::h256;
use ling_net::lwp::crypto::identity::IdentityKey;
use ling_net::lwp::*;

const S: Duration = Duration::from_secs(1);

fn msg(rx: &crossbeam_channel::Receiver<NetEvent>, kind: u8, d: Duration) -> Option<(PeerId, Vec<u8>)> {
    wait_for(rx, d, |e| match e {
        NetEvent::Message { peer, kind: k, payload } if *k == kind => Some((*peer, payload.clone())),
        _ => None,
    })
}

#[test]
fn handshake_and_messages_both_directions() {
    let (a, arx) = node(Opts::default());
    let (b, brx) = node(Opts { manual: vec![addr(&a)], listen: false, ..Opts::default() });
    let pa = connected(&brx, 15 * S).expect("b connects to a");
    let pb = connected(&arx, 15 * S).expect("a sees b");
    assert!(!pa.inbound && pb.inbound);
    assert_eq!(b.peers().len(), 1);

    b.send(pa.id, 0x20, b"hello a".to_vec());
    assert_eq!(msg(&arx, 0x20, 10 * S).unwrap(), (pb.id, b"hello a".to_vec()));
    a.send(pb.id, 0x21, b"hello b".to_vec());
    assert_eq!(msg(&brx, 0x21, 10 * S).unwrap(), (pa.id, b"hello b".to_vec()));

    let big: Vec<u8> = (0..3 * 1024 * 1024).map(|i: u32| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    a.send(pb.id, 0x22, big.clone());
    let got = msg(&brx, 0x22, 30 * S).expect("3 MiB payload");
    assert_eq!(got.1.len(), big.len());
    assert!(got.1 == big);
    b.send(pa.id, 0x23, big.clone());
    assert!(msg(&arx, 0x23, 30 * S).unwrap().1 == big);

    // Control kinds and oversize payloads are refused locally.
    b.send(pa.id, 0x05, b"x".to_vec());
    b.send(pa.id, 0x24, vec![0; MAX_PAYLOAD + 1]);
    b.send(pa.id, 0x25, b"after".to_vec());
    assert_eq!(msg(&arx, 0x25, 10 * S).unwrap().1, b"after");
    assert!(b.peers()[0].bytes_out > 3 * 1024 * 1024);

    b.shutdown();
    assert!(wait_for(&arx, 15 * S, |e| matches!(e, NetEvent::Disconnected(id) if *id == pb.id).then_some(())).is_some());
    a.shutdown();
}

#[test]
fn wire_frames_are_bucket_padded() {
    let (a, arx) = node(Opts::default());
    let t = tap(a.local_addr().unwrap(), None);
    let (b, brx) = node(Opts { manual: vec![t.addr.to_string()], listen: false, ..Opts::default() });
    let pa = connected(&brx, 15 * S).unwrap();
    let pb = connected(&arx, 15 * S).unwrap();
    let sizes = [0usize, 1, 100, 1019, 1020, 2000, 70_000, 300_000, 1_100_000];
    for (i, &n) in sizes.iter().enumerate() {
        b.send(pa.id, 0x30, vec![i as u8; n]);
        assert_eq!(msg(&arx, 0x30, 20 * S).unwrap().1.len(), n);
    }
    a.send(pb.id, 0x31, vec![7; 5000]);
    assert!(msg(&brx, 0x31, 10 * S).is_some());
    b.shutdown();
    a.shutdown();

    let c2s = t.c2s.lock().unwrap().clone();
    let s2c = t.s2c.lock().unwrap().clone();
    assert_eq!(&c2s[..4], b"LWP1");
    let hs_out = HANDSHAKE_MSG1_LEN + HANDSHAKE_MSG3_LEN;
    let up = frame_lengths(&c2s[hs_out..]);
    let down = frame_lengths(&s2c[HANDSHAKE_MSG2_LEN..]);
    assert!(up.len() >= sizes.len(), "{up:?}");
    assert!(!down.is_empty());
    let valid: Vec<usize> = FRAME_BUCKETS.iter().map(|b| b + FRAME_OVERHEAD).collect();
    for l in up.iter().chain(down.iter()) {
        assert!(valid.contains(l), "frame length {l} is not a bucket size");
    }
    // Each payload landed in the smallest bucket that fits it.
    for &n in &sizes {
        let want = FRAME_BUCKETS.iter().find(|&&b| b >= n + 5).unwrap() + FRAME_OVERHEAD;
        assert!(up.contains(&want), "no frame of {want} for payload {n}");
    }
    // Ciphertext must not contain the plaintext pattern.
    assert!(!c2s.windows(64).any(|w| w.iter().all(|&x| x == 7)));
}

#[test]
fn tampered_frame_kills_connection() {
    let (a, arx) = node(Opts::default());
    // Flip a byte inside the initiator's first encrypted frame.
    let t = tap(a.local_addr().unwrap(), Some(HANDSHAKE_MSG1_LEN + HANDSHAKE_MSG3_LEN + 200));
    let (b, brx) = node(Opts { manual: vec![t.addr.to_string()], listen: false, ..Opts::default() });
    let pa = connected(&brx, 15 * S).expect("handshake itself is untouched");
    let gone = wait_for(&brx, 15 * S, |e| matches!(e, NetEvent::Disconnected(id) if *id == pa.id).then_some(()));
    assert!(gone.is_some(), "tampered frame must close the connection");
    // No message ever surfaced from the tampered stream on A's side.
    assert!(wait_for(&arx, S / 2, |e| matches!(e, NetEvent::Message { .. }).then_some(())).is_none());
    b.shutdown();
    a.shutdown();
}

#[test]
fn seed_identity_is_pinned() {
    let a_id = IdentityKey::generate();
    let a_pub = a_id.public();
    let (a, _arx) = node(Opts { identity: Some(a_id), ..Opts::default() });
    let wrong = SeedPeer { addr: addr(&a), identity: IdentityKey::generate().public() };
    let (b, brx) = node(Opts { seeds: vec![wrong], listen: false, ..Opts::default() });
    assert!(connected(&brx, 4 * S).is_none(), "seed with wrong identity must be rejected");
    b.shutdown();

    let right = SeedPeer { addr: addr(&a), identity: a_pub.clone() };
    let (c, crx) = node(Opts { seeds: vec![right], listen: false, ..Opts::default() });
    let p = connected(&crx, 15 * S).expect("correct pin connects");
    assert!(p.is_seed);
    assert_eq!(p.identity_fingerprint, a_pub.fingerprint());
    c.shutdown();
    a.shutdown();
}

#[test]
fn self_connection_is_refused() {
    // A node whose manual peer is its own listen address (ephemeral outbound
    // identity, so only the handshake-nonce check can catch it).
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let cfg = NetConfig {
        network_magic: MAGIC,
        network: 9,
        listen: Some(format!("127.0.0.1:{port}").parse().unwrap()),
        data_dir: tmpdir(),
        seeds: vec![],
        socks5_proxy: None,
        max_outbound: 8,
        max_inbound: 8,
        identity: IdentityKey::generate(),
        ephemeral_outbound_identity: true,
        manual_peers: vec![format!("127.0.0.1:{port}")],
    };
    let (n, rx) = Network::start_tuned(cfg, Tuning::fast_for_tests()).unwrap();
    assert!(connected(&rx, 3 * S).is_none(), "must not connect to itself");
    n.shutdown();
}

#[test]
fn ban_disconnects_and_blocks_reconnect() {
    let (a, arx) = node(Opts::default());
    let (b, brx) = node(Opts { manual: vec![addr(&a)], listen: false, ephemeral: false, ..Opts::default() });
    let pb = connected(&arx, 15 * S).unwrap();
    let pa = connected(&brx, 15 * S).unwrap();
    a.misbehaving(pb.id, 60, "test");
    thread::sleep(S / 2);
    assert_eq!(a.peers().len(), 1, "below threshold stays connected");
    a.misbehaving(pb.id, 40, "test");
    assert!(wait_for(&arx, 10 * S, |e| matches!(e, NetEvent::Disconnected(id) if *id == pb.id).then_some(())).is_some());
    assert!(wait_for(&brx, 10 * S, |e| matches!(e, NetEvent::Disconnected(id) if *id == pa.id).then_some(())).is_some());
    // B keeps retrying (manual peer, short backoff) but A refuses it.
    assert!(connected(&arx, 4 * S).is_none(), "banned peer reconnected");
    assert!(connected(&brx, S).is_none());
    assert!(a.peers().is_empty());
    b.shutdown();
    a.shutdown();
}

/// Minimal SOCKS5 server: no-auth, CONNECT, records the requested target,
/// and maps the fake hostname to the real node address.
fn socks5_server(real: std::net::SocketAddr) -> (std::net::SocketAddr, Arc<Mutex<Vec<(u8, String, u16)>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { break };
            let log = log2.clone();
            thread::spawn(move || {
                let mut g = [0u8; 2];
                c.read_exact(&mut g).unwrap();
                assert_eq!(g[0], 5);
                let mut m = vec![0u8; g[1] as usize];
                c.read_exact(&mut m).unwrap();
                assert!(m.contains(&0));
                c.write_all(&[5, 0]).unwrap();
                let mut h = [0u8; 4];
                c.read_exact(&mut h).unwrap();
                assert_eq!(&h[..3], &[5, 1, 0]);
                let host = match h[3] {
                    3 => {
                        let mut l = [0u8; 1];
                        c.read_exact(&mut l).unwrap();
                        let mut n = vec![0u8; l[0] as usize];
                        c.read_exact(&mut n).unwrap();
                        String::from_utf8(n).unwrap()
                    }
                    1 => {
                        let mut n = [0u8; 4];
                        c.read_exact(&mut n).unwrap();
                        std::net::Ipv4Addr::from(n).to_string()
                    }
                    _ => panic!("unexpected atyp"),
                };
                let mut p = [0u8; 2];
                c.read_exact(&mut p).unwrap();
                log.lock().unwrap().push((h[3], host, u16::from_be_bytes(p)));
                c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
                let s = TcpStream::connect(real).unwrap();
                let (mut c2, mut s2) = (c.try_clone().unwrap(), s.try_clone().unwrap());
                let mut s = s;
                thread::spawn(move || {
                    let _ = std::io::copy(&mut c2, &mut s2);
                    let _ = s2.shutdown(std::net::Shutdown::Both);
                });
                let _ = std::io::copy(&mut s, &mut c);
                let _ = c.shutdown(std::net::Shutdown::Both);
            });
        }
    });
    (addr, log)
}

#[test]
fn socks5_proxies_outbound_with_unresolved_hostname() {
    let (a, arx) = node(Opts::default());
    let (proxy, log) = socks5_server(a.local_addr().unwrap());
    let port = a.local_addr().unwrap().port();
    let (b, brx) = node(Opts {
        manual: vec![format!("nodea.lwtest.invalid:{port}")],
        proxy: Some(proxy),
        listen: false,
        ..Opts::default()
    });
    assert!(b.local_addr().is_none(), "no listener unless explicitly requested");
    let pa = connected(&brx, 15 * S).expect("connect via SOCKS5");
    let pb = connected(&arx, 15 * S).unwrap();
    b.send(pa.id, 0x40, b"via tor".to_vec());
    assert_eq!(msg(&arx, 0x40, 10 * S).unwrap(), (pb.id, b"via tor".to_vec()));
    let l = log.lock().unwrap().clone();
    assert!(!l.is_empty());
    assert!(l.iter().all(|(atyp, host, p)| *atyp == 3 && host == "nodea.lwtest.invalid" && *p == port), "{l:?}");
    b.shutdown();
    a.shutdown();
}

/// Minimal "node" loop: validates nothing, relays stems, records fluffs.
fn harness(
    n: Arc<Network>,
    rx: crossbeam_channel::Receiver<NetEvent>,
    relay_stems: bool,
) -> (Arc<Mutex<Vec<Vec<u8>>>>, Arc<Mutex<Vec<Vec<u8>>>>, Arc<Mutex<Vec<(u8, Vec<u8>)>>>) {
    let fluffed = Arc::new(Mutex::new(Vec::new()));
    let stems = Arc::new(Mutex::new(Vec::new()));
    let other = Arc::new(Mutex::new(Vec::new()));
    let (f2, s2, o2) = (fluffed.clone(), stems.clone(), other.clone());
    thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            match ev {
                NetEvent::StemTx { peer, payload } => {
                    s2.lock().unwrap().push(payload.clone());
                    if relay_stems {
                        n.relay_tx(h256("LW1/test/tx", &[&payload]), payload, 0x50, Some(peer));
                    }
                }
                NetEvent::Message { kind: 0x50, payload, .. } => {
                    n.mark_fluffed(h256("LW1/test/tx", &[&payload]));
                    f2.lock().unwrap().push(payload);
                }
                NetEvent::Message { kind, payload, .. } => o2.lock().unwrap().push((kind, payload)),
                _ => {}
            }
        }
    });
    (fluffed, stems, other)
}

fn wait_until(d: Duration, mut f: impl FnMut() -> bool) -> bool {
    let until = std::time::Instant::now() + d;
    while std::time::Instant::now() < until {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    f()
}

#[test]
fn three_node_broadcast_and_dandelion() {
    let (a, arx) = node(Opts::default());
    let (b, brx) = node(Opts { manual: vec![addr(&a)], ..Opts::default() });
    let (c, crx) = node(Opts { manual: vec![addr(&a), addr(&b)], ..Opts::default() });
    let (a, b, c) = (Arc::new(a), Arc::new(b), Arc::new(c));
    assert!(wait_until(20 * S, || a.peers().len() == 2 && b.peers().len() == 2 && c.peers().len() == 2));
    let (af, a_st, ao) = harness(a.clone(), arx, true);
    let (bf, b_st, bo) = harness(b.clone(), brx, true);
    let (cf, _cs, co) = harness(c.clone(), crx, true);
    // A node "has" a tx once it saw it in either phase (a node that fluffs
    // does not receive its own broadcast).
    let has = |f: &Arc<Mutex<Vec<Vec<u8>>>>, st: &Arc<Mutex<Vec<Vec<u8>>>>, tx: &Vec<u8>| {
        f.lock().unwrap().contains(tx) || st.lock().unwrap().contains(tx)
    };

    a.broadcast(0x60, b"block".to_vec(), None);
    assert!(wait_until(10 * S, || bo.lock().unwrap().len() == 1 && co.lock().unwrap().len() == 1));
    assert!(ao.lock().unwrap().is_empty());

    // C has two outbound peers, so its local tx is stemmed and later fluffed
    // (by a relayer's coin flip or by an embargo timer); every other node
    // must end up with it.
    for i in 0..3u8 {
        let tx = vec![0xA0, i, 1, 2, 3];
        c.relay_tx(h256("LW1/test/tx", &[&tx]), tx.clone(), 0x50, None);
        assert!(
            wait_until(15 * S, || has(&af, &a_st, &tx) && has(&bf, &b_st, &tx)),
            "tx {i} did not reach all nodes"
        );
    }
    // Dedupe: relaying the same tx again is a no-op.
    thread::sleep(3 * S); // let embargoes of the txs above settle
    let count = || af.lock().unwrap().len() + bf.lock().unwrap().len() + a_st.lock().unwrap().len() + b_st.lock().unwrap().len();
    let n_before = count();
    let tx = vec![0xA0, 0, 1, 2, 3];
    c.relay_tx(h256("LW1/test/tx", &[&tx]), tx, 0x50, None);
    thread::sleep(3 * S);
    assert_eq!(count(), n_before);
    let _ = cf;
    c.shutdown();
    b.shutdown();
    a.shutdown();
}

#[test]
fn embargo_fires_unless_marked_fluffed() {
    let (a, arx) = node(Opts::default());
    let mut t = Tuning::fast_for_tests();
    t.stem_probability = 1.0;
    let (b, brx) = node(Opts { manual: vec![addr(&a)], listen: false, tuning: t, ..Opts::default() });
    connected(&brx, 15 * S).unwrap();
    let (a, b) = (Arc::new(a), Arc::new(b));
    // A swallows stems (a black-holing stem peer).
    let (af, a_stems, _) = harness(a.clone(), arx, false);
    let (_bf, _, _) = harness(b.clone(), brx, false);

    let tx1 = b"tx-marked".to_vec();
    let id1 = h256("LW1/test/tx", &[&tx1]);
    b.relay_tx(id1, tx1.clone(), 0x50, None);
    assert!(wait_until(10 * S, || a_stems.lock().unwrap().contains(&tx1)), "local tx is stemmed");
    b.mark_fluffed(id1);

    let tx2 = b"tx-unmarked".to_vec();
    b.relay_tx(h256("LW1/test/tx", &[&tx2]), tx2.clone(), 0x50, None);
    assert!(wait_until(10 * S, || a_stems.lock().unwrap().contains(&tx2)));
    // tx2's embargo (1.5–2.5 s in tests) fires and B fluffs it itself.
    assert!(wait_until(10 * S, || af.lock().unwrap().contains(&tx2)), "embargo must fluff");
    thread::sleep(2 * S);
    assert!(!af.lock().unwrap().contains(&tx1), "mark_fluffed must cancel the embargo");
    b.shutdown();
    a.shutdown();
}

#[test]
fn anchors_and_peerdb_persist() {
    let (a, _arx) = node(Opts::default());
    let dir = tmpdir();
    let mk = |manual: Vec<String>| NetConfig {
        network_magic: MAGIC,
        network: 9,
        listen: None,
        data_dir: dir.clone(),
        seeds: vec![],
        socks5_proxy: None,
        max_outbound: 8,
        max_inbound: 8,
        identity: IdentityKey::generate(),
        ephemeral_outbound_identity: true,
        manual_peers: manual,
    };
    // Seed the address book file directly, as if learned by gossip earlier.
    std::fs::write(dir.join("peers.dat"), format!("{} 0 0\n", addr(&a))).unwrap();
    let (b, brx) = Network::start_tuned(mk(vec![]), Tuning::fast_for_tests()).unwrap();
    let p = connected(&brx, 15 * S).expect("address book entry dialed");
    assert!(!p.is_seed);
    thread::sleep(2 * S); // > anchor_min_age, > save_interval
    b.shutdown();
    let anchors = std::fs::read_to_string(dir.join("anchors.dat")).unwrap();
    assert!(anchors.contains(&addr(&a)), "anchor persisted: {anchors:?}");
    a.shutdown();
}
