# Architecture

> 상태: 구현과 향후 설계를 함께 기록한다. 회의·브라우저 음성 인식(ephemeral token)·서버 번역 파이프라인·자막 UI·TURN 시간제한 자격 발급이 구현되어 있다. coturn 운영과 배포는 아직 계획이다. AI 제공자는 Gemini로 통일하며, "결정 필요" 표시는 아직 정하지 않은 항목이다.

## 범위

| 단계 | 기능 | 우선순위 |
|---|---|---|
| 1 | 화상회의(영상·음성) + 실시간 LLM 번역 자막 + 화면공유 | 지금 |
| 2 | 대면 모드: 한 기기로 두 언어 상호 번역 | 1단계 이후 |

2단계는 1단계의 **번역 파이프라인을 그대로 재사용**하도록 설계한다. 즉 번역은 WebRTC와 분리된 독립 모듈이다.

## 언어 모델 (사람 단위)

각 참가자는 두 값을 가진다.

- `lang` (내 언어): 내가 말하고 읽는 언어 → STT 언어 힌트
- `partner_lang` (상대 언어): 내 발화를 번역할 대상 언어

화자 A의 발화는 `A.lang`으로 인식되고 `A.partner_lang`으로 번역된다. 3명 이상이면 다른 참가자들의 `lang` 중 원문과 다른 언어도 추가로 한 번씩만 번역한다. 자막 패널은 모두에게 같은 형태로 **원문 → 번역** 순서로 보여준다(디자인 시스템의 자막 항목). 2단계 대면 모드는 같은 기기의 두 가상 참가자(`ko→ja`, `ja→ko`)로 표현된다.

## 전체 구조

아래 구조에서 TURN은 자격 발급까지 구현됐고, coturn 서버 운영은 배포와 함께 붙인다.

```
 Browser A                                   Rust server (EC2)                 외부 API
┌──────────────────────┐  WSS /ws (JSON)   ┌───────────────────────────┐
│ React UI             │◄─────────────────►│ signaling + room state    │
│                      │  stt 텍스트 이벤트 │ 화자별 문장 버퍼          │
│ CaptionPanel        ◄┼───────────────────│  └► Translator ───────────┼──► Gemini Flash-Lite
│                      │  caption 이벤트    │     (최신 우선·헤징)      │    (streamGenerateContent)
│ getUserMedia         │                   │ ephemeral token 발급 ─────┼──► (auth_tokens)
│ AudioWorklet + VAD ──┼── WSS (Live API, ephemeral token) ────────────┼──► Gemini Transcribe Live
└──────┬───────────────┘                   └───────────────────────────┘
       │ WebRTC (P2P, DTLS-SRTP) 영상·음성·화면공유
       ▼                                   ┌───────────────────────────┐
 Browser B  ◄──── NAT 통과 실패 시 ───────►│ TURN (coturn)             │
                                           └───────────────────────────┘
```

미디어(영상·음성·화면)는 브라우저끼리 WebRTC로 직접 주고받는다. 음성 인식은 **각 브라우저가 자기 마이크만** Gemini Live에 직접 보낸다(서버를 거치는 오디오 경로가 없어 지연과 서버 대역폭이 줄고, 화자 분리가 공짜다). API 키는 서버에만 있고, 브라우저는 서버가 발급한 짧은 수명의 ephemeral token으로만 연결한다. 인식된 텍스트는 기존 `/ws`로 서버에 올라오고, 번역과 팬아웃은 서버가 한다.

## 서버 (Rust)

| 영역 | 선택 | 이유 |
|---|---|---|
| 런타임 | `tokio` | 사실상 표준 |
| HTTP/WS | `axum` (+ `axum::extract::ws`) | tower 미들웨어, 타입 안전 라우팅 |
| 직렬화 | `serde`, `serde_json` | 프로토콜 메시지 |
| 외부 API | `reqwest`(HTTP/SSE), `tokio-tungstenite`(STT WS) | 스트리밍 STT·LLM |
| 상태 | 프로세스 메모리 (`Mutex<HashMap<..>>`, 잠금 안에서 await 없음) | MVP는 단일 인스턴스. 수평 확장 시 Redis pub/sub |
| 관측 | `tracing`, `tracing-subscriber` | 세그먼트별 지연 측정 |
| SFU (나중) | `str0m` | 5명 이상 회의가 필요해질 때. sans-IO라 axum과 함께 쓰기 쉬움 |

