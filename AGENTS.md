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

## Verification

- Server: `cargo test`, `cargo clippy --all-targets` (inside `server/`)
- Web: `pnpm build`, `pnpm lint` (inside `web/`)
