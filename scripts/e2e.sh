#!/bin/sh
# herdr-marketplace journey in a disposable Herdr profile. It never reaches
# the daily session: the Herdr variables a pane exports are removed first,
# and every path lives under a short /tmp directory (a Unix socket path is
# limited to about 104 bytes on macOS).
set -eu

unset HERDR_SOCKET_PATH HERDR_CLIENT_SOCKET_PATH HERDR_SESSION HERDR_BIN_PATH \
  HERDR_ENV HERDR_WORKSPACE_ID HERDR_TAB_ID HERDR_PANE_ID HERDR_CONFIG_PATH
for var in $(env | sed -n 's/^\(HERDR_PLUGIN_[A-Za-z0-9_]*\)=.*/\1/p'); do
  unset "$var"
done

PLUGIN_DIR=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
SESSION="herdr-mkt-e2e-$$"
TMP=$(mktemp -d /tmp/hme.XXXXXX)
XDG="$TMP/xdg"
CONFIG="$XDG/herdr/config.toml"
SERVER_LOG="$TMP/server.log"
SERVER_PID_FILE="$TMP/server.pid"
USER_SOCK="${HOME}/.config/herdr/herdr.sock"
USER_CFG="${HOME}/.config/herdr/config.toml"

export XDG_CONFIG_HOME="$XDG"
export XDG_STATE_HOME="$TMP/state"
export HERDR_CONFIG_PATH="$CONFIG"
# Test catalogues are served from this file; the journey rewrites it.
export HERDR_MARKETPLACE_INDEX_URL="file://$TMP/index.json"
# The fixture's build appends a line here; Herdr passes it to builds.
export HERDR_MARKETPLACE_FIXTURE_LOG="$TMP/fixture.log"
export HERDR_MARKETPLACE_E2E_SESSION="$SESSION"
export HERDR_MARKETPLACE_E2E_TMP="$TMP"
export HERDR_MARKETPLACE_E2E_SERVER_LOG="$SERVER_LOG"

cleanup() {
  status=$?
  herdr session stop "$SESSION" >/dev/null 2>&1 || true
  if [ -f "$SERVER_PID_FILE" ]; then
    kill "$(cat "$SERVER_PID_FILE")" >/dev/null 2>&1 || true
  fi
  rm -rf "$TMP"
  exit "$status"
}
trap cleanup EXIT INT HUP TERM

echo "== versions =="
herdr --version
rustc --version
command -v python3 >/dev/null
if [ "$(herdr --version | awk '{print $2}')" != "0.9.1" ]; then
  echo "Herdr 0.9.1 is required" >&2
  exit 1
fi

echo "== build plugin =="
sh "$PLUGIN_DIR/scripts/build.sh"

mkdir -p "$XDG/herdr" "$TMP/work"
cat > "$CONFIG" <<'EOF'
onboarding = false

[terminal]
default_shell = "/bin/sh"
shell_mode = "non_login"

[server]
headless_cols = 160
headless_rows = 48
EOF

echo "== start isolated server =="
herdr --session "$SESSION" server >"$SERVER_LOG" 2>&1 &
echo $! >"$SERVER_PID_FILE"
SOCKET=""
i=0
while [ "$i" -lt 50 ]; do
  CAND="$XDG/herdr/sessions/$SESSION/herdr.sock"
  if [ -S "$CAND" ]; then
    SOCKET=$CAND
    break
  fi
  i=$((i + 1))
  sleep 0.1
done
if [ -z "$SOCKET" ]; then
  echo "isolated socket did not appear" >&2
  cat "$SERVER_LOG" >&2
  exit 1
fi
if [ "$SOCKET" = "$USER_SOCK" ] || [ "$CONFIG" = "$USER_CFG" ]; then
  echo "isolation check failed: socket or config is the daily one" >&2
  exit 1
fi
export HERDR_SOCKET_PATH="$SOCKET"
echo "e2e_socket=$SOCKET"
echo "e2e_config=$CONFIG"

echo "== link plugin =="
herdr --session "$SESSION" plugin link "$PLUGIN_DIR" --enabled
herdr --session "$SESSION" workspace create --cwd "$TMP/work" --label e2e --no-focus >/dev/null

echo "== journey =="
python3 "$PLUGIN_DIR/scripts/e2e_journey.py"
echo "e2e_ok session=$SESSION"
