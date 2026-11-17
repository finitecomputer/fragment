#!/bin/bash
# The box's local transcription model on its Mac (fragment docs/self-host.md,
# "Local models on the box's Mac"): whisper.cpp's server with Metal, on the
# Mac's loopback alone, run by launchd. The box's VM reaches it as
# http://host.lima.internal:8178 (Lima's user-mode network answers that
# address from the Mac's own 127.0.0.1); nothing off the Mac can.
#
# ~/local-models holds (build-mac.sh; each checked against SHA256SUMS at every start):
#   bin/whisper-server  whisper.cpp v1.9.5, built here with Metal
#   bin/ffmpeg          FFmpeg 9.0.2, audio only
#   models/ggml-large-v3-turbo-q5_0.bin, models/ggml-silero-v6.2.0.bin
# usage: install-mac.sh [install|uninstall]
set -euo pipefail
D="$HOME/local-models"
LABEL=chat.finite.local-whisper
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
PORT=8178
domain="gui/$(id -u)"

if [ "${1:-install}" = uninstall ]; then
  launchctl bootout "$domain/$LABEL" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed $LABEL (files in $D kept)"
  exit 0
fi

cd "$D"
shasum -a 256 -c SHA256SUMS
# the audio passes through tmp: its owner's alone
mkdir -p tmp logs empty
chmod 700 tmp logs

cat > whisper.sh <<'EOF'
#!/bin/bash
# Run by launchd (chat.finite.local-whisper): checks every file against its
# pin, then the server, in its sandbox, on loopback alone.
set -euo pipefail
D="$HOME/local-models"
cd "$D"
shasum -a 256 -c SHA256SUMS > /dev/null
rm -f tmp/whisper-server-*
# its --convert runs `ffmpeg` from PATH: this one alone
export PATH="$D/bin:/usr/bin:/bin"
exec /usr/bin/sandbox-exec -f "$D/whisper.sb" -D DIR="$D" -D HOME_DIR="$HOME" \
  "$D/bin/whisper-server" --host 127.0.0.1 --port 8178 \
  -m "$D/models/ggml-large-v3-turbo-q5_0.bin" -t 4 -l auto \
  --convert --tmp-dir "$D/tmp" --vad-model "$D/models/ggml-silero-v6.2.0.bin" -nlp \
  --public "$D/empty"
EOF
chmod 0755 whisper.sh

# The server and the ffmpeg it runs: no connection out, inbound on its
# loopback port alone, writes in its tmp alone, and nothing of the rest of
# the home (the box's VM, keys, chats) read.
cat > whisper.sb <<'EOF'
(version 1)
(allow default)
(deny network-outbound (remote ip))
(deny network-inbound (local ip))
(allow network-inbound (local ip "localhost:8178"))
(deny file-write*)
(allow file-write*
  (subpath (string-append (param "DIR") "/tmp"))
  (literal "/dev/null")
  (subpath "/private/var/folders"))
(deny file-read* (subpath (param "HOME_DIR")))
(allow file-read* (subpath (param "DIR")))
(allow file-read-metadata (literal (param "HOME_DIR")))
EOF

mkdir -p "$(dirname "$PLIST")"
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array><string>/bin/bash</string><string>$D/whisper.sh</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Standard</string>
  <key>StandardOutPath</key><string>$D/logs/whisper.out.log</string>
  <key>StandardErrorPath</key><string>$D/logs/whisper.err.log</string>
</dict>
</plist>
EOF

launchctl bootout "$domain/$LABEL" 2>/dev/null || true
# a job booted out is gone only once its process has exited
for _ in $(seq 1 50); do
  launchctl print "$domain/$LABEL" > /dev/null 2>&1 || break
  sleep 0.2
done
launchctl bootstrap "$domain" "$PLIST"
for _ in $(seq 1 120); do
  curl -sf "http://127.0.0.1:$PORT/health" > /dev/null && break
  sleep 0.5
done
curl -s "http://127.0.0.1:$PORT/health"; echo
launchctl print "$domain/$LABEL" | grep -E "state =|pid =" | head -2
lsof -nP -iTCP:$PORT -sTCP:LISTEN
