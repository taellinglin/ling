#![cfg(feature = "lwp")]
#[path = "lwp_common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use ling_net::lwp::*;

const S: Duration = Duration::from_secs(1);

fn msgs(rx: &crossbeam_channel::Receiver<NetEvent>, kind: u8, n: usize, d: Duration) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let until = Instant::now() + d;
    while out.len() < n {
        let Some(left) = until.checked_duration_since(Instant::now()) else { break };
        match rx.recv_timeout(left) {
            Ok(NetEvent::Message { kind: k, payload, .. }) if k == kind => out.push(payload),
            Ok(NetEvent::Disconnected(_)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    out
}

#[test]
fn rekey_and_cover_traffic_keep_link_alive() {
    let mut t = Tuning::fast_for_tests();
    t.rekey_frames = 3; // ratchet every other frame
    t.cover_min = Duration::from_millis(50);
    t.cover_max = Duration::from_millis(150);
    t.ping_interval = Duration::from_millis(300);
    let (a, arx) = node(Opts { tuning: t.clone(), ..Opts::default() });
    let (b, brx) = node(Opts { manual: vec![addr(&a)], listen: false, tuning: t, ..Opts::default() });
    let pa = connected(&brx, 15 * S).unwrap();
    connected(&arx, 15 * S).unwrap();
    for i in 0..40u32 {
        b.send(pa.id, 0x20, i.to_le_bytes().to_vec());
    }
    let got = msgs(&arx, 0x20, 40, 15 * S);
    assert_eq!(got.len(), 40);
    for (i, g) in got.iter().enumerate() {
        assert_eq!(g, &(i as u32).to_le_bytes().to_vec(), "in order");
    }
    // Idle for a while: only cover/ping frames (each rekeying) flow.
    let before = b.peers()[0].bytes_in;
    thread::sleep(2 * S);
    let p = &b.peers()[0];
    assert!(p.bytes_in > before + 5 * 1096, "cover traffic flowing ({} -> {})", before, p.bytes_in);
    b.send(pa.id, 0x21, b"still alive".to_vec());
    assert_eq!(msgs(&arx, 0x21, 1, 10 * S), vec![b"still alive".to_vec()]);
    b.shutdown();
    a.shutdown();
}

#[test]
fn silent_peer_is_detected_dead() {
    let mut t = Tuning::fast_for_tests();
    t.dead_timeout = Duration::from_millis(1500);
    let mut quiet = Tuning::fast_for_tests();
    quiet.cover_min = Duration::from_secs(3600);
    quiet.cover_max = Duration::from_secs(3600);
    quiet.ping_interval = Duration::from_secs(3600);
    let (a, arx) = node(Opts { tuning: t, ..Opts::default() });
    let (b, brx) = node(Opts { manual: vec![addr(&a)], listen: false, tuning: quiet, ..Opts::default() });
    let pb = connected(&arx, 15 * S).unwrap();
    connected(&brx, 15 * S).unwrap();
    let start = Instant::now();
    assert!(wait_for(&arx, 10 * S, |e| matches!(e, NetEvent::Disconnected(id) if *id == pb.id).then_some(())).is_some());
    assert!(start.elapsed() >= Duration::from_millis(1000));
    b.shutdown();
    a.shutdown();
}

#[test]
fn garbage_and_slowloris_handshakes_are_dropped() {
    let mut t = Tuning::fast_for_tests();
    t.handshake_timeout = Duration::from_millis(1500);
    let (a, arx) = node(Opts { tuning: t, ..Opts::default() });
    let target = a.local_addr().unwrap();

    // Wrong magic / random bytes.
    let mut s = TcpStream::connect(target).unwrap();
    let junk: Vec<u8> = (0..HANDSHAKE_MSG1_LEN).map(|_| rand::random()).collect();
    s.write_all(&junk).unwrap();
    s.set_read_timeout(Some(5 * S)).unwrap();
    let mut buf = [0u8; 16];
    assert!(matches!(s.read(&mut buf), Ok(0) | Err(_)), "garbage must be dropped without a reply");

    // Correct prefix but invalid ML-KEM key.
    let mut s = TcpStream::connect(target).unwrap();
    let mut m = b"LWP1".to_vec();
    m.extend_from_slice(&MAGIC);
    m.push(9);
    m.extend(std::iter::repeat_n(0xff, HANDSHAKE_MSG1_LEN - 9));
    s.write_all(&m).unwrap();
    s.set_read_timeout(Some(5 * S)).unwrap();
    assert!(matches!(s.read(&mut buf), Ok(0) | Err(_)));

    // Slowloris: a byte at a time, never finishing.
    let mut s = TcpStream::connect(target).unwrap();
    let start = Instant::now();
    s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    let mut closed = false;
    while start.elapsed() < 6 * S {
        if s.write_all(b"L").is_err() {
            closed = true;
            break;
        }
        match s.read(&mut buf) {
            Ok(0) => {
                closed = true;
                break;
            }
            Ok(_) => panic!("no reply expected"),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => {
                closed = true;
                break;
            }
        }
    }
    assert!(closed, "slowloris handshake must hit the deadline");
    assert!(start.elapsed() < 5 * S);

    // The node still works afterwards.
    let (b, brx) = node(Opts { manual: vec![addr(&a)], listen: false, ..Opts::default() });
    assert!(connected(&brx, 15 * S).is_some());
    assert!(connected(&arx, 5 * S).is_some());
    b.shutdown();
    a.shutdown();
}

#[test]
fn address_gossip_discovers_peers() {
    // A is the hub. B (listening) connects to A and announces its port; C
    // asks A for addresses and dials B on its own.
    let (a, _arx) = node(Opts::default());
    let (b, brx) = node(Opts { manual: vec![addr(&a)], ..Opts::default() });
    connected(&brx, 15 * S).unwrap();
    thread::sleep(S); // let A record B's announcement
    let (c, crx) = node(Opts { manual: vec![addr(&a)], listen: false, ..Opts::default() });
    let b_addr = addr(&b);
    let found = wait_for(&crx, 15 * S, |e| match e {
        NetEvent::Connected(p) if p.addr == b_addr => Some(p.clone()),
        _ => None,
    });
    let p = found.expect("C learned B via gossip and connected");
    assert!(!p.inbound);
    assert!(c.peer_netgroup(p.id).unwrap().starts_with("local:"));
    c.shutdown();
    b.shutdown();
    a.shutdown();
}

#[test]
fn shutdown_closes_event_channel() {
    let (a, arx) = node(Opts::default());
    a.shutdown();
    let until = Instant::now() + 5 * S;
    loop {
        match arx.recv_timeout(Duration::from_millis(100)) {
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            _ => assert!(Instant::now() < until, "receiver should disconnect after shutdown"),
        }
    }
}
