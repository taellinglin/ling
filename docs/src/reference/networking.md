# Networking

Ling has four networking families besides HTTP. Pick by what you are building:

| You want | Use | Security |
|---|---|---|
| A game lobby or LAN tool | `net_*` | Plain text, no authentication. Trusted LANs only. |
| To run commands on a server, or accept SSH clients | `ssh_*` | SSH-2, host keys pinned, password login. |
| A private request/response service | `lingtp_*` | Post-quantum handshake, pinned host keys. |
| A peer-to-peer network of nodes with identities | `p2p_*` | Post-quantum, authenticated, padded frames. |

For HTTP and HTTPS, see [Web Server and HTTP](../reference/web.md). `net_*` is part of every
install; `ssh_*`, `lingtp_*` and `p2p_*` need the `privacy` profile (included in the default
`full` install).

All of these builtins block or poll; none of them takes a callback. You write a loop that
checks for work and calls `sleep(ms)` between checks. Failures to bind or connect show up in
a status number, not as an error, so always check the status.

## LAN lobby: `net_*`

`net_*` is a small, dependable transport for game state. A process is either a **host** that
accepts up to 64 clients, or a **client** connected to one host. Each message is one line of
UTF-8 text. Sockets run on background threads, so a game loop never blocks: `net_send`
queues a line and `net_recv` takes one.

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `net_host(port)` | — | — | — | `เน็ตโฮสต์` | Listen on a TCP port (default 7777) and become the host |
| `net_join(ip, port)` | — | — | — | `เน็ตจอย` | Connect to a host and become a client |
| `net_send(text)` | — | — | — | `เน็ตส่ง` | Send one line: host to all clients, client to host |
| `net_send_to(id, text)` | — | — | — | — | Host only: send one line to one client |
| `net_recv()` | — | — | — | `เน็ตรับ` | Oldest queued message, or `""` |
| `net_recv_from()` | — | — | — | — | Same, as `"id\|text"` |
| `net_clients()` | — | — | — | — | Host: client ids such as `"1,2"` |
| `net_client_count()` | — | — | — | — | Number of connected peers |
| `net_events()` | — | — | — | — | Drain `connect:<id>` / `disconnect:<id>` lines |
| `net_status()` | — | — | — | `เน็ตสถานะ` | `0` idle or failed, `1` waiting, `2` connected |
| `net_close()` | — | — | `연결종료` | — | Close the host or client |
| `net_announce(port, info)` | — | — | — | `เน็ตประกาศ` | Host: broadcast a lobby description on the LAN |
| `net_announce_stop()` | — | — | — | `เน็ตหยุดประกาศ` | Stop broadcasting |
| `net_discover(port)` | — | — | — | `เน็ตค้นหา` | Client: hosts heard in the last 5 seconds |
| `net_test(port)` | — | — | — | `เน็ตทดสอบ` | Report whether the port can be bound and the LAN address |
| `net_local_ip()` | — | — | — | — | This machine's LAN address, or `""` offline |
| `net_upnp_map(port, description)` | — | — | — | — | Ask the router to forward a TCP port (in the background) |
| `net_upnp_poll()` | — | — | — | — | Result of `net_upnp_map` |
| `net_upnp_unmap()` | — | — | — | — | Remove the forward |

### Hosting and joining

`net_host(port)` and `net_join(ip, port)` return nothing; watch `net_status()`. On a host,
`1` means waiting for a client and `2` means at least one is connected. On a client, `1` means
connecting and `2` means connected. If the port can't be bound or the connection fails, the
status goes back to `0`. One role per process: calling `net_host` or `net_join` again replaces
the old role.

```ling-gui
# host.ling
bind start = do {
    net_host(17777)
    bind n = 0
    while n < 100 {
        bind m = net_recv()
        if m != "" {
            print("host got: " + m)
            net_send("echo:" + m)
            bind n = 1000
        }
        sleep(50)
        bind n = n + 1
    }
    sleep(300)
    net_close()
}
```

```ling-gui
# client.ling
bind start = do {
    net_join("127.0.0.1", 17777)
    bind n = 0
    while net_status() != 2 && n < 100 {
        sleep(50)
        bind n = n + 1
    }
    net_send("hello")
    bind got = ""
    while got == "" && n < 200 {
        bind got = net_recv()
        sleep(50)
        bind n = n + 1
    }
    print("client got: " + got)
    net_close()
}
```

Run the host in one terminal and the client in another. The client prints
`client got: echo:hello`.

### Messages and events

