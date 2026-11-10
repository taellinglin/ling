# 네트워킹

Ling에는 HTTP 외에 네 가지 네트워킹 계열이 있습니다. 만들려는 것에 따라 고르십시오.

| 하려는 일 | 사용 | 보안 |
|---|---|---|
| 게임 로비나 LAN 도구 | `net_*` | 평문, 인증 없음. 신뢰할 수 있는 LAN 전용. |
| 서버에서 명령 실행, 또는 SSH 클라이언트 받기 | `ssh_*` | SSH-2, 호스트 키 고정, 비밀번호 로그인. |
| 비공개 요청/응답 서비스 | `lingtp_*` | 양자 내성 핸드셰이크, 호스트 키 고정. |
| 신원을 가진 노드들의 P2P 네트워크 | `p2p_*` | 양자 내성, 인증, 패딩된 프레임. |

HTTP와 HTTPS는 [웹 서버와 HTTP](../reference/web.md)를 보십시오. `net_*`는 모든 설치에
포함됩니다. `ssh_*`, `lingtp_*`, `p2p_*`는 `privacy` 프로필이 필요합니다(기본 `full` 설치에 포함).

이 내장 함수들은 모두 블로킹하거나 폴링하며, 콜백을 받는 것은 없습니다. 할 일이 있는지 확인하고
확인 사이에 `sleep(ms)`를 호출하는 루프를 작성하십시오. 바인드나 연결 실패는 오류가 아니라 상태
숫자로 나타나므로 항상 상태를 확인하십시오.

## LAN 로비: `net_*`

`net_*`는 게임 상태를 주고받기 위한 작고 믿을 만한 전송 방식입니다. 프로세스는 최대 64개의
클라이언트를 받는 **호스트**이거나, 호스트 하나에 연결된 **클라이언트**입니다. 메시지 하나는
UTF-8 텍스트 한 줄입니다. 소켓은 백그라운드 스레드에서 동작하므로 게임 루프가 막히지 않습니다.
`net_send`는 한 줄을 대기열에 넣고 `net_recv`는 한 줄을 꺼냅니다.

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `net_host(port)` | — | — | — | `เน็ตโฮสต์` | TCP 포트(기본값 7777)에서 대기하며 호스트가 됨 |
| `net_join(ip, port)` | — | — | — | `เน็ตจอย` | 호스트에 연결해 클라이언트가 됨 |
| `net_send(text)` | — | — | — | `เน็ตส่ง` | 한 줄 전송: 호스트는 모든 클라이언트에, 클라이언트는 호스트에 |
| `net_send_to(id, text)` | — | — | — | — | 호스트 전용: 한 클라이언트에 한 줄 전송 |
| `net_recv()` | — | — | — | `เน็ตรับ` | 가장 오래된 메시지, 없으면 `""` |
| `net_recv_from()` | — | — | — | — | 같은 동작, `"id\|text"` 형태 |
| `net_clients()` | — | — | — | — | 호스트: `"1,2"` 같은 클라이언트 ID |
| `net_client_count()` | — | — | — | — | 연결된 피어 수 |
| `net_events()` | — | — | — | — | `connect:<id>` / `disconnect:<id>` 줄을 비우며 읽기 |
| `net_status()` | — | — | — | `เน็ตสถานะ` | `0` 유휴 또는 실패, `1` 대기 중, `2` 연결됨 |
| `net_close()` | — | — | `연결종료` | — | 호스트 또는 클라이언트 종료 |
| `net_announce(port, info)` | — | — | — | `เน็ตประกาศ` | 호스트: LAN에 로비 설명 브로드캐스트 |
| `net_announce_stop()` | — | — | — | `เน็ตหยุดประกาศ` | 브로드캐스트 중지 |
| `net_discover(port)` | — | — | — | `เน็ตค้นหา` | 클라이언트: 최근 5초 동안 들린 호스트 |
| `net_test(port)` | — | — | — | `เน็ตทดสอบ` | 포트를 바인드할 수 있는지와 LAN 주소 보고 |
| `net_local_ip()` | — | — | — | — | 이 컴퓨터의 LAN 주소, 오프라인이면 `""` |
| `net_upnp_map(port, description)` | — | — | — | — | 라우터에 TCP 포트 포워딩 요청(백그라운드) |
| `net_upnp_poll()` | — | — | — | — | `net_upnp_map`의 결과 |
| `net_upnp_unmap()` | — | — | — | — | 포워딩 제거 |

