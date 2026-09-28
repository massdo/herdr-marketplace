#!/bin/sh
# Captures the real marketplace screens that docs/demo/demo.html animates.
# Same isolation as scripts/e2e.sh: a disposable Herdr profile under /tmp,
# never the daily session. It reads the public catalog and opens a plugin's
# install preview, then cancels it: nothing is installed.
set -eu

unset HERDR_SOCKET_PATH HERDR_CLIENT_SOCKET_PATH HERDR_SESSION HERDR_BIN_PATH \
  HERDR_ENV HERDR_WORKSPACE_ID HERDR_TAB_ID HERDR_PANE_ID HERDR_CONFIG_PATH
unset NO_COLOR
for var in $(env | sed -n 's/^\(HERDR_PLUGIN_[A-Za-z0-9_]*\)=.*/\1/p'); do
  unset "$var"
done

DEMO_DIR=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
PLUGIN_DIR=$(CDPATH= cd -- "$DEMO_DIR/../.." && pwd)
SESSION="herdr-mkt-demo-$$"
TMP=$(mktemp -d /tmp/hmd.XXXXXX)
XDG="$TMP/xdg"
CONFIG="$XDG/herdr/config.toml"
SERVER_LOG="$TMP/server.log"
SERVER_PID_FILE="$TMP/server.pid"
USER_SOCK="${HOME}/.config/herdr/herdr.sock"
USER_CFG="${HOME}/.config/herdr/config.toml"

export XDG_CONFIG_HOME="$XDG"
export XDG_STATE_HOME="$TMP/state"
export HERDR_CONFIG_PATH="$CONFIG"
# Links open through this script, which only writes the address down.
export HERDR_MARKETPLACE_OPEN="$TMP/open.sh"
export HERDR_MARKETPLACE_DEMO_SESSION="$SESSION"
# The prompt the shell pane shows.
export PS1='~/my-app $ '

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

# pyte reads the client's screen; it is installed once in a local venv.
PYTHON=python3
if ! python3 -c "import pyte" >/dev/null 2>&1; then
  VENV="$DEMO_DIR/.cache/venv"
  if [ ! -x "$VENV/bin/python" ]; then
    python3 -m venv "$VENV"
    "$VENV/bin/pip" install --quiet pyte
  fi
  PYTHON="$VENV/bin/python"
fi

sh "$PLUGIN_DIR/scripts/build.sh"

mkdir -p "$XDG/herdr" "$TMP/work"
printf '#!/bin/sh\nprintf "%%s\\n" "$1" >> "$(dirname "$0")/opened.log"\n' > "$HERDR_MARKETPLACE_OPEN"
chmod +x "$HERDR_MARKETPLACE_OPEN"
cat > "$CONFIG" <<'EOF'
onboarding = false

[terminal]
default_shell = "/bin/sh"
shell_mode = "non_login"
EOF

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

herdr --session "$SESSION" plugin link "$PLUGIN_DIR" --enabled >/dev/null
herdr --session "$SESSION" workspace create --cwd "$TMP/work" --label my-app --no-focus >/dev/null

"$PYTHON" "$DEMO_DIR/capture.py" "$DEMO_DIR/screens.json"
