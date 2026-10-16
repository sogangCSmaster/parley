#!/bin/sh
# macOS 내장 TTS로 측정용 발화를 만든다(16kHz mono 16-bit PCM WAV). 생성물은 git에 넣지 않는다.
set -eu
cd "$(dirname "$0")"
mkdir -p fixtures
make() { # <name> <voice> <text>
  say -v "$2" -o "fixtures/$1.aiff" "$3"
  afconvert -f WAVE -d LEI16@16000 -c 1 "fixtures/$1.aiff" "fixtures/$1.wav"
  rm "fixtures/$1.aiff"
}
make ko Yuna "저희 투자팀은 네 명의 멤버로 구성되어 있고요, 주로 인공지능이랑 최근에는 딥테크 분야에 집중적으로 투자하고 있습니다. 혹시 그쪽 팀은 어떤 분야를 보고 계신가요?"
make ja Kyoko "私たちの投資チームは四人のメンバーで構成されていて、主にAI、それから最近だとディープテックといったところへの投資に注力しています。来年は海外のスタートアップにも投資する予定です。"
echo "fixtures/ko.wav fixtures/ja.wav"
