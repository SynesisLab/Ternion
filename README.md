# Ternion

One interface. Three models. The right model for every message.

> **Ternion is a proof of concept for the Triad model routing system** — a
> Herald / Scout / Titan architecture where a lightweight classifier routes
> each message to the right model (and, since M3, the right endpoint).

Ternion is a local-first Windows desktop app for chatting and working with LLMs.
It speaks native Ollama and any OpenAI-compatible endpoint. See
[DESIGN.md](./DESIGN.md) for the full product design (the Triad system:
Herald / Scout / Titan routing).

## Status — M0–M4 complete (Triad router, tools, vision, polish)

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

**Tools & file-system agency (M2)** — DESIGN.md §6:

- **Workspaces**: bind up to 3 folders per chat (header bar); with no
  workspace bound the model has zero file access — the `tools` array is not
  even declared
- **Windows path guard (§6.3)**: every model-supplied path resolves through
  canonicalize + case-insensitive prefix checks — no verbatim-prefixed
  drives, no `..` escapes, no device names (`CON`, `NUL`), no junction
  escapes; creation paths probe the deepest existing ancestor
- **Read tools (auto-allowed)**: `fs_list`, `fs_read` (numbered lines,
  offset/limit), `fs_stat`, `fs_search` (bundled ripgrep engine — regex,
  glob filter, honors `.gitignore`), `fs_tree`; every result capped with an
  explicit truncation note
- **Mutating tools (permission matrix §6.6)**: `fs_write` (atomic
  tmp+rename, append mode), `fs_edit` (exact-string replace), `fs_move`,
  `fs_copy`, `fs_delete` (Recycle Bin only — never permanent), `fs_mkdir`
- **Approvals**: Ask / Allow-for-session / Always-for-folder per
  (tool × workspace); an approval modal with a unified diff and the full
  resulting content for fs_write/fs_edit, including Edit-in-place (the
  user's version becomes the tool's own argument); the stream pauses, never
  cancels, while a request is pending; Stop denies the pending call.
  Managed in Settings → Permissions
- **Shell tool (opt-in)**: `shell` runs PowerShell (7 → 5.1 fallback) with
  the cwd pinned inside the workspace; output capped at 8 KB, killed on
  timeout, and every single command is approved — shell commands are never
  whitelisted
- **Tool loop**: validate → permission gate → execute → `<tool_result
  source="untrusted">` fed back to the model, up to `tools.max_hops` (12)
  then a forced no-tools summary; every call persisted with its outcome and
  permission mode; live tool activity renders in the chat

**Vision & multi-endpoint (M3)** — DESIGN.md §5, §7:

- **Endpoint profiles**: named Ollama / OpenAI-compatible endpoints
  (Settings → Endpoints) with base URL, optional API key, custom headers,
  enable toggle, and a connection test that lists discovered models
- **OpenAI-compatible adapter**: SSE streaming (with tool calls) compiled to
  the same normalized stream protocol as the Ollama adapter
- **Model refs**: a bare name means built-in Ollama; `model@endpoint_id`
  means a profile — Triad roles, pins, and capability records all resolve
  per endpoint, and a mid-conversation upgrade can move turns across
  providers
- **Capability registry**: per (endpoint, model) facts — capabilities,
  context length, role, VRAM estimate — verified via Ollama `/api/show` or
  edited by hand (Settings → Models tab)
- **Image attachments (§7.2)**: paste, drag-drop, file picker, or the region
  capture below → decode → EXIF orientation fix → downscale (long edge
  ≤ 1568) → JPEG q85; files stored sha256-keyed next to the database, DB
  rows keep paths only, and the base64 body is hydrated fresh for each
  provider hop
- **Vision routing (§7.6)**: any attached image forces a vision-capable
  model — auto turns upgrade (Scout → vision Scout → Titan → first capable
  registry model), pinned turns surface a warning chip with a one-click fix
  instead of being overridden
- **Region capture**: `Win+Alt+S` opens a crosshair overlay over the monitor
  under the cursor; the drag rect (Esc cancels) goes through the IMG
  pipeline and lands in the composer as a pending attachment chip

**Polish (M4)** — DESIGN.md §3.11, §9.3, §9.5, §14:

- **Triad report** (Settings → Triad): fleet-wide aggregates over every
  routing event and completed turn — decision sources (Herald / heuristics /
  hard rules / manual), escalation & de-escalation counts, overrides, average
  Herald latency, per-role turns / latency / token spend, and a "time saved
  vs always-Titan" estimate with the observed baseline it used
- **Adaptive tuning** (opt-in): a pin that contradicts the conversation's
  latest auto decision nudges that flag class's escalation threshold — the
  learned bumps (with their override counts) show in the report and can be
  reset
- **Quick capture (Win+Alt+T)**: a frameless always-on-top palette at the
  top-right of the screen; asks run through the full Triad pipeline into
  real persisted conversations — routing ribbons, image paste, region
  capture, and a one-click hand-off to the main window
- **Local-only mode** (Settings → General, or the tray): a privacy toggle
  that suppresses every cloud endpoint — cloud pins/roles fall back to the
  best local model for the role, Herald is treated as down (heuristics
  decide), vision upgrades never leave built-in Ollama, and when no local
  model can serve, the turn fails loudly instead of silently leaking
- **Traditional Chinese**: a complete zh-TW catalog with a Language setting
  (model codenames stay English)
- **Updater + packaging (§14)**: NSIS per-user installer
  (English/繁體中文), and the tauri updater armed against GitHub Releases —
  dormant until the signing keypair is generated (see
  [Packaging & updates](#packaging--updates-14-m4))

**Post-v1 backlog (all done)** — the §6.5/§5.6 items that stayed on the
backlog after M4:

- **Light theme (§10)**: a token-swapped light palette following the system
  mode, with a flash-free boot (theme applies before the first paint)
- **MCP client (§6.5)**: local stdio servers configured in Settings; their
  tools merge namespaced `mcp__<server>__<tool>` and gate like the shell —
  ask per call, grantable per tool, session cache dropped on config change
- **Context compression (§6.5c)**: when the assembled context approaches the
  routed model's window, older history is progressively summarized (Herald
  when assigned, else the routed model) into a stored summary that folds in
  each pass; the recent tail stays verbatim and failures degrade to plain
  truncation with a note — never blocking
- **OpenWebUI tools (§6.5b)**: OpenWebUI "Tools" manifests (Python
  `class Tools`) parse into JSON-schema specs, merge namespaced
  `owui__<manifest>__<method>`, and execute through a local Python
  interpreter via a generated shim — same §6.6 gate as MCP tools
- **Provider hand-off prompt (§5.6)**: endpoints are probed on a timer; when
  one comes online the UI offers the switch (Switch now · Keep current ·
  Never for this endpoint, "Always" auto-applies) — local-only mode
  suppresses non-local offers, cloud offers carry a cost note, and a switch
  rides the §3.6 digest hand-off machinery

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

The SQLite database lives at `%LOCALAPPDATA%\com.paperplane.ternion\ternion.db`,
attachment files under `attachments\` beside it; logs at
`%LOCALAPPDATA%\com.paperplane.ternion\logs\ternion.log`.

## Build

```sh
npm run tauri build
```

Produces an NSIS installer (per-user install, English/繁體中文) under
`src-tauri/target/release/bundle/nsis/` (unsigned — SmartScreen will warn;
code signing is a release-step decision, see below).

## Packaging & updates (§14 M4)

The updater is armed but dormant until a release pipeline exists: updates
are pulled from `SynesisLab/Ternion` GitHub Releases (`latest.json` next to
the installer assets), verified with a minisign key, and installed
passively on Windows. To arm it for a real release:

1. Generate the signing keypair (keep the private key off-repo, e.g. in a
   password manager / CI secret):
   ```sh
   npm run tauri signer generate -w ./ternion.key
   ```
2. Put the generated public key into `plugins.updater.pubkey` in
   `src-tauri/tauri.conf.json` (replacing the placeholder).
3. Build with the private key available as an environment variable
   (`.env` files do not work for this):
   ```sh
   TAURI_SIGNING_PRIVATE_KEY=$(cat ternion.key) npm run tauri build
   ```
   This produces the signed `.nsis.zip` updater artifact next to the
   installer.
4. Publish: attach installer + signed updater artifact + a `latest.json`
   manifest to a GitHub Release (the manifest is written automatically
   when `createUpdaterArtifacts` is on, with `-should-sign` metadata).

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