### 모듈

지금은 단일 크레이트(`server/src`)이고, 번역이 붙어 커지면 같은 경계로 workspace를 나눈다.

```
server/src/
  main.rs       axum 라우터 (/api/health, POST /api/rooms, /ws)
  ice.rs        참가자별 ICE 서버 목록(STUN + 시간제한 TURN 자격)
  ws.rs         WS 연결 = 참가자. join 검증, 메시지 라우팅
  room.rs       방·참가자 상태, 입력 검증, 시그널 중계 (+ 단위 테스트)
  protocol.rs   메시지 타입. `cargo test` 시 ts-rs가 web/src/protocol/*.ts 생성
  pipeline/     STT 텍스트 → 문장 버퍼 → 번역 → 자막 이벤트 (WebRTC와 무관, 2단계 재사용). 화자당 워커 태스크 1개
  providers/    trait Translator + 헤징, Gemini 스트리밍 번역 구현
```

`pipeline`과 `providers`의 경계는 번역 상태 처리와 Gemini API 통신을 분리하고, 테스트에서 가짜 번역기를 주입하기 위한 것이다. 실제 AI 제공자는 Gemini만 사용한다. 운영자가 루트 `.env` 또는 서버 환경 변수에 `GEMINI_API_KEY`를 설정하며 참가자별 키 입력·보관 기능은 없다.

### 번역 파이프라인 (지연이 제품 품질을 결정한다)

측정 근거: [docs/benchmarks/2026-10-16-gemini.md](benchmarks/2026-10-16-gemini.md).

1~3단계는 브라우저(`web/src/lib/{capture,vad,live,stt,speech}.ts`), 4~6단계는 서버 파이프라인이다.

실제 경로 측정(2026-10-16, `web/tests/browser-stt.mjs`, 가짜 마이크로 TTS 음성 재생, 유료 Standard):

| 단계 | ko → ja | en → ko |
|---|---|---|
| 토큰 요청 → 수신 | 0.13s | 0.15s |
| 세션 연결 → 준비(setupComplete) | 0.59s | 0.60s |
| 말 시작(activityStart) → 첫 중간 결과 | 0.96s | 1.48s |
| 첫 중간 결과 → 첫 번역 자막 | **0.63s** | **0.55s** |
| 말 끝 → `activityEnd`(행오버) | 0.6s | 0.6s |
| `activityEnd` → 확정 전사 | 0.35s | 0.32s |
| 확정 전사 → 확정 번역 자막 | 0.65s | 0.69s |
| **말 끝 → 확정 번역 자막** | **약 1.6s** | **약 1.6s** |

세션은 입장할 때 미리 열어 두므로 토큰·연결 시간은 첫 발화에 더해지지 않는다. 번역 자막은 말하는 도중에 이미 보이고, 말이 끝난 뒤의 1.6초는 확정 교정에 걸리는 시간이다. 가장 큰 몫은 행오버(0.6s)다.

1. **마이크 → AudioWorklet**: 16kHz mono PCM16, 100ms 조각. 클라이언트 VAD가 말하는 구간만 보낸다(아래 "음성 구간 감지").
2. **STT**: 브라우저 → `gemini-3.5-transcribe-live`(수동 활동 모드). 중간 결과가 약 0.5초마다 "지금까지의 전체 문장"으로 오고, `activityEnd` 후 0.3~0.4초에 확정 결과가 온다. 일본어 중간 결과의 단어 사이 공백은 제거한다.
3. **텍스트 업로드**: 브라우저가 `/ws`로 `stt { seg, text, final }`을 보낸다. 바뀐 경우에만 보낸다. `activityEnd` 뒤 2초 안에 확정 결과가 오지 않으면 마지막 중간 결과를 확정으로 보내고, 그 뒤에 늦게 온 결과는 버린다(같은 말이 두 번 확정되지 않게). `seg`는 `/ws` 연결 하나에서 계속 늘어난다(번역을 껐다 켜도).
4. **문장 버퍼**: 서버가 화자별로 "아직 문장이 끝나지 않은 확정 조각 + 현재 중간 결과"를 하나로 이어 둔다. 문장 끝 부호(。.?!)가 오면 그 문장을 확정한다.
5. **번역**: 버퍼의 미완성 문장만 `gemini-3.5-flash-lite`(`thinkingLevel: minimal`, 유료 Standard)로 스트리밍 번역. 문맥은 직전 확정 문장 2~3쌍.
   - **최신 우선**: 말하는 중인 문장은 목표 언어마다 동시에 1개만 번역한다. 그사이 바뀐 텍스트는 진행 중 요청이 끝나는 즉시 최신 것 하나만 번역한다(중간 버전은 건너뜀). 진행 중 요청을 매번 취소하지 않는 이유: 중간 결과는 0.5초마다 오고 번역은 약 1초라 말하는 동안 번역이 하나도 끝나지 않는다. 문장이 확정되면 그때는 진행 중 요청을 취소하고 확정 번역을 바로 시작한다.
   - **문장 확정**: 확정된 STT 텍스트에서 문장 끝 부호가 나오면 확정한다. 확정 조각이 부호 없이 끝나고 2초간 말이 없으면 그대로 확정한다. 400자를 넘어도 확정한다.
   - **헤징**: 1.5초 안에 첫 토큰이 없으면 같은 요청을 한 번 더 보내 먼저 오는 쪽을 쓴다(드문 20초급 지연 제거).
