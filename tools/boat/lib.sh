# tools/boat/lib.sh: what tools/boat/run and tools/boat/golden share.
# Sourced, never run. Shell rather than xtask: these run before anything is
# built on this computer, which is their point.

B=${BOAT_BIN:-$(command -v boat || echo "$HOME/.ascii/bin/boat")}
[ -x "$B" ] || {
  echo "tools/boat: no boat CLI (curl -fsSL https://boat.dev/install | sh, then boat login), or set BOAT_BIN" >&2
  exit 1
}
SNAPSHOT=${FRAGMENT_BOAT_SNAPSHOT:-fragment-warm}
TYPE=${FRAGMENT_BOAT_TYPE:-large}

# The first sandbox id in a `boat ... --json` reply. It reads the whole
# reply: stopping early would end boat with SIGPIPE.
first_id() {
  python3 -I -c '
import json, sys
ids = []
for line in sys.stdin:
    try:
        reply = json.loads(line)
    except ValueError:
        continue
    found = reply.get("id") or (reply.get("sandbox") or {}).get("id")
    if isinstance(found, str) and found.startswith("bx_"):
        ids.append(found)
if not ids:
    sys.exit("tools/boat: no sandbox id in boat'"'"'s reply")
print(ids[0])
'
}

# A sandbox's state (running, stopped, ...), or "gone" once it is deleted
# or its data has expired.
state_of() {
  "$B" info "$1" --json 2>/dev/null | python3 -I -c '
import json, sys
try:
    box = json.load(sys.stdin)["sandbox"]
except (ValueError, KeyError, TypeError):
    print("gone")
else:
    print("gone" if box.get("dataExpiredAt") else box.get("state", "gone"))
' || echo gone
}

# bx ID COMMAND [TIMEOUT]: run COMMAND in a login shell on the sandbox and
# print its output as it comes (boat exec answers in JSON frames when its
# output is not a terminal). Exits with the command's code.
bx() {
  "$B" exec "$1" --timeout "${3:-120}" --stream -- bash -lc "$2" | python3 -I -u -c '
import json, sys
for line in sys.stdin:
    try:
        frame = json.loads(line)
    except ValueError:
        sys.stderr.write(line)
        continue
    kind = frame.get("type")
    if kind == "stdout":
        sys.stdout.write(frame["data"])
        sys.stdout.flush()
    elif kind == "stderr":
        sys.stderr.write(frame["data"])
        sys.stderr.flush()
    elif "error" in frame:
        sys.stderr.write("boat: %s\n" % frame["error"])
'
}
