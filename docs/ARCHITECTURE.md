# Architecture

> 상태: 초안. 구현 전 결정을 위한 설계 문서다. "결정 필요" 표시는 아직 정하지 않은 항목이다.

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

```
 Browser A                              Rust server (EC2)                      외부 API
┌───────────────┐  WSS /ws (JSON)     ┌──────────────────────────┐
│ React UI      │◄───────────────────►│ signaling + room state   │
│               │                     │                          │
│ getUserMedia  │  WSS /audio (PCM16) │ audio ingest per speaker │      ┌──────────────┐
│ AudioWorklet ─┼────────────────────►│   └► STT stream ─────────┼─────►│ STT (스트리밍) │
│               │                     │        partial/final     │      └──────────────┘
│ CaptionStream◄┼─────────────────────│   ◄─ caption fan-out ◄── │      ┌──────────────┐
└──────┬────────┘                     │        Translator ───────┼─────►│ LLM / DeepL  │
       │ WebRTC (P2P, DTLS-SRTP)      └──────────────────────────┘      └──────────────┘
       │ 영상·음성·화면공유
       ▼                              ┌──────────────────────────┐
 Browser B  ◄── NAT 통과 실패 시 ────►│ TURN (coturn)            │
                                      └──────────────────────────┘
```

미디어(영상·음성·화면)는 브라우저끼리 WebRTC로 직접 주고받고, 서버는 **시그널링 + 번역**만 맡는다. 번역용 음성은 각 화자가 자기 마이크만 별도로 서버에 올린다(상대 음성을 다시 올리지 않으므로 화자 분리가 공짜다).

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
  main.rs       axum 라우터 (/api/health, POST /api/rooms, /api/config, /ws)
  ws.rs         WS 연결 = 참가자. join 검증, 메시지 라우팅
  room.rs       방·참가자 상태, 입력 검증, 시그널 중계 (+ 단위 테스트)
  protocol.rs   메시지 타입. `cargo test` 시 ts-rs가 web/src/protocol/*.ts 생성
  (예정) pipeline/   오디오 → STT → 번역 → 자막 이벤트 (WebRTC와 무관, 2단계 재사용)
  (예정) providers/  trait SpeechToText, trait Translator + 구현체(OpenAI, Gemini, Claude, DeepL …)
```

`pipeline`과 `providers`를 trait로 분리해 README가 말하는 여러 제공자와 BYOK를 구현체 교체로 지원한다.

### 번역 파이프라인 (지연이 제품 품질을 결정한다)

1. **오디오 업링크**: AudioWorklet에서 16kHz mono PCM16으로 다운샘플, 40ms 프레임을 WS 바이너리로 전송 (~256kbps). 음성 활동이 없으면 보내지 않는다(VAD는 클라이언트에서 간단히, 서버 STT도 자체 VAD).
2. **STT 스트리밍**: 제공자에 실시간 연결, `lang`을 언어 힌트로. partial은 원문 자막(`listening`)으로 즉시 브로드캐스트.
3. **번역 트리거**: STT가 final을 내면 번역. 긴 발화는 partial이 1.2초 이상 안정되면 그 접두부를 미리 번역(`translating`)하고 final에서 교체.
4. **문맥**: 최근 N개 확정 세그먼트 + 회의 제목/용어집을 프롬프트에 넣는다. 고정 부분은 prompt caching 대상.
5. **스트리밍 출력**: LLM 토큰을 자막 이벤트로 흘려보내고 끝나면 `final`.
6. **팬아웃**: 목표 언어별 1회 번역 → 해당 언어 독자들에게만 전송.

목표 지연(말 끝 → 번역 자막 첫 글자): **1.5초 이내**. 각 단계 시간을 `tracing` span으로 기록한다.

장애 시: 번역 실패하면 원문을 final로 보내고 경고 이벤트. STT 실패하면 화자 자막 일시 중단 표시.

### 프로토콜

원본은 [server/src/protocol.rs](../server/src/protocol.rs). 구현된 메시지:

```jsonc
// client → server
{ "t": "join", "room": "abcd-efgh-ijkm", "name": "유승재", "lang": "ko", "partner_lang": "ja",
  "media": { "mic": true, "cam": true, "screen": false } }
{ "t": "lang", "lang": "ko", "partner_lang": "ja" }
{ "t": "media", "media": { "mic": false, "cam": true, "screen": false } }
{ "t": "signal", "to": "<peer id>", "data": { "description": {…} } | { "candidate": {…} } }

// server → client
{ "t": "welcome", "you": "<peer id>", "peers": [Peer…] }
{ "t": "peer_joined" | "peer_updated", "peer": Peer }
{ "t": "peer_left", "id": "<peer id>" }
{ "t": "signal", "from": "<peer id>", "data": {…} }
{ "t": "error", "code": "bad_request" | "room_full" | "not_joined" | "unknown_peer", "message": "…" }
```

번역 단계에서 추가할 메시지(초안):

```jsonc
{ "t": "caption", "seg": "p2-0042", "speaker": "p2", "src_lang": "ja", "src": "私たちの…",
  "tgt_lang": "ko", "text": "저희 투자팀은…", "final": false, "rev": 3 }
{ "t": "notice", "level": "warning", "code": "translator_slow" }
```

`seg` + `rev`로 같은 줄을 갱신한다(늦게 도착한 낮은 rev는 버린다).

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

주의: ALB는 UDP를 다루지 못하므로 TURN·(나중의) SFU 트래픽은 로드밸런서 없이 인스턴스 직접 또는 NLB로 받는다.

### 확장 경로

1. 2~4인 회의: P2P mesh + TURN (MVP)
2. 5인 이상 또는 녹화 필요: Rust SFU(`str0m`) 도입. 이때 서버가 Opus 트랙을 직접 받으므로 번역용 별도 오디오 업링크를 없앨 수 있다.
3. 다중 인스턴스: 방을 인스턴스에 고정(sticky), 방 상태·자막 팬아웃은 Redis pub/sub.

## 보안

| 항목 | 방침 |
|---|---|
| 주최자 인증 | 운영자가 `HOST_ACCOUNTS`에 argon2id 해시로 등록. 세션은 256bit 랜덤 토큰을 HttpOnly·SameSite=Lax·Secure 쿠키로(12시간). 계정별 15분 내 10회 실패 시 잠금, 없는 계정도 같은 시간이 걸리게 비교 |
| 회의 생성 | 로그인한 주최자만(`POST /api/rooms`), 주최자당 동시 20개. 아무도 없는 방은 30분 뒤 삭제 |
| 입장 | 방을 만든 계정으로 접속하면 바로 입장, 그 외는 대기열(방당 8명) → 주최자가 수락해야 입장. 수락·거절·번역 설정은 서버가 매 요청 주최자인지 확인. 방당 최대 4명 |
| CSRF·WS 하이재킹 | API는 JSON Content-Type만 받음 + SameSite=Lax. `/ws`는 Origin이 Host와 같아야 업그레이드 |
| 시그널링 | `signal.to`는 같은 방 참가자에게만 중계 |
| TURN 자격 | 정적 비밀번호 대신 서버가 발급하는 시간제한 HMAC 자격(TURN REST API 방식) |
| BYOK 키 | 아래 "결정 필요" 참고. 어떤 방식이든 로그·에러 메시지·클라이언트 응답에 키를 노출하지 않음 |
| 음성·자막 | 기본은 저장하지 않음. 기록 저장 기능은 참가자 전원 고지 후에만 |
| 입력 제한 | 이름 1~40자·언어 태그·방 코드 형식 검증, WS 메시지 64KB 제한(구현됨). 메시지 빈도 제한, 방당 인원 제한, 오디오 업링크 레이트 제한(비용 폭주 방지) |

## 결정 필요

| 항목 | 권장 | 대안 |
|---|---|---|
| STT 제공자 | 스트리밍 품질·한국어 기준으로 2~3개 실측 후 결정 (OpenAI Realtime transcription, Deepgram, Google Chirp 등) | 하나로 고정 후 trait로 교체 |
| 번역 기본 모델 | 빠른 소형 모델(지연 우선) + 용어집 문맥 | 고품질 대형 모델 / DeepL(문맥 약함) |
| BYOK 키 보관 | 호스트가 방 생성 시 입력 → 서버 메모리에만, 방 종료 시 폐기 | 서버 운영 키로 제공(과금 필요) / 계정 저장(암호화 at rest, 범위 커짐) |
| 인증 | 주최자: 환경변수 계정(결정됨). 게스트: 링크 + 주최자 수락 | OAuth로 확장 |
| TURN | coturn 자체 운영 | 관리형 TURN |