6. **팬아웃**: 목표 언어별 1회 번역 → 해당 독자에게 `caption` 이벤트로 스트리밍.

측정치(유료 Standard): 번역 첫 토큰 p50 1.0s · p90 1.2~1.6s. 말하는 동안 번역 자막은 말보다 **약 1.5~1.7초** 뒤에 따라온다. 비용은 말하는 1분당 약 $0.012(STT $0.009 + 번역 $0.003).

장애 시: 번역 실패하면 원문을 final로 보내고 경고 이벤트. STT 실패하면 화자 자막 일시 중단 표시.

### 음성 구간 감지(VAD)와 침묵 비용

STT 비용(전체의 약 75%)은 **보낸 오디오 길이**에 비례한다. 각자 자기 말할 때만 보내면 1:1 회의 기준 절반 이하가 된다(측정: 보낸 오디오가 경과 시간의 21~49%).

측정으로 정한 규칙:

| 결정 | 이유(측정) |
|---|---|
| **서버 VAD를 끄고 수동 모드**(`realtimeInputConfig.automaticActivityDetection.disabled: true`), 클라이언트 VAD가 `activityStart` / `activityEnd`를 보낸다 | 자동 모드에서 침묵을 안 보내면 서버가 발화 끝을 못 알아채 **확정 결과가 오지 않고** 다음 발화가 같은 턴에 이어 붙는다. 수동 모드는 `activityEnd` 후 **0.31~0.42초**에 확정(자동 모드 1.34초보다 빠름) |
| **keepalive**: 침묵 중 30초마다 100ms 무음을 보낸다 | 입력이 **60초** 없으면 서버가 세션을 끊는다(자동·수동 동일). keepalive 2회로 75초 침묵을 버티고 다음 발화가 정상 인식됨. 추가 비용 = 침묵 시간의 0.3% |
| 세션 하나에서 언어 자동 감지 | 한국어 발화 → 침묵 → 일본어 발화가 같은 세션에서 각각 정확히 인식됨 |
| **세션 교체(rotation)**: 침묵이 3분을 넘으면 새 세션을 백그라운드에서 미리 열어 두고 옛 세션을 닫는다. 말하는 중이면 다음 `activityEnd` 직후 교체. 10분 제한 전에도 같은 방식으로 교체 | keepalive를 보내도 침묵 **약 4.5분**(281초) 뒤 `Deadline expired`로 끊김. 세션 수립에 0.6~0.9초가 걸리므로 말이 시작될 때 새로 여는 대신 미리 열어 두면 지연이 0. 세션을 여는 것 자체는 오디오를 보내지 않아 비용 없음 |