### 호스팅과 참가

`net_host(port)`와 `net_join(ip, port)`는 아무것도 반환하지 않으니 `net_status()`를 지켜보십시오.
호스트에서 `1`은 클라이언트를 기다리는 중, `2`는 하나 이상 연결됨을 뜻합니다. 클라이언트에서
`1`은 연결 중, `2`는 연결됨을 뜻합니다. 포트를 바인드할 수 없거나 연결에 실패하면 상태는 `0`으로
돌아갑니다. 프로세스당 역할은 하나뿐이어서 `net_host`나 `net_join`을 다시 호출하면 이전 역할을
대체합니다.

```ling-gui
# host.ling
바인드 시작 = 실행 {
    net_host(17777)
    바인드 횟수 = 0
    동안 횟수 < 100 {
        바인드 메시지 = net_recv()
        만약 메시지 != "" {
            출력("호스트가 받음: " + 메시지)
            net_send("메아리:" + 메시지)
            바인드 횟수 = 1000
        }
        잠(50)
        바인드 횟수 = 횟수 + 1
    }
    잠(300)
    net_close()
}
```

```ling-gui
# client.ling
바인드 시작 = 실행 {
    net_join("127.0.0.1", 17777)
    바인드 횟수 = 0
    동안 net_status() != 2 && 횟수 < 100 {
        잠(50)
        바인드 횟수 = 횟수 + 1
    }
    net_send("안녕")
    바인드 받은것 = ""
    동안 받은것 == "" && 횟수 < 200 {
        바인드 받은것 = net_recv()
        잠(50)
        바인드 횟수 = 횟수 + 1
    }
    출력("클라이언트가 받음: " + 받은것)
    net_close()
}
```

한 터미널에서 호스트를, 다른 터미널에서 클라이언트를 실행하십시오. 클라이언트는
`클라이언트가 받음: 메아리:안녕`을 출력합니다.

### 메시지와 이벤트

- 메시지에 줄바꿈이 들어가면 안 됩니다. 중간의 줄바꿈은 메시지 두 개로 도착합니다.
- 수신 대기열마다 4096줄을 담으며, 가득 차면 가장 오래된 줄부터 버립니다.
- `net_recv()`와 `net_recv_from()`은 같은 대기열을 읽으므로 프로그램마다 한 방식만 쓰십시오.
  `net_recv_from()`은 `"2|hello"`를 반환합니다. 메시지 자체에 `|`가 있을 수 있으니 **첫 번째**
  `|`에서 나누십시오.
- `net_events()`는 최근 이벤트 256개를 줄바꿈으로 이어서 보관합니다.

### LAN에서 호스트 찾기

`net_announce(port, info)`는 `info`를 UDP(기본 포트 7778)로 약 1초마다 브로드캐스트합니다. 다시
호출하면 텍스트가 바뀝니다. `net_discover(port)`는 최근 5초 동안 들린 호스트를 `"ip|info"` 줄로
반환합니다. 처음 호출할 때 수신기를 시작하므로 알림이 도착할 때까지는 `""`를 반환합니다.

```ling-gui
바인드 시작 = 실행 {
    바인드 호스트들 = ""
    바인드 횟수 = 0
    동안 호스트들 == "" && 횟수 < 40 {
        바인드 호스트들 = net_discover(7778)
        잠(250)
        바인드 횟수 = 횟수 + 1
    }
    출력(호스트들)
}
```

### 라우터 포트 포워딩

`net_upnp_map(port, description)`은 라우터(UPnP IGD)에 TCP 포트를 이 컴퓨터로 포워딩해 달라고
요청합니다. `net_upnp_poll()`은 작업 중에는 `""`를, 이후에는 `"ok|<public ip>"`,
`"cgnat|<ip>"`(라우터 자체가 통신사 NAT 뒤에 있어 포워딩이 닿지 않음), 또는 `"fail|<reason>"`을
반환합니다. `net_upnp_unmap()`은 포워딩을 제거합니다.

### 보안

