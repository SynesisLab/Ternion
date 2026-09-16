# Ternion

One interface. Three models. The right model for every message.

Ternion is a local-first Windows desktop app for chatting and working with LLMs.
It speaks native Ollama and any OpenAI-compatible endpoint. See
[DESIGN.md](./DESIGN.md) for the full product design (the Triad system:
Herald / Scout / Titan routing).

## Status

**M0 — Foundation** (Tauri shell, Ollama adapter, single-model streaming chat,
SQLite, markdown renderer, tray). Later milestones add the Triad router (M1),
tools & file-system agency (M2), vision & multi-endpoint (M3), polish (M4).

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

The SQLite database lives at `%APPDATA%\com.paperplane.ternion\ternion.db`.

## Build

```sh
npm run tauri build
```

Produces an NSIS installer under `src-tauri/target/release/bundle/nsis/`
(unsigned until M4).