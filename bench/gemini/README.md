# gemini-bench

A Rust tool that measures the latency and cost of the real-time caption pipeline (Gemini Transcribe Live → Flash-Lite streaming translation) before we build it.

## Setup

```bash
./make-fixtures.sh
```

Uses macOS's built-in TTS (Yuna, Kyoko) to create `fixtures/ko.wav` and `fixtures/ja.wav` (16kHz mono). You can also use real recordings in the same format.

Put the API key in `GEMINI_API_KEY=...` in `.env` (excluded from git), or pass it as an environment variable.

## Running

```bash
cargo run --release -- translate
```

Calls the translation with Korean → Japanese and Japanese → Korean sentences, "growing as you speak", and reports TTFT, total time, tokens, and cost. Options: `--thinking minimal|low|none`, `--model`, `--runs N`.

```bash
cargo run --release -- transcribe fixtures/ko.wav --lang ko
```

Sends audio at real-time speed (100ms chunks) and reports interim-result cadence, whether results are cumulative or incremental, and latency from end of speech to the final result.
