#!/usr/bin/env bash
#
# Local POC for pinned-HTTPS pairing: pair the REAL Android app (on the emulator) with the
# REAL idiolect-sync-server over a genuine self-signed-TLS + SPKI-pin handshake — no stubs.
#
# It proves, end to end:
#   1. the server serves TLS by default and prints an https URL + one-time code + cert pin;
#   2. the phone pairs over a PINNED handshake and persists the endpoint + pin + token;
#   3. a WRONG pin is rejected — pairing does not happen (the pin is not cosmetic).
#
# Re-run it freely. Watch the UI live by booting the AVD with a window (drop `-no-window`).
#
# Env overrides: PORT (8765), IDIOLECT_MODEL_PATH (repo tiny.en fixture), ADB.
set -uo pipefail
cd "$(dirname "$0")/.."

ADB="${ADB:-$HOME/Android/Sdk/platform-tools/adb}"
PKG=org.idiolect.android
PORT="${PORT:-8765}"
MODEL="${IDIOLECT_MODEL_PATH:-tests/fixtures/whisper/ggml-tiny.en.bin}"
SERVER_BIN=target/debug/idiolect-sync-server
WORK=$(mktemp -d)
trap 'pkill -f "$SERVER_BIN" 2>/dev/null' EXIT

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }

say "building the server + installing the debug app"
cargo build -q -p idiolect-sync-server --bin idiolect-sync-server || exit 1
./android/gradlew -q -p android :app:installDebug -PandroidAbis=x86_64 || exit 1

start_server() {
  pkill -f "$SERVER_BIN" 2>/dev/null; sleep 1
  IDIOLECT_MODEL_PATH="$MODEL" \
  IDIOLECT_TOKENS_PATH="$WORK/device-tokens.json" \
  IDIOLECT_SYNC_ADDR="127.0.0.1:$PORT" \
  IDIOLECT_PAIR_URL="https://10.0.2.2:$PORT" \
  IDIOLECT_MODEL_ID="tiny.en" \
  "$SERVER_BIN" --pair > "$WORK/announce.txt" 2> "$WORK/server.log" &
  sleep 2
}

fire_deeplink() { # $1 = pin
  local uri="idiolect://pair?u=https%3A%2F%2F10.0.2.2%3A$PORT&c=$CODE&f=$1"
  # Single-quote the URI for the *device* shell so '&' stays inside the data URI.
  $ADB shell "am start -a android.intent.action.VIEW -c android.intent.category.BROWSABLE -d '$uri'" >/dev/null
}

wait_for_pair() { # poll the app's private files for the persisted endpoint
  for _ in $(seq 1 40); do
    local url; url=$($ADB shell run-as "$PKG" cat files/sync.url 2>/dev/null | tr -d '\r')
    [ -n "$url" ] && { echo "$url"; return 0; }
    sleep 1
  done
  return 1
}

# ----------------------------------------------------------------------------------------
say "starting the TLS sync server (--pair)"
start_server
cat "$WORK/announce.txt"
grep . "$WORK/server.log" | head
CODE=$(grep -oE 'code: [0-9A-Z-]+' "$WORK/announce.txt" | sed 's/code: //; s/-//')
PIN=$(grep -oE 'pin:  [0-9a-f]{64}' "$WORK/announce.txt" | sed 's/pin:  //')
echo "parsed → code=$CODE  pin=$PIN"

say "PHASE 1 — correct pin: fire the real pairing deep link"
$ADB shell pm clear "$PKG" >/dev/null
fire_deeplink "$PIN"
if URL=$(wait_for_pair); then
  PINNED=$($ADB shell run-as "$PKG" cat files/sync.pin 2>/dev/null | tr -d '\r')
  echo "✅ PAIRED over pinned TLS"
  echo "   device persisted endpoint : $URL"
  echo "   device persisted pin      : $PINNED"
  echo "   server cert fingerprint   : $PIN"
  [ "$PINNED" = "$PIN" ] && echo "   → pin MATCHES the server cert ✔" || echo "   → PIN MISMATCH ✗"
  echo "   server token store        :"; sed 's/^/     /' "$WORK/device-tokens.json" 2>/dev/null | head -c 600; echo
else
  echo "❌ no pairing persisted in time — server log:"; tail "$WORK/server.log"
fi
sleep 4
$ADB shell screencap -p /sdcard/poc.png >/dev/null 2>&1
$ADB pull /sdcard/poc.png "$WORK/phase1.png" >/dev/null 2>&1

say "PHASE 2 — WRONG pin: same code, a fingerprint the phone never scanned"
start_server                      # fresh code (re-mint), same persisted cert ⇒ same real pin
CODE=$(grep -oE 'code: [0-9A-Z-]+' "$WORK/announce.txt" | sed 's/code: //; s/-//')
$ADB shell pm clear "$PKG" >/dev/null
fire_deeplink "$(printf '0%.0s' {1..64})"   # 64 zeros — not the server's pin
sleep 8
if WRONG=$($ADB shell run-as "$PKG" cat files/sync.url 2>/dev/null | tr -d '\r'); [ -n "$WRONG" ]; then
  echo "❌ UNEXPECTED: a wrong pin still paired ($WRONG)"
else
  echo "✅ REJECTED: a wrong pin never pairs — the handshake aborts, nothing persisted"
fi

say "artifacts in $WORK (phase1.png = app after pairing)"
