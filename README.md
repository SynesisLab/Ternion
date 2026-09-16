# Ternion

One interface. Three models. The right model for every message.

Ternion is a local-first Windows desktop app for chatting and working with LLMs.
It speaks native Ollama and any OpenAI-compatible endpoint. See
[DESIGN.md](./DESIGN.md) for the full product design (the Triad system:
Herald / Scout / Titan routing).

## Status — M0 (Foundation) + M1 (Triad router) complete

**Foundation (M0)**

- **Streaming chat** with any Ollama model: token-by-token rendering, thinking
  blocks (`thinking` models), Stop mid-stream (frees the Ollama slot), usage
  (tokens in/out, latency) on every message
- **Conversations**: create / rename / delete / search; auto-titled from the
  first message; full history persisted in SQLite (WAL)
- **Markdown**: GFM tables, syntax-highlighted code, links open the system
  browser (model HTML is escaped — no raw HTML rendering)
- **Models**: live discovery from `/api/tags` with capability badges
  (vision / tools / thinking)
- **Settings**: Ollama base URL (+ connection test), generation defaults
  (temperature, context tokens, keep-alive), close-to-tray
- **Tray**: Show/Hide · New Chat · Quit; close-to-tray keeps streams alive;
  single-instance (second launch focuses the existing window)

**Triad router (M1)** — the Herald / Scout / Titan system from DESIGN.md §3:

- **Routing precedence**: hard rules → Herald (structured classify call,
  confidence ≥ 0.65) → heuristics → Scout default; every decision persisted
  to a `routing_events` table and joined back into the message history
- **Pins**: Auto / Scout / Titan chips, or pin an explicit model; per-role
  model assignment in Settings → Triad
- **Handoff**: on model switch between turns the new model receives a rolling
  digest of earlier task state plus the router's handoff note; the ribbon
  marks the switch (⚡)
- **Hysteresis**: Scout output past its ceiling offers "Continue with Titan";
  Titan sticks for `sticky_turns` before de-escalating
- **Sidecars** (Herald-only, fire-and-forget): conversation title refinement,
  digest maintenance, follow-up suggestion chips
- **Router log**: per-conversation drawer showing every decision's source,
  confidence, estimated tokens, and override kind

Later milestones: tools & file-system agency (M2), vision & multi-endpoint
(M3), polish/i18n/signing (M4).

## Prerequisites

- Windows 10 21H2+ / Windows 11
- [Node.js](https://nodejs.org) 20+ (npm)
- [Rust](https://rustup.rs) — `stable-msvc` toolchain + Visual Studio 2022
  Build Tools with the "Desktop development with C++" workload
- [Ollama](https://ollama.com) running locally (default `http://127.0.0.1:11434`)
- WebView2 runtime (preinstalled on Windows 11)

## Development

```sh
npm install
npm run tauri dev
```

The SQLite database lives at `%APPDATA%\com.paperplane.ternion\ternion.db`;
logs at `%APPDATA%\com.paperplane.ternion\logs\ternion.log`.

## Build

```sh
npm run tauri build
```

Produces an NSIS installer under `src-tauri/target/release/bundle/nsis/`
(unsigned until M4 — SmartScreen will warn; that's expected).

## Tests

```sh
cd src-tauri
cargo test            # unit tests — no Ollama needed
cargo test -- --ignored   # live probes against a running Ollama
```

Architecture notes for contributors: `src-tauri/src/chat.rs` owns one-stream
orchestration (guards, placeholders, throttled persistence, cancellation);
`src-tauri/src/providers/` compiles every provider to the normalized stream
protocol in `types.rs` / `src/types/stream.ts` (a unit test guards protocol
drift); the frontend store refetches from the DB after every stream — the DB
is the source of truth.