- A message must not contain a newline: an embedded newline arrives as two messages.
- Each inbound queue holds 4096 lines; when it is full, the oldest are dropped.
- `net_recv()` and `net_recv_from()` read the same queue, so use one style per program.
  `net_recv_from()` returns `"2|hello"`; split on the **first** `|`, because the message itself
  may contain `|`.
- `net_events()` keeps the last 256 events, joined with newlines.

### Finding hosts on the LAN

`net_announce(port, info)` broadcasts `info` over UDP (default port 7778) about once a second.
Calling it again updates the text. `net_discover(port)` returns `"ip|info"` lines for hosts
heard in the last 5 seconds. Its first call starts a listener, so it returns `""` until an
announcement arrives.

```ling-gui
bind start = do {
    bind hosts = ""
    bind n = 0
    while hosts == "" && n < 40 {
        bind hosts = net_discover(7778)
        sleep(250)
        bind n = n + 1
    }
    print(hosts)
}
```

### Router port forwarding

`net_upnp_map(port, description)` asks the router (UPnP IGD) to forward a TCP port to this
machine. `net_upnp_poll()` returns `""` while it works, then `"ok|<public ip>"`,
`"cgnat|<ip>"` (the router is itself behind carrier NAT, so a forward can't reach you), or
`"fail|<reason>"`. `net_upnp_unmap()` removes the forward.

### Security

The host listens on all interfaces, the traffic is plain text, and anyone who can reach the
port can join. Use `net_*` on trusted LANs or inside a tunnel. Across the internet, use
`lingtp_*`, `ssh_*` or HTTPS. `net_test` and `net_local_ip` learn the LAN address by opening a
UDP socket towards a public address; nothing is sent.

## SSH: `ssh_*`

The SSH builtins are a real SSH-2 client and a small SSH server. The client works with any
OpenSSH server. The server lets a Ling program receive shell lines and `exec` requests from
OpenSSH clients. None of them has a localized alias.

| Builtin | Purpose |
|---|---|
| `ssh_connect(host, port, user, password)` | Connect and log in: `1` ok, `0` wrong credentials, `-1` connection error, `-2` host key changed |
| `ssh_exec(command)` | Run one command and return its standard output |
| `ssh_exit_code()` | Exit status of the last `ssh_exec` (`-1` if none) |
| `ssh_client_status()` | `0` disconnected, `2` connected |
| `ssh_close()` | Disconnect the client |
| `ssh_config(port, user, password, host_key_path)` | Stage the server settings |
| `ssh_serve()` | Start the server in the background; `true` if listening |
| `ssh_serve_recv()` | Oldest input from any client as `"id\|text"`, or `""` |
| `ssh_serve_send(id, text)` | Send one line to a client |
| `ssh_serve_exit(id, code)` | Finish an `exec` request with an exit status and close the channel |
| `ssh_serve_clients()` | Connected client ids, comma-separated |
| `ssh_serve_client_count()` | Number of connected clients |
| `ssh_serve_events()` | Drain `connect:`, `disconnect:` and `authfail:` lines |
| `ssh_serve_status()` | `0` idle, `1` listening, `2` client connected |
| `ssh_stop()` | Stop listening and drop all clients |

### Client

```ling-gui
bind start = do {
    bind r = ssh_connect("192.168.1.20", 22, "deploy", env_get("DEPLOY_PASSWORD", ""))
    if r == 1 {
        print(ssh_exec("uname -a"))
        print(ssh_exit_code())
        ssh_close()
    } else {
        print("ssh failed: " + r)
    }
}
```

The client checks every host key against a `known_hosts` file in OpenSSH format. The first
connection to a host trusts its key, like OpenSSH; a changed key is refused (`-2`).
Authentication is by password only. Don't put passwords in source code: read them with
`env_get`.

### Server

`ssh_config` takes the port (default 22), the one accepted user name, a password that must not
be empty, and the Ed25519 host key file (`""` means `.ling-ssh/host_ed25519`, created on first
start). The server offers password login only and compares passwords in constant time. It
doesn't run commands for you: it queues what the client typed, and your code decides what
to answer. It is a protocol endpoint, not a shell.

```ling-gui
bind start = do {
    ssh_config(2222, "admin", env_get("SSH_PASSWORD", ""), "")
    if ssh_serve() {
        while true {
            bind m = ssh_serve_recv()
            if m != "" {
                bind parts = split(m, "|")
                ssh_serve_send(to_number(parts[0]), "you said: " + parts[1])
                ssh_serve_exit(to_number(parts[0]), 0)
            }
            sleep(50)
        }
    }
}
```

> Known limitation: in a loopback test of the Ling client against the Ling server, `ssh_exec`
> returned `""` with exit code `-1` instead of the server's reply. Prefer OpenSSH as the peer
> and test your own pairing before you depend on it.

## Post-quantum transport: `lingtp://`

`lingtp://` is a small request/response protocol that plays the role of HTTPS with
post-quantum cryptography and no certificate authority. Each connection starts with a
handshake: the server proves its identity with an **ML-DSA-87** signature, and both sides agree
a key with the **X25519 + ML-KEM-768** hybrid KEM. Every frame is then sealed with
XChaCha20-Poly1305 and wrapped again with the DICE-42 cascade. Keys are fresh per connection,
which gives forward secrecy.

Trust works like SSH. The first time a client sees a host, it stores the host's public key in
`.lingtp/known_hosts` (relative to the current folder). If that host later shows a different
key, the connection is refused. If the network may be hostile, check the key out of band the
first time.

A request is one string (the path, including any query string) and a response is one string
(the body). A POST sends `path + "\n" + body`, so a handler tells them apart by looking for a
newline.

| Builtin | Purpose |
|---|---|
| `lingtp_get(host, port, path)` | Fetch a path (port defaults to 7780); returns the body, `""` on failure |
| `lingtp_post(host, port, path, body)` | Send a body; returns the response body |
| `lingtp_get_status()` | Outcome of the last request: `1` ok, `0` connect error, `-1` bad server signature, `-2` host key changed |
| `lingtp_config(port, host_key_path)` | Stage the server settings |
| `lingtp_route(prefix, handler)` | Answer paths that start with `prefix` (longest prefix wins) |
| `lingtp_serve()` | Serve until `lingtp_stop`; blocks the calling thread |
| `lingtp_stop()` | Stop the server |
| `lingtp_status()` | `0` stopped, `1` listening, `2` client connected |
| `lingtp_client_count()` | Number of connected clients |
| `lingtp_events()` | Drain `connect:<id>` / `disconnect:<id>` lines |

None of them has a localized alias.

### Serving

Register routes just before `lingtp_serve()`: it clears the route table when it starts. It
returns `false` if nothing was configured or the key or socket couldn't be created. A handler
takes the path and returns the body.

```ling-gui
fn greet(path) {
    return "pq-hello from " + path
}

bind start = do {
    lingtp_config(17780, "hostkey.hex")
    lingtp_route("/", greet)
    print("serving")
    lingtp_serve()
}
```

### Fetching

```ling-gui
bind start = do {
    bind body = lingtp_get("127.0.0.1", 17780, "/hi?x=1")
    if lingtp_get_status() == 1 {
        print(body)
    } else {
        print("lingtp failed: " + lingtp_get_status())
    }
}
```

Against the server above this prints `pq-hello from /hi?x=1`.

### Security notes

- The host key is stored as a hex seed in a plain file (`.lingtp/host_mldsa87` by default, or the
  path you give `lingtp_config`). Protect it like an SSH private key and back it up: if you lose
  it, the server's identity changes and every client refuses to connect.
- A handler error is sent back to the client as `handler error: ...`, which can reveal internals.
  Validate input and avoid errors in handlers.
- The server listens on all interfaces and answers one request at a time.
- `lingtp_post` treats the first newline as the start of the body, so percent-encode newlines
  in the path.
- The `ling-net` Rust crate has its own `lingtp` module with a different wire format. A Ling
  `lingtp_get` can't talk to a `ling-net` server. Use one implementation on both ends.

## Peer-to-peer: `p2p_*`

`p2p_*` runs LWP-1, the transport of the Ling Wallet project. Peers authenticate each other with
ML-DSA-87 identities during a hybrid X25519 + ML-KEM-1024 handshake, and frames are padded to
fixed sizes and sealed. A node keeps a peer database, can use a signed seed list and a SOCKS5
proxy (such as Tor on `127.0.0.1:9050`), and relays transactions with Dandelion++. Payloads are
hex strings tagged with a `kind` number that your program defines.

| Builtin | Purpose |
|---|---|
| `p2p_start(config, events)` | Start a node; `Ok(handle)` or `Err(message)`. Pass `""` for `events` |
| `p2p_poll(net, max, timeout_ms)` | Drain up to `max` events (default 64), waiting up to `timeout_ms` |
| `p2p_send(net, peer, kind, payload_hex)` | Send to one peer; `true` if queued |
| `p2p_broadcast(net, kind, payload_hex, except_peer)` | Send to every peer except one (`-1` for none) |
| `p2p_relay(net, id_hex, payload_hex, kind, from_peer)` | Dandelion++ relay of a validated transaction |
| `p2p_mark_fluffed(net, id_hex)` | Record that a transaction was already broadcast |
| `p2p_peers(net)` | `[id, addr, inbound, fingerprint, is_seed, connected_secs]` per peer |
| `p2p_misbehaving(net, peer, score, reason)` | Add to a peer's ban score |
| `p2p_local_addr(net)` | Listening address `"ip:port"`, or `""` |
| `p2p_fingerprint(identity_seed_hex)` | Fingerprint peers will see for a seed, like `9ebc-56af-97f2-61af` |
| `p2p_netgroup(net, peer)` | Network group of a peer, such as `v4:1.2` |
| `p2p_seeds_sign(network, issued_at, entries, authority_seed_hex)` | Sign a seed list as the seed authority |
| `p2p_seeds_verify(list_hex, authority_pk_hex)` | `Ok([network, issued_at, entries])` or `Err(...)` |
| `p2p_shutdown(net)` | Stop the node; `true` the first time |

None of them has a localized alias. `str_to_hex(text)` and `hex_to_str(hex)` convert payloads
between text and hex.

### Configuration

`config` is a list of `[key, value]` pairs. Unknown keys, duplicates and wrong types are
rejected with `Err(message)`.

| Key | Meaning |
|---|---|
| `magic_hex` | **Required.** 8 hex characters; peers with different magic never connect. |
| `network` | **Required.** Network id, 0–255. |
| `data_dir` | **Required.** Folder for the peer database and identity; created if missing. |
| `listen` | `"ip:port"` to accept peers, or `""` for outbound only. Port `0` picks a free port. |
| `peers` | List of `"host:port"` peers to always try. |
| `identity_seed_hex` | 64 hex characters for a long-term identity; omit for a random one. |
| `ephemeral_outbound` | Use a fresh identity per outbound connection (default `true`). |
| `max_outbound`, `max_inbound` | Connection limits (defaults 8 and 32). |
| `proxy` | SOCKS5 proxy `"ip:port"` for all outbound traffic. |
| `seeds_hex`, `authority_pk_hex` | A signed seed list and the key that must verify it. |
| `fast_timers` | Short timers, for tests. |

### Events

`p2p_poll` returns a list of events, each itself a list:

| First element | Layout |
|---|---|
| `"connected"` | `["connected", peer_id, addr, inbound, fingerprint, is_seed]` |
| `"disconnected"` | `["disconnected", peer_id]` |
| `"message"` | `["message", peer_id, kind, payload_hex]` |
| `"stem"` | `["stem", peer_id, payload_hex]`: a Dandelion++ transaction to validate |

Keep polling: events wait in a bounded queue, and a full queue slows down every peer. Message
kinds are 16–255 (lower values are reserved), and payloads are at most about 4 MiB.

### Example

Two nodes on one machine: A listens, B connects to A and sends `hello`.

```ling-gui
fn u(r) { return match r { ok(v) => v, bad(e) => "ERR:" + e } }

bind start = do {
    bind a = u(p2p_start([["magic_hex", "4c494e47"], ["network", 1], ["listen", "127.0.0.1:0"], ["data_dir", "node_a"]], ""))
    bind b = u(p2p_start([["magic_hex", "4c494e47"], ["network", 1], ["listen", ""], ["data_dir", "node_b"], ["peers", [p2p_local_addr(a)]]], ""))
    bind peers = []
    bind n = 0
    while len(peers) == 0 && n < 100 {
        bind peers = p2p_peers(b)
        sleep(100)
        bind n = n + 1
    }
    p2p_send(b, peers[0][0], 16, str_to_hex("hello"))
    bind seen = ""
    bind n = 0
    while seen == "" && n < 20 {
        bind evs = p2p_poll(a, 10, 500)
        bind i = 0
        while i < len(evs) {
            if evs[i][0] == "message" { bind seen = hex_to_str(evs[i][3]) }
            bind i = i + 1
        }
        bind n = n + 1
    }
    print("A received: " + seen)
    p2p_shutdown(a)
    p2p_shutdown(b)
}
```

### Security notes

- In a restricted isolate every `p2p_*` builtin needs the `"p2p"` capability. The main thread
  has every capability.
- A network handle is an unguessable 128-bit hex string, valid only in the isolate that
  started it. An unknown handle raises an error; after `p2p_shutdown`, calls return `false` or `[]`.
- `data_dir` holds the node's identity. Protect it like a private key.

## See also

- [Web Server and HTTP](../reference/web.md) for `http_*`.
- [Cryptography](../reference/crypto.md) for the primitives behind `lingtp://` and `p2p_*`.
