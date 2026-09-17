# Ternion

One interface. Three models. The right model for every message.

> **English** | [繁體中文](./README.zh-TW.md)

**Ternion** is a local-first Windows desktop app for chatting and working
with LLMs. It speaks native Ollama and any OpenAI-compatible endpoint —
and instead of making you pick a model for every message, its **Triad
routing system** decides: a small fast model for small asks, a large
capable model for hard work, and a lightweight classifier that routes
each message to the right one, per message, automatically.

## The Triad system

Ternion is the reference implementation of the **Triad model routing
system** (Herald / Scout / Titan):

- **Herald** — a lightweight classifier that reads every message and
  decides who answers (one structured-output call, temperature 0,
  confidence-gated)
- **Scout** — the small, fast worker: greetings, quick lookups, short
  rewrites, single-file edits
- **Titan** — the large, capable worker: multi-step reasoning,
  architecture-level coding, long documents, multi-tool agent work

Routing precedence: hard rules → Herald → heuristics → Scout default.
Every decision is persisted and inspectable. When Herald is unsure, the
confidence floor falls through to heuristics; hard rules always correct
capability gaps (an image forces a vision-capable model, for example).

What you see as a user: a per-message ribbon showing who answered and
why, pin chips (Auto / Scout / Titan / a specific model), a per-chat
**router log**, and a **Triad report** of fleet-wide aggregates. Model
switches ride a rolling-digest handoff so the new model picks up the
task, and Scout overruns offer a one-click "Continue with Titan".

## Highlights

- **Streaming chat** — token-by-token output, thinking blocks for
  thinking models, Stop that actually frees the Ollama slot, and
  per-message usage (tokens in/out, latency)
- **Full conversation history** in SQLite (WAL) — create / rename /
  delete / search, auto-titled
- **File-system agency** — bind up to 3 workspace folders per chat; 12
  bundled `fs_*` tools behind a Windows path guard (canonicalize +
  case-insensitive prefix checks); mutating tools go through an approval
  matrix (ask / allow-for-session / always-for-folder) with unified
  diffs; opt-in `shell` tool, every command approved
- **Extensible tools** — local stdio MCP servers and **OpenWebUI Skills**
  (the Python `class Tools` manifests, compatible with the format
  OpenWebUI renamed from Tools to Skills) merge into the same
  namespaced, permission-gated tool surface
  (`mcp__<server>__<tool>`, `owui__<manifest>__<method>`). OpenWebUI
  **Functions** work too: a **Filter**'s `inlet` transforms the assembled
  request body before the model sees it (best-effort, composed in stored
  order), and a **Pipe** runs as a selectable pseudo-model whose output
  streams into the chat like any model's.
- **Context compression** — when the routed model's window fills, older
  history is progressively summarized into a stored rolling summary;
  the recent tail stays verbatim and failures degrade to plain
  truncation, never blocking
- **Vision** — paste, drop, pick, or capture (Win+Alt+S) images; EXIF
  fixes, ≤1568 downscale, JPEG q85; vision-aware routing upgrades the
  model when an image arrives
- **Multi-endpoint** — named Ollama / OpenAI-compatible profiles with
  API keys stored in the Windows Credential Manager, a per-(endpoint,
  model) capability registry, and cross-provider model refs
  (`model@endpoint_id`)
- **Provider hand-off prompt** — endpoints are probed on a timer; when
  one comes online you're offered the switch (switch now · keep
  current · never), with cost notes for cloud and suppression in
  local-only mode
- **Quick capture palette** — Win+Alt+T opens a frameless always-on-top
  ask bar that runs the full Triad pipeline into real conversations
- **Local-only mode** — one toggle (settings or tray) that suppresses
  every cloud endpoint; routing falls back to the best local model and
  fails loudly rather than leaking when nothing local can serve
- **Light / dark / system themes**, flash-free boot, and a complete
  繁體中文 (zh-TW) interface
