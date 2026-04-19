#!/bin/zsh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
GENERATED_DIR="$ROOT_DIR/testdata/generated"
AUDIO_FILE="$GENERATED_DIR/test-tone.mp3"
EVENT_LOG="$GENERATED_DIR/media-events.log"
LAUNCHER="$GENERATED_DIR/run-shuffle-media-test.sh"

mkdir -p "$GENERATED_DIR"

if [[ ! -f "$AUDIO_FILE" ]]; then
  ffmpeg -y -f lavfi -i "sine=frequency=440:duration=6" -c:a libmp3lame -q:a 4 "$AUDIO_FILE" >/dev/null 2>&1
fi

cargo build
rm -f "$EVENT_LOG"

cat >"$LAUNCHER" <<EOF
#!/bin/zsh
cd '$ROOT_DIR'
export SHUFFLE_MEDIA_EVENTS_LOG='$EVENT_LOG'
exec '$ROOT_DIR/target/debug/shuffle' '$AUDIO_FILE'
EOF
chmod +x "$LAUNCHER"

osascript <<EOF
tell application "Terminal"
  activate
  do script quoted form of POSIX path of "$LAUNCHER"
end tell
EOF

for _ in {1..40}; do
  if [[ -f "$EVENT_LOG" ]] && grep -q "media_remote_started" "$EVENT_LOG"; then
    break
  fi
  sleep 0.25
done

if [[ ! -f "$EVENT_LOG" ]] || ! grep -q "media_remote_started" "$EVENT_LOG"; then
  echo "shuffle did not start media integration"
  exit 1
fi

swift "$ROOT_DIR/scripts/media_remote_send.swift" 2 >/dev/null

for _ in {1..40}; do
  if grep -q "remote_action=toggle" "$EVENT_LOG"; then
    echo "media integration passed"
    pkill -f "$ROOT_DIR/target/debug/shuffle" || true
    exit 0
  fi
  sleep 0.25
done

echo "media integration failed"
cat "$EVENT_LOG"
pkill -f "$ROOT_DIR/target/debug/shuffle" || true
exit 1
