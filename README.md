# zed-mplab-debug

A [Zed](https://zed.dev) debug adapter extension for Microchip MPLAB targets
(PIC, AVR, dsPIC, SAM). It wraps the Java/NetBeans **MPLAB backend** — the
same engine used by the VS Code `mplab-core-da` extension — so Zed can debug
Microchip firmware over the Debug Adapter Protocol (DAP).

> **Status: early development.** Bootstrap, DAP spawn, HTTP cache, and minimum
> pack-management scaffolding are in place. Hardware validation is still pending.
> See the roadmap below.

## How it works

Zed talks DAP over TCP to the MPLAB RCP backend. The extension's only job is
to:

1. Make sure the RCP backend and a compatible JRE are installed (downloaded
   on first run from `shelf.download.microchip.com`).
2. Make sure the Device Family Pack for the selected device is installed
   (downloaded from `packs.download.microchip.com`, with ETag + Cache-Control
   aware caching).
3. Spawn the backend in DAP-direct mode
   (`-J-Ddebug.adapter.protocol.server.port=<P>`) and return the connection
   info to Zed, which then speaks DAP to the backend directly.

Named pipes, an IPC controller channel, and the backend's Tool/Pack services
over IPC are all deliberately **not** used — DAP-direct is the only transport.

## Configuration

Add a debug configuration to `.zed/debug.json`:

```json
{
  "adapter": "mplab",
  "label": "Flash & debug",
  "request": "launch",
  "program": "${ZED_WORKTREE_ROOT}/build/firmware.elf",
  "device": "PIC18F47Q10",
  "tool": "PKOB nano"
}
```

Planned settings (Zed API 0.7 does not publicly expose custom settings yet;
defaults are currently used internally):

| Key | Purpose | Default |
|---|---|---|
| `mplab.rcpPath` | Absolute path to a pre-installed `mplab_backend64.exe` (or Unix equivalent). Bypasses download. | *(auto-install)* |
| `mplab.javaHome` | Absolute path to a pre-installed JDK/JRE. Bypasses download. | *(auto-install)* |
| `mplab.userDir` | NetBeans RCP `--userdir`. | *(extension work dir)* |
| `mplab.cacheDir` | NetBeans RCP `--cachedir`. | *(extension work dir)* |
| `mplab.packRepo` | Directory passed as `-J-Dpackslib.packsfolder`. | *(extension work dir)* |
| `mplab.shelfUrl` | Shelf manifest URL. | `https://shelf.download.microchip.com/shelf.json` |
| `mplab.packIndexUrl` | Pack index URL. | *(TBD — see roadmap)* |
| `mplab.jreName` | Shelf application id for the JRE. | `zulu-jre-25` |
| `mplab.logLevel` | Java logging level (OFF/SEVERE/WARNING/INFO/CONFIG/FINE/FINER/FINEST/ALL). | `WARNING` |
| `mplab.symbolLoading` | `on-demand` or `pre-processed`. | `on-demand` |
| `mplab.extraArgs` | Extra args appended to the backend command line. | `[]` |

Zed's built-in `dap.mplab.binary` / `dap.mplab.args` overrides are also
honored.

## Roadmap

- [x] Phase 0 — skeleton.
- [x] Phase 1 — bootstrap (shelf fetch + RCP/JRE install).
- [x] Phase 2 — DAP spawn + full launch schema.
- [x] Phase 3 — unified HTTP cache (Cache-Control + ETag + lock).
- [x] Phase 3b — minimum pack service port.
- [ ] Phase 4 — settings defaults/wiring.
- [x] Phase 5 — tests + CI.

## Building

```bash
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release
```

Load as a dev extension in Zed via `zed: install dev extension` pointed at
this repository root.

## License

Apache-2.0.
