//! Black-box tests against `ling_net::lingtp`'s public API only: connect,
//! send several requests over one keep-alive connection, and check the
//! trust-on-first-use known-hosts store rejects a changed key.

#![cfg(feature = "lingtp")]

use std::net::TcpListener;
use std::thread;

use ling_crypto::MlDsa87Keypair;
use ling_net::lingtp::{client, known_hosts, server, LingtpRequest, LingtpResponse};

fn spawn_test_server(
    handler: impl Fn(&LingtpRequest) -> LingtpResponse + Send + Sync + 'static,
) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    let identity = MlDsa87Keypair::generate();
    thread::spawn(move || {
        let _ = server::serve_on(listener, identity, handler);
    });
    port
}

#[test]
fn keep_alive_connection_handles_several_requests() {
    let port = spawn_test_server(|req| match req.path.as_str() {
        "/shop/sweet-tooth-supply" => {
            LingtpResponse::json(200, &serde_json::json!({ "shop": "Sweet Tooth Supply" }))
        }
        "/health" => LingtpResponse::text(200, "ok"),
        _ => LingtpResponse::error(404, "not found"),
    });

    let mut conn = client::connect("127.0.0.1", port).expect("client handshake should succeed");

    let resp = conn.request(&LingtpRequest::get("/shop/sweet-tooth-supply")).expect("request 1");
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = resp.json_body().unwrap();
    assert_eq!(body["shop"], "Sweet Tooth Supply");

    let resp2 = conn.request(&LingtpRequest::get("/health")).expect("request 2 on same connection");
    assert_eq!(resp2.status, 200);
    assert_eq!(resp2.body, "ok");

    let resp3 = conn.request(&LingtpRequest::get("/nope")).expect("request 3");
    assert_eq!(resp3.status, 404);
}

#[test]
fn post_with_json_body_round_trips() {
    let port = spawn_test_server(|req| {
        let payload: serde_json::Value = req.json().unwrap();
        LingtpResponse::json(201, &serde_json::json!({ "echoed": payload }))
    });

    let mut conn = client::connect("127.0.0.1", port).unwrap();
    let req = LingtpRequest::post("/api/orders", "")
        .with_json_body(&serde_json::json!({ "product": "cosmic sour belts", "qty": 3 }))
        .unwrap();
    let resp = conn.request(&req).unwrap();
    assert_eq!(resp.status, 201);
    let body: serde_json::Value = resp.json_body().unwrap();
    assert_eq!(body["echoed"]["product"], "cosmic sour belts");
    assert_eq!(body["echoed"]["qty"], 3);
}

#[test]
fn known_hosts_rejects_a_changed_key() {
    let host_port = format!("lingtp-known-hosts-test-{}.invalid:1", std::process::id());
    let pubkey_a = "aa".repeat(32);
    let pubkey_b = "bb".repeat(32);

    assert_eq!(known_hosts::check_known_host(&host_port, &pubkey_a), Ok(false));
    known_hosts::learn_known_host(&host_port, &pubkey_a);
    assert_eq!(known_hosts::check_known_host(&host_port, &pubkey_a), Ok(true));
    assert_eq!(known_hosts::check_known_host(&host_port, &pubkey_b), Err(()));
}