호스트는 모든 인터페이스에서 대기하고, 트래픽은 평문이며, 포트에 닿을 수 있는 누구나 참가할 수
있습니다. `net_*`는 신뢰할 수 있는 LAN이나 터널 안에서만 쓰십시오. 인터넷을 건너야 한다면
`lingtp_*`, `ssh_*` 또는 HTTPS를 쓰십시오. `net_test`와 `net_local_ip`는 공인 주소 쪽으로 UDP
소켓을 열어 LAN 주소를 알아냅니다. 실제로 보내는 데이터는 없습니다.

## SSH: `ssh_*`

SSH 내장 함수는 실제 SSH-2 클라이언트와 작은 SSH 서버입니다. 클라이언트는 어떤 OpenSSH 서버와도
동작합니다. 서버는 Ling 프로그램이 OpenSSH 클라이언트에서 셸 입력 줄과 `exec` 요청을 받을 수 있게
합니다. 현지화된 별칭은 없습니다.

| 내장 함수 | 용도 |
|---|---|
| `ssh_connect(host, port, user, password)` | 연결과 로그인: `1` 성공, `0` 자격 증명 틀림, `-1` 연결 오류, `-2` 호스트 키 변경 |
| `ssh_exec(command)` | 명령 하나를 실행하고 표준 출력을 반환 |
| `ssh_exit_code()` | 마지막 `ssh_exec`의 종료 상태(없으면 `-1`) |
| `ssh_client_status()` | `0` 연결 끊김, `2` 연결됨 |
| `ssh_close()` | 클라이언트 연결 끊기 |
| `ssh_config(port, user, password, host_key_path)` | 서버 설정 준비 |
| `ssh_serve()` | 백그라운드에서 서버 시작. 대기 중이면 `true` |
| `ssh_serve_recv()` | 아무 클라이언트에서 온 가장 오래된 입력 `"id\|text"`, 없으면 `""` |
| `ssh_serve_send(id, text)` | 클라이언트에 한 줄 전송 |
| `ssh_serve_exit(id, code)` | 종료 상태와 함께 `exec` 요청을 마치고 채널 닫기 |
| `ssh_serve_clients()` | 연결된 클라이언트 ID(쉼표 구분) |
| `ssh_serve_client_count()` | 연결된 클라이언트 수 |
| `ssh_serve_events()` | `connect:`, `disconnect:`, `authfail:` 줄을 비우며 읽기 |
| `ssh_serve_status()` | `0` 유휴, `1` 대기 중, `2` 클라이언트 연결됨 |
| `ssh_stop()` | 대기를 멈추고 모든 클라이언트 끊기 |

### 클라이언트

```ling-gui
바인드 시작 = 실행 {
    바인드 결과 = ssh_connect("192.168.1.20", 22, "deploy", env_get("DEPLOY_PASSWORD", ""))
    만약 결과 == 1 {
        출력(ssh_exec("uname -a"))
        출력(ssh_exit_code())
        ssh_close()
    } 아니면 {
        출력("ssh 실패: " + 결과)
    }
}
```

클라이언트는 모든 호스트 키를 OpenSSH 형식의 `known_hosts` 파일과 대조합니다. 호스트에 처음
연결할 때는 OpenSSH처럼 그 키를 신뢰하고, 키가 바뀌면 거부합니다(`-2`). 인증은 비밀번호로만
합니다. 비밀번호를 소스 코드에 넣지 말고 `env_get`으로 읽으십시오.

### 서버

`ssh_config`는 포트(기본값 22), 허용할 사용자 이름 하나, 비어 있으면 안 되는 비밀번호, Ed25519
호스트 키 파일(`""`이면 `.ling-ssh/host_ed25519`, 처음 시작할 때 생성)을 받습니다. 서버는 비밀번호
로그인만 제공하고 비밀번호를 상수 시간으로 비교합니다. 명령을 대신 실행하지는 않습니다.
클라이언트가 입력한 내용을 대기열에 넣고, 무엇을 답할지는 여러분의 코드가 정합니다. 셸이 아니라
프로토콜 엔드포인트입니다.

```ling-gui
바인드 시작 = 실행 {
    ssh_config(2222, "admin", env_get("SSH_PASSWORD", ""), "")
    만약 ssh_serve() {
        동안 참 {
            바인드 입력 = ssh_serve_recv()
            만약 입력 != "" {
                바인드 조각 = split(입력, "|")
                ssh_serve_send(to_number(조각[0]), "입력한 내용: " + 조각[1])
                ssh_serve_exit(to_number(조각[0]), 0)
            }
            잠(50)
        }
    }
}
```

