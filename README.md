# vpnstat-overlay

**A 340 KB, zero-network-traffic Windows overlay that shows live VPN tunnel speed and connection health.**

No driver. No admin rights. No config files. Does not read or modify your VPN client.
Reads kernel counters only — **0 bytes** of network traffic.

[![CI](https://github.com/OWNER/vpnstat-overlay/actions/workflows/ci.yml/badge.svg)](https://github.com/OWNER/vpnstat-overlay/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![rust](https://img.shields.io/badge/rust-1.99%2B-orange.svg)](https://www.rust-lang.org)
[![size](https://img.shields.io/badge/binary-343_KB-brightgreen.svg)](https://github.com/OWNER/vpnstat-overlay/releases)
[![platform](https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-blue.svg)](https://github.com/OWNER/vpnstat-overlay/releases)

![screenshot](docs/screenshot.png)

---

## What it does

A small always-on-top panel, click-through, that sits on top of everything and shows:

```
●  ↓ 42.6 MB/s   ↑ 3.1 MB/s
   stable
```

| Colour | Meaning | Triggered by |
|---|---|---|
| 🟢 Green | `stable` | No packet loss, tunnel link up |
| 🟡 Yellow | `unstable` | Loss was observed, or very spiky throughput (CV ≥ 2.0) |
| 🟠 Orange | `dropped` | 4+ consecutive seconds with loss, or the tunnel link fell |
| 🔴 Red | `leak: traffic bypassing VPN` | Tunnel does **not** hold the default route, or it holds it but no byte passes for 5+ s while the physical NIC is busy |
| ⚪ Grey | `VPN is down` | No tunnel adapter found |

## Why this exists

Every network-speed widget I tried either burned 50–100 MB of RAM (Python + Qt), needed a
kernel driver, or sent probe traffic to measure "stability". This does none of those:

| Tool | RAM | Network traffic | Admin |
|---|---|---|---|
| NetSpeedTray | ~50 MB | latency probe (opt-in) | no |
| TrafficMonitor | ~10 MB | none | Lite: no |
| **this** | **1.7 MB private** | **none** | **no** |

It answers one question cheaply: *is my tunnel actually carrying my traffic right now, or is
something going around it?*

## How it works

Two Windows APIs, nothing else:

- **`GetIfTable2`** — per-adapter byte/discard/error counters. Speed is the delta per sample.
- **`GetIpForwardTable`** — which interface owns the default route. This is the actual
  evidence for the red state.

The tunnel is identified by adapter name/type match (`sing-box`, `clash`, `hiddify`, `wintun`,
`wireguard`, `xray`, …) or `IF_TYPE_TUNNEL`, preferring whichever candidate holds the default
route. Configs of the VPN client are never opened.

Rendering is plain GDI into a 32-bpp top-down DIB, with an SDF-computed alpha mask for the
rounded corners, blitted with `UpdateLayeredWindow`. No GDI+, no web view, no Qt.

## Install

Grab `vpnstat.exe` from [Releases](https://github.com/OWNER/vpnstat-overlay/releases) and run
it. That is the whole install — it is a single self-contained binary.

Requires Windows 10 1903+ / Windows 11, x64. No installer, no dependencies.

```
vpnstat.exe                # default position: zone 5 (bottom-right cell centre)
vpnstat.exe --zone 1..6    # pick a cell of the 3x2 screen grid
vpnstat.exe --interval 500 # poll interval in ms (default 1000, min 200)
VPNSTAT_DEBUG=1            # write vpnstat.log next to the exe
```

Drag the panel anywhere; the position persists in `%APPDATA%\vpnstat\pos.txt`.
Zones are numbered bottom-up per column, so **zone 5 is the bottom of the right column**.

| Key | Action |
|---|---|
| `Ctrl+Alt+V` | hide / show |
| `Ctrl+Alt+Q` | quit |

Click-through is disabled automatically while the cursor is over the panel, which is when the
close button and drag appear.

## Measured footprint

Windows 11, 2256×1504 @ 200% scaling, tunnel active:

| Mode | Working set | Private | CPU (one core) | Handles |
|---|---|---|---|---|
| Idle, 60 s | 7.19 MB | 1.69 MB | 0.8 % | 89 |
| Under load | 7.18 MB | 1.68 MB | 0.4 % | 104 |
| After 2.5 min | 7.21 MB | 1.69 MB | — | 89 |

The gap between working set and private is shared `gdi32`/`user32` pages, which the task
manager counts against the process. Actual owned memory is the ~1.7 MB private figure.
One thread. No leaks: handle count is flat over time, every GDI object is released.

## Limitations — read this

Stated plainly, because a health indicator that lies is worse than none:

1. **Latency and jitter are not measured.** Measuring them requires sending probes (ICMP or a
   TCP connect). This tool sends zero bytes, so "stability" here means **packet loss**
   (`discards`/`errors` on the tunnel adapter), which is the only trustworthy local signal.

2. **It cannot tell you *which* application is leaking.** Windows does not attribute bytes per
   process without a kernel driver. Red means *the routing evidence says traffic is not going
   through the tunnel* — not *app X is leaking*.

3. **DPI blocking of your server is not detected.** Locally you only see the symptom (handshake
   fails). Telling "DPI is resetting me" apart from "the server is down" needs active probing.

4. **Throughput variance mostly reflects your traffic, not the link.** The yellow state is
   therefore driven by loss counters, with CV used only as a secondary hint at a very high
   threshold.

## Building

Needs a Rust toolchain (GNU host) and mingw for `windres`:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:PATH = "$env:CARGO_HOME\bin;<mingw>\bin;$env:PATH"
cargo build --release
```

The icon is generated, not hand-drawn. Regenerate it with:

```powershell
powershell -ExecutionPolicy Bypass -File make_icon.ps1
```

`build.rs` compiles `vpnstat.rc` with `windres` and links the resulting `.res`. It passes
**relative** paths to windres on purpose: windres shells out to `cc1`, which does not quote
paths containing spaces.

CI builds and smoke-tests on GitHub's Windows runners — that is also how this gets verified on
a machine other than the author's.

## License

MIT. See [LICENSE](LICENSE).

Not affiliated with or endorsed by any VPN provider. Does not circumvent censorship or detect
DPI; it only observes adapter counters and routing state.
