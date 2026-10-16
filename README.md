# parley
🌐 Open-source real-time multilingual caption translator for Video Meet. Context-aware AI translations (OpenAI, Gemini, Claude, DeepL) with Bring-Your-Own-Key support.

## Structure

| Path | Contents |
|---|---|
| `server/` | Rust (axum) — rooms, WebRTC signaling, (later) translation pipeline |
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

Server environment variables:

| Variable | Default | Description |
|---|---|---|
| `BIND_ADDR` | `127.0.0.1:8080` | Listen address |
| `ICE_SERVERS` | Google public STUN | JSON array passed to `RTCPeerConnection`. Served publicly by `/api/config`, so static TURN `username`/`credential` fields are refused at startup |
| `RUST_LOG` | `info` | Log filter |
| `HOST_ACCOUNTS` | (none) | Host accounts: `id:argon2-hash`, space-separated. If empty, nobody can create meetings |
| `COOKIE_SECURE` | `true` | Session cookie `Secure` flag. Use `false` only for local http development |
| `TRUSTED_PROXY` | (unset) | Set to `1` only behind a reverse proxy (Caddy) that sets `X-Forwarded-For`; login throttling then uses its last hop as the client IP. Otherwise the socket peer address is used |

Protocol types are defined in `server/src/protocol.rs`; `cargo test --manifest-path server/Cargo.toml` regenerates `web/src/protocol/*.ts`.
