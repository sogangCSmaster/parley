# AGENTS.md

Project-specific rules. Structure and run commands are in [README.md](README.md); design is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [design/DESIGN_SYSTEM.md](design/DESIGN_SYSTEM.md).

## Blazing fast is a feature

Speed is part of product quality. Every change is judged first on whether it makes things faster.

- **Latency budget**: end of speech → first translated caption character within 1.5s. Measure the time spent at each step.
- **Pick the light option first**: before adding a dependency, check whether the platform/standard API or a few dozen lines of code can do the job. Weigh bundle size against what it brings in (e.g. i18n is homegrown, not i18next).
- **Remove network round-trips**: if a few KB of resources are needed on first render, put them in the bundle. Lazy-load only large resources that are needed later.
- **Fetch only what's needed**: icons are Material Symbols SVG paths put in the bundle (`web/src/components/icons.ts`); there is no icon font request. When adding an icon, copy that icon's path into this file. Fonts load Pretendard JP's dynamic subset via a single stylesheet.
- **Don't block render**: the server never awaits while holding a lock; the client uses no synchronous heavy work or waterfall requests.
- **When adding or changing a feature, explain its speed impact** (bundle size, extra requests, latency) in one line.

## i18n

- UI strings must go through `t()` from `web/src/lib/i18n.ts`. Hardcoded strings are not allowed.
- English, Korean and Japanese are required. When you add a key to `en`, any key missing from `ko`/`ja` shows up as a type error.
- Language names (`lib/languages.ts`) are always shown in their own language (endonym).

## Protocol

- Message types are defined only in `server/src/protocol.rs`. `cargo test --manifest-path server/Cargo.toml` regenerates `web/src/protocol/*.ts` (no manual edits).

## Running the app and checking the screen (AI included)

- Start: `scripts/dev.sh` (stop with `scripts/dev.sh stop`, check with `status`). It runs the Rust server in `dev` mode and Vite together, and waits until the API proxy and login are actually working.
- Test host account: user `dev`, password in `server/.dev-host-password` (git-ignored, mode 0600, auto-created on first run and reused after). Never paste the password into chat or logs.
- `tolkmeet-server dev` refuses to start unless bound to loopback (127.0.0.1/::1). Never use it on a deployed server.
- Checking the screen (works without a browser pane):

  ```bash
  pnpm --dir web shot [--login] [--mobile] [--locale ko|ja|en] / /login @meeting
  ```

  It saves PNGs under `shots/`, and you look at them with the Read tool. `@meeting` creates a global meeting and captures the lobby and then the meeting room. Camera and mic use Chromium's fake devices.
- Cloud sessions: the `SessionStart` hook in `.claude/settings.json` runs `scripts/cloud-setup.sh` (web dependencies, Playwright Chromium, server prebuild). The Playwright download domains (`cdn.playwright.dev`, `playwright.download.prss.microsoft.com`) are not in the default network allowlist, so they must be allowed in the environment's network settings. `GEMINI_API_KEY` goes in the cloud environment's environment variables.

## Verification

- Server: `cargo test`, `cargo clippy --all-targets` (inside `server/`)
- Web: `pnpm build`, `pnpm lint` (inside `web/`)
