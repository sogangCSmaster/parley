# parley

Open-source video meetings with context-aware multilingual captions powered by Google Gemini.

Parley uses **Google Gemini as its only AI provider**, with one API key configured by the server operator. Video, audio and screen sharing use peer-to-peer WebRTC.

## Current status

- **Implemented:** host login, meeting links, guest admission, video/audio calls, screen sharing and a multilingual caption panel.
- **Implemented:** server-side STT text → sentence buffering → Gemini streaming translation → captions, with recent sentence context and one translation per target language. Translated text appears from the first chunk; failed or incomplete translations fall back to the original.
- **Planned:** browser microphone capture for Gemini live transcription and server-issued ephemeral tokens. Microphone-to-caption translation is not connected yet; setting an API key alone does not enable it.

The Gemini key belongs in the server environment or the repository root `.env`. Participants do not supply individual keys.

## Structure

| Path | Contents |
|---|---|
| `server/` | Rust (axum) — rooms, WebRTC signaling, translation pipeline |
| `web/` | React + Vite + TypeScript — home, lobby, meeting room, caption panel |
| `design/` | Design tokens (`tokens.css`), rules, static preview |
| `docs/ARCHITECTURE.md` | Architecture and deployment (EC2) plan |

## Local development

Requires Rust (stable) and Node 20+ / pnpm.

```bash
cargo run --manifest-path server/Cargo.toml
```

```bash
pnpm --dir web install && pnpm --dir web dev
```

Only **hosts** can create meetings. Guests open the meeting link and **ask to join**, and get in once the host admits them.

### Registering host accounts

Accounts are registered by the server operator via an environment variable (no DB). Run the command below, type the password and press Enter, and it prints the hash (the password isn't left in your shell history):

```bash
cargo run -q --manifest-path server/Cargo.toml -- hash-password
```

Put the output into `HOST_ACCOUNTS` as `id:hash` (separate multiple accounts with spaces or newlines). Sessions live in server memory, so a server restart means logging in again.

Open http://localhost:5173, log in via **Host login**, click **New meeting**, and open the generated link in a different browser (or a private window).
The Vite dev server proxies `/api` and `/ws` to the Rust server on `127.0.0.1:8080`.

For local secrets, put `GEMINI_API_KEY=your-key` in the repository root `.env` (see `.env.example`).
The Rust server loads this file before starting its runtime; existing environment variables take precedence.
The Gemini benchmark also reads the root file, with `bench/gemini/.env` as a legacy fallback.
`.env` is ignored by Git. Keep this key server-side; never give it a `VITE_` prefix.
The server translates STT messages with this key; browser microphone-to-STT capture is still planned, so loading the key alone does not enable microphone captions yet.

Server environment variables:

| Variable | Default | Description |
|---|---|---|
| `BIND_ADDR` | `127.0.0.1:8080` | Listen address |
| `ICE_SERVERS` | Google public STUN | JSON array passed to `RTCPeerConnection`. Served publicly by `/api/config`, so static TURN `username`/`credential` fields are refused at startup |
| `RUST_LOG` | `info` | Log filter |
| `HOST_ACCOUNTS` | (none) | Host accounts: `id:argon2-hash`, space-separated. If empty, nobody can create meetings |
| `COOKIE_SECURE` | `true` | Session cookie `Secure` flag. Use `false` only for local http development |
| `GEMINI_API_KEY` | (none) | Translation API key (server only, never sent to browsers). If empty, captions show the original only |
| `GEMINI_TRANSLATE_MODEL` | `gemini-3.5-flash-lite` | Translation model |
| `TRUSTED_PROXY` | (unset) | Set to `1` only behind a reverse proxy (Caddy) that sets `X-Forwarded-For`; login throttling then uses its last hop as the client IP. Otherwise the socket peer address is used |

Per-step translation latency (first token, total, STT → translation) is logged at debug level: `RUST_LOG=info,parley_server::pipeline=debug`. Caption text is never logged.

Protocol types are defined in `server/src/protocol.rs`; `cargo test --manifest-path server/Cargo.toml` regenerates `web/src/protocol/*.ts`.

### Verification

Run `cargo test` and `cargo clippy --all-targets` in `server/`, and `pnpm test`, `pnpm build`, and `pnpm lint` in `web/`.
The browser regression script uses an optional existing Playwright installation and a running Vite server:

```bash
PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs node web/tests/browser-regressions.mjs
PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs node web/tests/browser-mesh.mjs
```

These scripts check waiting-room disconnects, failed admissions, mobile caption controls, mute preservation and capture cleanup, plus real WebRTC connections with and without local devices. Signaling and the screen picker are mocked. `TEST_ORIGIN` overrides the default `http://127.0.0.1:5173`.
