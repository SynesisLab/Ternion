# Ternion

One interface. Three models. The right model for every message.

Ternion is a local-first Windows desktop app for chatting and working with LLMs.
It speaks native Ollama and any OpenAI-compatible endpoint. See
[DESIGN.md](./DESIGN.md) for the full product design (the Triad system:
Herald / Scout / Titan routing).

## Status — M0 (Foundation) complete

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

Later milestones: Triad router — Herald/Scout/Titan (M1), tools & file-system
agency (M2), vision & multi-endpoint (M3), polish/i18n/signing (M4).

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