> 알려진 제한: Ling 클라이언트를 Ling 서버에 루프백으로 연결해 시험했을 때, `ssh_exec`가 서버의
> 응답 대신 종료 코드 `-1`과 함께 `""`를 반환했습니다. 상대로는 OpenSSH를 우선 쓰고, 의존하기 전에
> 여러분의 조합을 직접 시험하십시오.

## 양자 내성 전송: `lingtp://`

`lingtp://`는 양자 내성 암호를 쓰고 인증 기관이 없는, HTTPS 역할을 하는 작은 요청/응답
프로토콜입니다. 연결마다 핸드셰이크로 시작합니다. 서버는 **ML-DSA-87** 서명으로 신원을 증명하고,
양쪽은 **X25519 + ML-KEM-768** 하이브리드 KEM으로 키를 합의합니다. 이후 모든 프레임은
XChaCha20-Poly1305로 봉인되고 DICE-42 캐스케이드로 한 번 더 감싸집니다. 키는 연결마다 새로 만들어
전방 비밀성을 제공합니다.

신뢰 방식은 SSH와 같습니다. 클라이언트는 호스트를 처음 볼 때 그 공개 키를 `.lingtp/known_hosts`
(현재 폴더 기준)에 저장합니다. 나중에 그 호스트가 다른 키를 내밀면 연결을 거부합니다. 네트워크가
적대적일 수 있다면 처음에 다른 경로로 키를 확인하십시오.

요청은 문자열 하나(쿼리 문자열을 포함한 경로)이고 응답도 문자열 하나(본문)입니다. POST는
`path + "\n" + body`를 보내므로, 핸들러는 줄바꿈이 있는지로 둘을 구별합니다.

| 내장 함수 | 용도 |
|---|---|
| `lingtp_get(host, port, path)` | 경로 가져오기(포트 기본값 7780). 본문을 반환하고 실패하면 `""` |
| `lingtp_post(host, port, path, body)` | 본문 전송. 응답 본문을 반환 |
| `lingtp_get_status()` | 마지막 요청 결과: `1` 성공, `0` 연결 오류, `-1` 서버 서명 불량, `-2` 호스트 키 변경 |
| `lingtp_config(port, host_key_path)` | 서버 설정 준비 |
| `lingtp_route(prefix, handler)` | `prefix`로 시작하는 경로에 응답(가장 긴 접두사 우선) |
| `lingtp_serve()` | `lingtp_stop`까지 서비스. 호출한 스레드를 블로킹 |
| `lingtp_stop()` | 서버 중지 |
| `lingtp_status()` | `0` 중지, `1` 대기 중, `2` 클라이언트 연결됨 |
| `lingtp_client_count()` | 연결된 클라이언트 수 |
| `lingtp_events()` | `connect:<id>` / `disconnect:<id>` 줄을 비우며 읽기 |

현지화된 별칭은 없습니다.

### 서비스하기

라우트는 `lingtp_serve()` 직전에 등록하십시오. 시작할 때 라우트 테이블을 비우기 때문입니다.
아무것도 설정하지 않았거나 키나 소켓을 만들 수 없으면 `false`를 반환합니다. 핸들러는 경로를 받아
본문을 반환합니다.

```ling-gui
함수 인사(경로) {
    반환 "pq-hello from " + 경로
}

바인드 시작 = 실행 {
    lingtp_config(17780, "hostkey.hex")
    lingtp_route("/", 인사)
    출력("서비스 중")
    lingtp_serve()
}
```

### 가져오기

```ling-gui
바인드 시작 = 실행 {
    바인드 본문 = lingtp_get("127.0.0.1", 17780, "/hi?x=1")
    만약 lingtp_get_status() == 1 {
        출력(본문)
    } 아니면 {
        출력("lingtp 실패: " + lingtp_get_status())
    }
}
```

위의 서버를 상대로 실행하면 `pq-hello from /hi?x=1`을 출력합니다.

### 보안 참고