클라이언트 VAD(의존성 없음). AudioWorklet은 기기 샘플레이트를 16kHz로 줄여 20ms 프레임만 넘기고, VAD는 메인 스레드에서 프레임마다 돈다(초당 50번, 프레임당 수 μs). 순수 함수라 단위 테스트가 쉽다. 워클릿 소스는 번들 안 문자열이라 추가 요청이 없다.
- 프레임(20ms) 에너지와 **적응형 잡음 바닥**을 비교. 바닥의 약 3배(10dB)를 2프레임 연속 넘으면 말 시작. 처음 100ms는 바닥만 잰다.
- 바닥은 조용해지면 빨리 내려가고 시끄러워지면 천천히 올라간다(조용할 때 약 2초, 말하는 중 약 10초). 말은 음절 사이마다 에너지가 떨어져 끊기지 않고, 끊김 없는 잡음(선풍기 등)은 몇 초 안에 바닥이 된다.
- **프리롤 300ms**: 항상 직전 300ms를 링버퍼에 두고, 말 시작 시 `activityStart` 직후 먼저 보낸다(첫 음절 잘림 방지).
- **행오버 600ms**: 에너지가 떨어져도 600ms는 더 보낸 뒤 `activityEnd`(문장 중간 쉼에서 끊기지 않게). 500~800ms 사이에서 실제 음성으로 조정.
- 마이크를 끄면 즉시 `activityEnd` 후 송신 중단, 오디오 처리도 멈춘다(`AudioContext.suspend`). 3분 넘게 꺼져 있으면 세션을 닫고, 다시 켜면 새로 연다(그사이 말은 버퍼에 모았다가 보낸다).
- 세션이 끊기면 교체용으로 열어 둔 세션이 있으면 그것을 바로 쓰고, 없으면 다시 연다. 30초 넘게 잘 동작하던 세션이면 바로, 아니면 1·3·10·30초 간격으로 다시 시도하고 그동안 자막 패널에 "일시 중단"을 표시한다. 서버에 키가 없으면 "쓸 수 없음"을 표시하고 다시 요청하지 않는다.
- 언어를 바꾸거나 `goAway`를 받거나 9분이 지나면 위 교체 규칙대로 새 세션으로 바꾼다(14분이 넘으면 말하는 중이어도).
- **스피커 소리 유입**: `echoCancellation`을 켠다(구현됨). 상대가 말하는 동안(원격 오디오 레벨이 높을 때) 임계값을 올리는 것과 헤드폰 권장 안내는 아직 하지 않았다. 잡음 많은 환경에서 오검출이 많으면 Silero VAD(ONNX 약 2MB, 지연 로딩)로 교체.

### 프로토콜

원본은 [server/src/protocol.rs](../server/src/protocol.rs). 구현된 메시지:

```jsonc
// client → server
{ "t": "stt", "seg": 3, "text": "저희 투자팀은", "final": false }   // seg = STT 구간 번호, 중간 결과는 구간 내 누적
{ "t": "join", "room": "abcd-efgh-ijkm", "name": "유승재", "lang": "ko", "partner_lang": "ja",
  "media": { "mic": true, "cam": true, "screen": false } }
{ "t": "lang", "lang": "ko", "partner_lang": "ja" }
{ "t": "media", "media": { "mic": false, "cam": true, "screen": false } }
{ "t": "signal", "to": "<peer id>", "data": { "description": {…} } | { "candidate": {…} } }
{ "t": "stt_token", "id": 7 }                                         // 음성 인식 세션 하나용 토큰 요청

// server → client
{ "t": "welcome", "you": "<peer id>", "host": false, "settings": {…}, "peers": [Peer…], "knocks": [],
  "ice_servers": [{ "urls": ["stun:…"] }, { "urls": ["turn:…"], "username": "1760000000:<peer id>", "credential": "…" }] }
{ "t": "peer_joined" | "peer_updated", "peer": Peer }
{ "t": "peer_left", "id": "<peer id>" }
{ "t": "signal", "from": "<peer id>", "data": {…} }
{ "t": "caption", "seg": "p2-42", "speaker": "p2", "src_lang": "ja", "src": "私たちの…",
  "tgt_lang": "ko", "text": "저희 투자팀은…" | null, "final": false, "rev": 3 }
{ "t": "stt_token", "id": 7, "token": "auth_tokens/…", "model": "models/gemini-3.5-transcribe-live" }
{ "t": "stt_token_denied", "id": 7, "retry": true }
{ "t": "notice", "code": "translation_failed" }
{ "t": "error", "code": "bad_request" | "room_full" | "not_joined" | "unknown_peer", "message": "…" }
```