- **Windows-native packaging** — NSIS per-user installer
  (English/繁體中文), single-instance, close-to-tray with live streams,
  and a dormant tauri updater aimed at GitHub Releases

## Install

Prebuilt installers are attached to
[GitHub Releases](https://github.com/SynesisLab/Ternion/releases/latest)
(`Ternion_0.1.0_x64-setup.exe`, per-user install, English/繁體中文).
The binaries are unsigned at the moment — SmartScreen will ask; choose
*More info → Run anyway*.

To build from source instead:

```sh
npm install
npm run tauri build
```

The installer lands in `src-tauri/target/release/bundle/nsis/`, the
standalone executable in `src-tauri/target/release/`.

Prerequisites: Windows 10 21H2+ / Windows 11, [Node.js](https://nodejs.org)
20+ (npm), the Rust `stable-msvc` toolchain + Visual Studio 2022 Build
Tools with the C++ desktop workload, [Ollama](https://ollama.com) running
locally (default `http://127.0.0.1:11434`), and the WebView2 runtime
(preinstalled on Windows 11). For development:

```sh
npm install
npm run tauri dev
```

## Hotkeys

| Combo      | Action                                          |
| ---------- | ----------------------------------------------- |
| `Win+Alt+S`| Region capture → image attachment in the composer|
| `Win+Alt+T`| Quick-ask palette (top-right of the screen)     |

Both are registered globally: if another app already owns a combo,
Ternion logs a warning at boot and starts without it — the app is not
affected, only that shortcut is.

## Where your data lives

Everything is local-first, under
`%LOCALAPPDATA%\com.paperplane.ternion\`:

- `ternion.db` — conversations, messages, routing events, tool calls
  (SQLite, WAL)
- `attachments\` — processed image files (DB rows store paths only)
- `logs\ternion.log` — app + webview diagnostics

Nothing leaves the machine unless you point it somewhere else: with no
cloud endpoint configured and local-only mode available in the tray,
Ternion talks to your local Ollama. Model images are stored locally as
sha256-keyed files; API keys go to the Windows Credential Manager, never
to the database.

## Tests

```sh
cd src-tauri
cargo test            # unit tests — no Ollama needed
cargo test -- --ignored   # live probes against a running Ollama
```

The frontend builds type-checked through `npm run build`.

## Architecture notes for contributors

`src-tauri/src/chat.rs` owns one-stream orchestration (guards,
placeholders, throttled persistence, cancellation);
`src-tauri/src/providers/` compiles every provider to the normalized
stream protocol in `types.rs` / `src/types/stream.ts` (a unit test
guards protocol drift); `src-tauri/src/router/` holds the Triad router —
Herald classification, the heuristic policy engine, handoff digests, and
context compression. The frontend store refetches from the database
after every stream — the DB is the source of truth. The full product
design lives in [DESIGN.md](./DESIGN.md).

## Roadmap

Deliberately out of scope for now: OpenWebUI filter valves and outlet
transforms (the deeper half of Functions middleware), mid-stream provider
failover retry, mDNS endpoint discovery, and code signing for the
installer.

## Releases & updates

The tauri updater is armed but dormant until a real release pipeline
exists. To arm it:

1. Generate the signing keypair (keep the private key off-repo):
   ```sh
   npm run tauri signer generate -w ./ternion.key
   ```
2. Put the public key into `plugins.updater.pubkey` in
   `src-tauri/tauri.conf.json` (replacing the placeholder).
3. Build with the private key as an environment variable
   (`.env` files do not work for this):
   ```sh
   TAURI_SIGNING_PRIVATE_KEY=$(cat ternion.key) npm run tauri build
   ```
   This produces the signed `.nsis.zip` updater artifact next to the
   installer.
4. Publish: attach installer + signed updater artifact + the generated
   `latest.json` manifest to a GitHub Release.

## License

[MIT](./LICENSE) — © 2026 SynesisLab