- 호스트 키는 평문 파일에 16진수 시드로 저장됩니다(기본값 `.lingtp/host_mldsa87`, 또는
  `lingtp_config`에 준 경로). SSH 비밀 키처럼 보호하고 백업하십시오. 잃어버리면 서버의 신원이 바뀌어
  모든 클라이언트가 연결을 거부합니다.
- 핸들러 오류는 `handler error: ...`로 클라이언트에 돌려보내지므로 내부 정보가 드러날 수 있습니다.
  입력을 검증하고 핸들러에서 오류가 나지 않게 하십시오.
- 서버는 모든 인터페이스에서 대기하며 요청을 한 번에 하나씩 처리합니다.
- `lingtp_post`는 첫 줄바꿈을 본문의 시작으로 보므로 경로 안의 줄바꿈은 퍼센트 인코딩하십시오.
- `ling-net` Rust 크레이트에는 와이어 형식이 다른 자체 `lingtp` 모듈이 있습니다. Ling의
  `lingtp_get`은 `ling-net` 서버와 통신할 수 없습니다. 양쪽에 같은 구현을 쓰십시오.

## P2P: `p2p_*`

`p2p_*`는 Ling Wallet 프로젝트의 전송 방식인 LWP-1을 실행합니다. 피어는 하이브리드 X25519 +
ML-KEM-1024 핸드셰이크 중에 ML-DSA-87 신원으로 서로를 인증하고, 프레임은 고정 크기로 패딩되어
봉인됩니다. 노드는 피어 데이터베이스를 유지하고, 서명된 시드 목록과 SOCKS5 프록시(예:
`127.0.0.1:9050`의 Tor)를 쓸 수 있으며, Dandelion++로 트랜잭션을 중계합니다. 페이로드는 여러분의
프로그램이 정의하는 `kind` 숫자가 붙은 16진수 문자열입니다.

| 내장 함수 | 용도 |
|---|---|
| `p2p_start(config, events)` | 노드 시작. `Ok(handle)` 또는 `Err(message)`. `events`에는 `""`를 전달 |
| `p2p_poll(net, max, timeout_ms)` | 이벤트를 최대 `max`개(기본값 64)까지 꺼냄. 최대 `timeout_ms`만큼 대기 |
| `p2p_send(net, peer, kind, payload_hex)` | 피어 하나에 전송. 대기열에 들어가면 `true` |
| `p2p_broadcast(net, kind, payload_hex, except_peer)` | 하나를 제외한 모든 피어에 전송(없으면 `-1`) |
| `p2p_relay(net, id_hex, payload_hex, kind, from_peer)` | 검증된 트랜잭션의 Dandelion++ 중계 |
| `p2p_mark_fluffed(net, id_hex)` | 트랜잭션이 이미 브로드캐스트되었다고 기록 |
| `p2p_peers(net)` | 피어마다 `[id, addr, inbound, fingerprint, is_seed, connected_secs]` |
| `p2p_misbehaving(net, peer, score, reason)` | 피어의 차단 점수에 더하기 |
| `p2p_local_addr(net)` | 대기 주소 `"ip:port"`, 없으면 `""` |
| `p2p_fingerprint(identity_seed_hex)` | 시드에 대해 피어들이 보게 될 지문(예: `9ebc-56af-97f2-61af`) |
| `p2p_netgroup(net, peer)` | 피어의 네트워크 그룹(예: `v4:1.2`) |
| `p2p_seeds_sign(network, issued_at, entries, authority_seed_hex)` | 시드 기관으로서 시드 목록에 서명 |
| `p2p_seeds_verify(list_hex, authority_pk_hex)` | `Ok([network, issued_at, entries])` 또는 `Err(...)` |
| `p2p_shutdown(net)` | 노드 중지. 처음에만 `true` |

현지화된 별칭은 없습니다. `str_to_hex(text)`와 `hex_to_str(hex)`는 페이로드를 텍스트와 16진수
사이에서 변환합니다.

### 설정

`config`는 `[key, value]` 쌍의 리스트입니다. 알 수 없는 키, 중복, 잘못된 타입은
`Err(message)`로 거부됩니다.