`caption`은 독자마다 자기 목표 언어(`tgt_lang`)로 받는다. `seg` + `rev`로 같은 줄을 갱신한다(늦게 도착한 낮은 rev는 버린다). 확정 전 `text`는 이전 원문의 번역일 수 있다. `text`가 null이면 번역 중이거나, 같은 언어라 번역이 필요 없거나(`tgt_lang == src_lang`), 확정 번역에 실패한 것이다(`notice` 함께 전송). `stt`는 입장한 참가자의 것만, 번역이 켜진 방에서만 처리하고 연결당 초당 5개(버스트 10)로 제한한다. `stt_token`도 같은 조건에서만 발급하고 연결당 30초에 1개(버스트 4)로 제한한다. 발급은 비동기라 답 순서가 바뀔 수 있어 `id`로 짝을 맞춘다. 조건이 안 되거나 키가 없으면 `retry: false`(다시 요청하지 않음), 일시적 실패·빈도 초과면 `retry: true`(잠시 뒤 다시 요청). 연결이 끊기면 진행 중인 발급도 취소한다.

## 클라이언트

| 항목 | 선택 | 이유 |
|---|---|---|
| 프레임워크 | React + Vite + TypeScript | WebRTC·AudioWorklet·getDisplayMedia가 모두 JS API라 직접 다루기 쉽다 |
| 스타일 | `design/tokens.css` CSS 변수 | 디자인 시스템 그대로 |
| 타입 | `protocol` 크레이트에서 `ts-rs`로 생성 | 서버·클라이언트 메시지 불일치 방지 |

화면공유: `getDisplayMedia()` 트랙을 기존 PeerConnection에 추가하고 재협상. 공유 시작/중지를 시그널링으로 알려 상대 레이아웃을 `presenter`로 전환.

## "전부 Rust로 + EC2에 띄우면 되나?"

**서버는 전부 Rust, 프런트는 TypeScript, 배포는 EC2 — 이 조합을 권장한다.**

- **프런트까지 Rust(Leptos/Dioxus → WASM)는 비추천.** 이 앱의 핵심인 WebRTC, `getUserMedia`, `getDisplayMedia`, AudioWorklet이 전부 브라우저 JS API라 WASM에서는 `web-sys` 바인딩을 거쳐야 하고, 오디오 워클릿은 결국 JS로 써야 한다. 얻는 것보다 마찰이 크다. 대신 프로토콜 타입을 Rust에서 생성해 "진실의 원천"은 Rust에 둔다.
- **TURN 서버는 coturn(C) 사용 권장.** Rust 구현체도 있지만 운영 검증된 건 coturn이다. 직접 운영이 싫으면 Cloudflare Calls TURN 같은 관리형도 가능. TURN 없이는 회사·모바일 망 사용자 일부가 연결되지 않는다.
- **EC2는 적합하다.** WebRTC/TURN은 UDP와 고정 IP가 필요해서 Lambda·App Runner 같은 서버리스보다 EC2가 맞다.

### EC2 MVP 구성 (1대)

| 구성 | 내용 |
|---|---|
| 리전 | `ap-northeast-2` (서울) |
| 인스턴스 | t4g.small 정도로 시작 (ARM, Rust 크로스빌드 `aarch64-unknown-linux-gnu`). 번역은 외부 API라 CPU 부담 낮음 |
| 고정 IP | Elastic IP (TURN은 고정 공인 IP 필요) |
| TLS | Caddy가 443에서 자동 인증서 → Rust 서버(127.0.0.1:8080) 프록시. **getUserMedia는 HTTPS 필수** |
| 정적 파일 | 프런트 빌드 결과를 Caddy가 서빙 (나중에 S3+CloudFront로 분리 가능) |
| TURN | 같은 인스턴스에 coturn, 3478/udp+tcp, 5349/tcp, 릴레이 포트 범위(예: 49152-49999/udp) |
| 보안그룹 | 443/tcp, 3478/udp·tcp, 5349/tcp, 릴레이 UDP 범위만 개방. SSH는 SSM Session Manager로 대체 |
| 비밀정보 | 서버 운영 키는 SSM Parameter Store / Secrets Manager. 로그에 키·음성·자막 원문 남기지 않음 |
| 배포 | GitHub Actions에서 바이너리 빌드 → systemd 서비스 재시작 (컨테이너는 선택) |

coturn 필수 설정(서버의 `TURN_URLS`·`TURN_SECRET`과 짝):

```ini
use-auth-secret
static-auth-secret=<TURN_SECRET과 같은 값>
realm=<도메인>
fingerprint
no-cli
# 릴레이로 인스턴스 내부·VPC·메타데이터(169.254.169.254)에 닿지 못하게 막는다.
no-loopback-peers
no-multicast-peers
denied-peer-ip=10.0.0.0-10.255.255.255
denied-peer-ip=172.16.0.0-172.31.255.255
denied-peer-ip=192.168.0.0-192.168.255.255
denied-peer-ip=169.254.0.0-169.254.255.255
```

