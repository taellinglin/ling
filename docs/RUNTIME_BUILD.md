# Building the Ling runtime for Ling For Fun (Windows)

This snapshot builds the native `ling` executable used to run the game
(`ling run --O0 <file>`, with `--interp` as the fallback).

## Toolchain

* Rust **1.98.0** (pinned in `rust-toolchain.toml`; `rustup` installs it on demand)
* MSVC build tools (Visual Studio 2022 Build Tools, "Desktop development with C++")
* No host-specific flags: no `.cargo/config`, no `RUSTFLAGS`/`target-cpu`; output is portable x86-64.

## Build

```powershell
$env:CARGO_TARGET_DIR = "$PWD\target-game"      # keep it out of other checkouts
cargo build --locked --release --bin ling --features "game gl"
if ($LASTEXITCODE -ne 0) { throw "cargo failed ($LASTEXITCODE)" }
```

Output: `target-game\release\ling.exe`.

Keep `lto = false` (set in `Cargo.toml`): fat/thin LTO miscompiles or hangs the JIT.

Do not run cargo as `cargo ... 2>&1 | Select-Object -Last N` under Windows
PowerShell and judge it by the shell result: PowerShell turns cargo's stderr
warnings into `NativeCommandError` records, so a successful build can look
like "exit 1 but Finished". Check `$LASTEXITCODE` (or redirect stderr to a
file) instead.

`LING_GPU=1` enables the GPU rasteriser at run time (feature `gl`); without it
the CPU rasteriser is used.

## `net_host` bind address

```
net_host(port)              # listens on 0.0.0.0:port (unchanged default)
net_host(port, "127.0.0.1") # loopback only
net_host(port, "::1")       # IPv6 loopback
net_host_addr()             # "ip:port" the listener really bound; "" until bound or if the bind failed
```

The optional second argument must be an IP literal; anything else
(`"localhost"`, `"999.1.1.1"`, `"127.0.0.1:80"`) raises a runtime error. A
bind failure leaves `net_status()` at `0` as before. Under `--interp` an invalid
address is a runtime error; under the default JIT / `--O0` path the runtime's
builtin bridge turns every builtin error into unit (existing behavior), so the
call does nothing -- no socket is opened -- and `net_host_addr()` stays `""`.
Scripts that need certainty should check `net_host_addr()`.
`net_host_addr()` is
filled in by the listener thread, so poll it briefly after `net_host`.

## Not included

`crates/ling-kernel` depends on the unpublished `ling-messenger` crate and is
excluded from the workspace; it is not needed for the `game`/`gl` build.