| 키 | 의미 |
|---|---|
| `magic_hex` | **필수.** 16진수 8자. 매직이 다른 피어끼리는 연결되지 않습니다. |
| `network` | **필수.** 네트워크 ID, 0~255. |
| `data_dir` | **필수.** 피어 데이터베이스와 신원을 둘 폴더. 없으면 만듭니다. |
| `listen` | 피어를 받을 `"ip:port"`, 나가는 연결만 쓰려면 `""`. 포트 `0`은 빈 포트를 고릅니다. |
| `peers` | 항상 시도할 `"host:port"` 피어 리스트. |
| `identity_seed_hex` | 장기 신원용 16진수 64자. 생략하면 무작위 신원. |
| `ephemeral_outbound` | 나가는 연결마다 새 신원 사용(기본값 `true`). |
| `max_outbound`, `max_inbound` | 연결 수 제한(기본값 8과 32). |
| `proxy` | 모든 나가는 트래픽에 쓸 SOCKS5 프록시 `"ip:port"`. |
| `seeds_hex`, `authority_pk_hex` | 서명된 시드 목록과 그것을 검증할 키. |
| `fast_timers` | 테스트용 짧은 타이머. |

### 이벤트

`p2p_poll`은 이벤트 리스트를 반환하며, 각 이벤트도 리스트입니다.

| 첫 요소 | 구성 |
|---|---|
| `"connected"` | `["connected", peer_id, addr, inbound, fingerprint, is_seed]` |
| `"disconnected"` | `["disconnected", peer_id]` |
| `"message"` | `["message", peer_id, kind, payload_hex]` |
| `"stem"` | `["stem", peer_id, payload_hex]`: 검증할 Dandelion++ 트랜잭션 |

계속 폴링하십시오. 이벤트는 크기가 제한된 대기열에서 기다리며, 대기열이 가득 차면 모든 피어가
느려집니다. 메시지 kind는 16~255이고(그보다 작은 값은 예약됨), 페이로드는 최대 약 4 MiB입니다.

### 예제

한 컴퓨터의 두 노드입니다. A는 대기하고, B는 A에 연결해 `안녕`을 보냅니다.

```ling-gui
함수 풀기(r) { 반환 매치 r { 좋아(v) => v, 나쁨(e) => "오류:" + e } }

바인드 시작 = 실행 {
    바인드 가 = 풀기(p2p_start([["magic_hex", "4c494e47"], ["network", 1], ["listen", "127.0.0.1:0"], ["data_dir", "node_a"]], ""))
    바인드 나 = 풀기(p2p_start([["magic_hex", "4c494e47"], ["network", 1], ["listen", ""], ["data_dir", "node_b"], ["peers", [p2p_local_addr(가)]]], ""))
    바인드 피어들 = []
    바인드 횟수 = 0
    동안 길이(피어들) == 0 && 횟수 < 100 {
        바인드 피어들 = p2p_peers(나)
        잠(100)
        바인드 횟수 = 횟수 + 1
    }
    p2p_send(나, 피어들[0][0], 16, str_to_hex("안녕"))
    바인드 받은것 = ""
    바인드 횟수 = 0
    동안 받은것 == "" && 횟수 < 20 {
        바인드 이벤트들 = p2p_poll(가, 10, 500)
        바인드 i = 0
        동안 i < 길이(이벤트들) {
            만약 이벤트들[i][0] == "message" { 바인드 받은것 = hex_to_str(이벤트들[i][3]) }
            바인드 i = i + 1
        }
        바인드 횟수 = 횟수 + 1
    }
    출력("A가 받음: " + 받은것)
    p2p_shutdown(가)
    p2p_shutdown(나)
}
```

### 보안 참고

- 제한된 아이솔레이트에서는 모든 `p2p_*` 내장 함수에 `"p2p"` 권한이 필요합니다. 메인 스레드는
  모든 권한을 가집니다.
- 네트워크 핸들은 추측할 수 없는 128비트 16진수 문자열이며, 그것을 시작한 아이솔레이트에서만
  유효합니다. 알 수 없는 핸들은 오류를 일으킵니다. `p2p_shutdown` 이후의 호출은 `false`나 `[]`를
  반환합니다.
- `data_dir`에는 노드의 신원이 들어 있습니다. 비밀 키처럼 보호하십시오.

## 함께 보기

- `http_*`는 [웹 서버와 HTTP](../reference/web.md)를 보십시오.
- `lingtp://`와 `p2p_*` 뒤의 기본 연산은 [암호화](../reference/crypto.md)를 보십시오.