주의: ALB는 UDP를 다루지 못하므로 TURN·(나중의) SFU 트래픽은 로드밸런서 없이 인스턴스 직접 또는 NLB로 받는다.

### 확장 경로

1. 2~4인 회의: P2P mesh + TURN (MVP)
2. 5인 이상 또는 녹화 필요: Rust SFU(`str0m`) 도입. 이때도 STT는 각 브라우저 → Gemini 직접 연결을 유지한다(서버가 오디오를 중계하지 않음).
3. 다중 인스턴스: 방을 인스턴스에 고정(sticky), 방 상태·자막 팬아웃은 Redis pub/sub.

## 보안

| 항목 | 방침 |
|---|---|
| 주최자 인증 | 운영자가 `HOST_ACCOUNTS`에 argon2id 해시로 등록. 세션은 256bit 랜덤 토큰을 HttpOnly·SameSite=Lax·Secure 쿠키로(12시간). 계정별 15분 내 10회 실패 시 잠금, 없는 계정도 같은 시간이 걸리게 비교 |
| 회의 생성 | 로그인한 주최자만(`POST /api/rooms`), 주최자당 동시 20개. 아무도 없는 방은 30분 뒤 삭제 |
| 입장 | 방을 만든 계정으로 접속하면 바로 입장, 그 외는 대기열(방당 8명) → 주최자가 수락해야 입장. 수락·거절·번역 설정은 서버가 매 요청 주최자인지 확인. 방당 최대 4명 |
| CSRF·WS 하이재킹 | API는 JSON Content-Type만 받음 + SameSite=Lax. `/ws`는 Origin이 Host와 같아야 업그레이드 |
| 시그널링 | `signal.to`는 같은 방 참가자에게만 중계 |
| TURN 자격 | 정적 비밀번호 대신 서버가 발급하는 시간제한 HMAC 자격(coturn `use-auth-secret`, TURN REST API 방식). `username = "<만료>:<참가자 id>"`, `credential = base64(HMAC-SHA1(TURN_SECRET, username))`, 12시간(coturn은 할당 갱신 때도 자격을 검사하므로 회의보다 길게). `welcome`으로만 나가므로 입장이 확정된 참가자만 받는다. 공개 API로는 ICE 설정을 내주지 않는다(구현됨) |
| Gemini API 키 | 운영자가 루트 `.env` 또는 서버 환경 변수로 설정. Git·로그·에러 메시지·클라이언트 응답에 키를 노출하지 않음 |
| 음성·자막 | 기본은 저장하지 않음. 기록 저장 기능은 참가자 전원 고지 후에만 |
| 입력 제한 | 이름 1~40자·언어 태그·방 코드 형식 검증, WS 메시지 64KB 제한, `stt` 빈도 제한·1000자 제한, STT 토큰 발급 빈도 제한(구현됨). 다른 메시지 빈도 제한, 방당 인원 제한 |
| STT ephemeral token | 서버가 **입장이 확정된 참가자**에게만, 번역이 켜진 방에서만 발급. 1회용(`uses: 1`), 1분 안에 세션을 열어야 하고 세션은 15분까지. `v1alpha/auth_tokens`의 `bidiGenerateContentSetup` + `fieldMask`로 모델·응답 형식(TEXT)·전사 언어·수동 활동 모드를 잠그고, 시스템 지시·도구는 비운 채로 잠근다(실측: 클라이언트가 다른 모델이나 AUDIO를 요청해도 잠긴 값으로 동작하고, 텍스트 질문에는 답하지 않는다). `fieldMask`가 없으면 아무것도 잠기지 않는다(실측). 서버 API 키는 브라우저에 절대 내려가지 않음(구현됨) |

## 결정 필요

AI 구성은 Gemini로 결정했다. 번역 기본 모델은 `gemini-3.5-flash-lite`이며 `GEMINI_TRANSLATE_MODEL`로 변경할 수 있다. 브라우저 STT는 `gemini-3.5-transcribe-live`를 사용하는 설계다. 키는 서버 운영자가 관리한다.

| 항목 | 권장 | 대안 |
|---|---|---|
| 인증 | 주최자: 환경변수 계정(결정됨). 게스트: 링크 + 주최자 수락 | OAuth로 확장 |
| TURN | coturn 자체 운영 | 관리형 TURN |
