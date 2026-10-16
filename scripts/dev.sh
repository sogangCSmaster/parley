#!/usr/bin/env bash
# 개발 서버(Rust dev 모드 + Vite)를 백그라운드로 띄우고 준비될 때까지 기다린다.
#   scripts/dev.sh         시작 (이미 떠 있으면 상태만 보여 줌)
#   scripts/dev.sh stop    종료
#   scripts/dev.sh status  상태
# 로그: .dev/server.log, .dev/web.log  · 테스트 계정: server/.dev-host-password
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)
STATE="$ROOT/.dev"
WEB=http://localhost:5173
mkdir -p "$STATE"

running() { [ -f "$STATE/$1.pid" ] && kill -0 "$(cat "$STATE/$1.pid")" 2>/dev/null; }

stop() {
  for name in web server; do
    if running "$name"; then
      # 자식 프로세스(cargo → tolkmeet-server, pnpm → vite)까지 함께 끈다.
      pkill -TERM -P "$(cat "$STATE/$name.pid")" 2>/dev/null || true
      kill "$(cat "$STATE/$name.pid")" 2>/dev/null || true
      echo "stopped $name"
    fi
    rm -f "$STATE/$name.pid"
  done
}

# API 프록시까지 실제로 동작하는지 본다(/api/me가 JSON이어야 함. HTML이면 Vite 프록시가 죽은 것).
healthy() { curl -sf "$WEB/api/me" 2>/dev/null | grep -q '"username"'; }

status() {
  for name in server web; do
    if running "$name"; then echo "$name: running (pid $(cat "$STATE/$name.pid"))"; else echo "$name: stopped"; fi
  done
  if healthy; then echo "app: ready at $WEB"; else echo "app: not ready"; fi
}

case "${1:-start}" in
  stop) stop; exit 0 ;;
  status) status; exit 0 ;;
  start) ;;
  *) echo "usage: scripts/dev.sh [start|stop|status]" >&2; exit 2 ;;
esac

if healthy; then
  status
  exit 0
fi
stop >/dev/null

[ -d web/node_modules ] || pnpm --dir web install --frozen-lockfile

# BIND_ADDR를 명시해 dev 모드의 loopback 제한과 맞춘다.
BIND_ADDR=127.0.0.1:8080 nohup cargo run -q --manifest-path server/Cargo.toml -- dev >"$STATE/server.log" 2>&1 &
echo $! >"$STATE/server.pid"
nohup pnpm --dir web dev --port 5173 --strictPort >"$STATE/web.log" 2>&1 &
echo $! >"$STATE/web.pid"

printf 'starting'
for _ in $(seq 1 300); do
  if healthy; then
    echo
    status
    echo "login: user 'dev', password in server/.dev-host-password (created on first run)"
    exit 0
  fi
  if ! running server || ! running web; then break; fi
  printf '.'
  sleep 1
done
echo
echo "failed to start. last log lines:" >&2
tail -n 20 "$STATE/server.log" "$STATE/web.log" >&2
stop >/dev/null
exit 1
