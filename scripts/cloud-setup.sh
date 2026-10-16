#!/usr/bin/env bash
# Claude Code 클라우드 세션이 시작될 때 실행된다(.claude/settings.json 의 SessionStart 훅).
# 로컬에서는 아무것도 하지 않는다. 표준출력은 Claude의 문맥에 들어가므로 짧게 쓴다.
set -uo pipefail
[ "${CLAUDE_CODE_REMOTE:-}" = "true" ] || exit 0
cd "${CLAUDE_PROJECT_DIR:-$(dirname "$0")/..}"
mkdir -p .dev
LOG=.dev/cloud-setup.log
notes=()

# 웹 의존성: lockfile이 바뀌었을 때만 설치(재개할 때마다 다시 깔지 않게).
lock_hash=$(sha256sum web/pnpm-lock.yaml | cut -c1-16)
if [ "$(cat .dev/pnpm.hash 2>/dev/null)" != "$lock_hash" ] || [ ! -d web/node_modules ]; then
  if pnpm --dir web install --frozen-lockfile >>"$LOG" 2>&1; then
    echo "$lock_hash" >.dev/pnpm.hash
  else
    notes+=("pnpm install failed (see $LOG)")
  fi
fi

# 화면 캡처용 Chromium. 기본 네트워크(Trusted) 허용 목록에 Playwright CDN이 없어 실패할 수 있다.
if ! ls "${PLAYWRIGHT_BROWSERS_PATH:-$HOME/.cache/ms-playwright}"/chromium_headless_shell-* >/dev/null 2>&1; then
  if ! pnpm --dir web exec playwright install chromium >>"$LOG" 2>&1; then
    notes+=("Playwright Chromium download failed: allow cdn.playwright.dev and playwright.download.prss.microsoft.com in the environment's network access (Custom), then start a new session")
  fi
fi

# 서버 첫 빌드는 오래 걸리므로 백그라운드로 미리 해 둔다(scripts/dev.sh 가 결과를 재사용).
nohup cargo build -q --manifest-path server/Cargo.toml >>"$LOG" 2>&1 &

cat <<'EOF'
tolkmeet cloud dev environment is ready:
- Start the app: scripts/dev.sh (Rust server in dev mode + Vite at http://localhost:5173; stop with scripts/dev.sh stop)
- Test host login: user "dev", password in server/.dev-host-password (created automatically on first start)
- See screens: pnpm --dir web shot [--login] [--mobile] [--locale ko|ja|en] / /login @meeting  → PNG paths are printed; open them with the Read tool
EOF
for n in ${notes[@]+"${notes[@]}"}; do echo "- WARNING: $n"; done
exit 0
