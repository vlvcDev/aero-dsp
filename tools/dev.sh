#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_DIR=$(cd -- "$SCRIPT_DIR/.." && pwd)
RUNTIME_DIR=${PEDAL_DEV_RUNTIME_DIR:-"$REPO_DIR/.pedal-dev"}
CONTROL_SOCKET=${PEDAL_CONTROL_SOCKET:-"$RUNTIME_DIR/control.sock"}
CATALOG_SOCKET=${PEDAL_CATALOG_SOCKET:-"$RUNTIME_DIR/catalog.sock"}
STATE_FILE=${PEDAL_STATE_FILE:-"$RUNTIME_DIR/engine-state.json"}
PRESET_FILE=${PEDAL_PRESET_FILE:-"$RUNTIME_DIR/presets.json"}
TOKEN_FILE=${PEDAL_TONE3000_TOKEN_FILE:-"$RUNTIME_DIR/tone3000-tokens.json"}
PREVIEW_FILE=${PEDAL_PREVIEW_FILE:-"$RUNTIME_DIR/preview.wav"}
RIFF_DIRECTORY=${PEDAL_RIFF_DIRECTORY:-"$RUNTIME_DIR/riffs"}
BACKING_TRACK_DIRECTORY=${PEDAL_BACKING_TRACK_DIRECTORY:-"$RIFF_DIRECTORY/backing-tracks"}
TEST_AUDIO_FILE=${PEDAL_TEST_AUDIO_FILE:-"$REPO_DIR/data/test-audio/guitarjam-184.wav"}
MODELS_DIR=${PEDAL_MODELS_DIR:-"$REPO_DIR/data/models"}
CATALOG_DB=${PEDAL_CATALOG_DB:-"$RUNTIME_DIR/catalog.db"}
SECRETS_FILE=${PEDAL_SECRETS_FILE:-"$REPO_DIR/deploy/pedal-secrets.env"}
ENGINE_PID_FILE="$RUNTIME_DIR/engine.pid"
CATALOG_PID_FILE="$RUNTIME_DIR/catalog.pid"
ENGINE_LOG="$RUNTIME_DIR/engine.log"
CATALOG_LOG="$RUNTIME_DIR/catalog.log"
ENGINE_STARTED_BY_SCRIPT=0

ENGINE_PROFILE=${PEDAL_ENGINE_PROFILE:-}
if [[ -z "$ENGINE_PROFILE" ]]; then
  if [[ ${PEDAL_AUDIO_ENABLED:-0} == 1 ]]; then
    ENGINE_PROFILE=release
  else
    ENGINE_PROFILE=debug
  fi
fi
case "$ENGINE_PROFILE" in
debug) ENGINE_BINARY="$REPO_DIR/target/debug/pedal-engine" ;;
release) ENGINE_BINARY="$REPO_DIR/target/release/pedal-engine" ;;
*)
  printf 'Error: PEDAL_ENGINE_PROFILE must be debug or release\n' >&2
  exit 1
  ;;
esac

if [[ -f "$SCRIPT_DIR/dev-env.sh" ]]; then
  # shellcheck disable=SC1091
  source "$SCRIPT_DIR/dev-env.sh"
fi

# Desktop Linux normally gives PipeWire ownership of USB interfaces such as a
# Scarlett. Route both ends through its ALSA bridge by default so the pedal can
# coexist with the desktop, browser, and output selector. A deployer can still
# request direct ALSA explicitly with PEDAL_AUDIO_*_DEVICE=hw:CARD=... .
if [[ ${PEDAL_AUDIO_ENABLED:-0} == 1 ]]; then
  : "${PEDAL_AUDIO_CAPTURE_DEVICE:=pipewire}"
  : "${PEDAL_AUDIO_PLAYBACK_DEVICE:=pipewire}"
  export PEDAL_AUDIO_CAPTURE_DEVICE PEDAL_AUDIO_PLAYBACK_DEVICE
fi

mkdir -p "$RUNTIME_DIR" "$MODELS_DIR"

say() { printf '\033[1;36m%s\033[0m\n' "$*"; }
ok() { printf '\033[1;32m✓\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
fail() {
  printf '\033[1;31mError:\033[0m %s\n' "$*" >&2
  exit 1
}

pid_from_file() {
  local file=$1 pid
  [[ -f "$file" ]] || return 1
  read -r pid <"$file"
  [[ "$pid" =~ ^[0-9]+$ ]] || return 1
  kill -0 "$pid" 2>/dev/null || return 1
  printf '%s' "$pid"
}

service_status() {
  local name=$1 pid_file=$2 socket=$3 pid
  if pid=$(pid_from_file "$pid_file"); then
    if [[ -S "$socket" ]]; then
      ok "$name is running (PID $pid)"
    else
      warn "$name process exists (PID $pid), but its socket is not ready"
    fi
  else
    printf '  %s is stopped\n' "$name"
  fi
}

wait_for_socket() {
  local name=$1 socket=$2 pid_file=$3 log=$4 pid attempt
  for attempt in {1..100}; do
    [[ -S "$socket" ]] && return 0
    if ! pid=$(pid_from_file "$pid_file"); then
      warn "$name exited during startup. Recent log output:"
      tail -n 30 "$log" 2>/dev/null || true
      return 1
    fi
    sleep 0.05
  done
  warn "$name did not become ready. Recent log output:"
  tail -n 30 "$log" 2>/dev/null || true
  return 1
}

start_engine() {
  local pid
  local -a cargo_profile_args=()
  if pid=$(pid_from_file "$ENGINE_PID_FILE"); then
    ok "Engine is already running (PID $pid)"
    return
  fi
  rm -f "$CONTROL_SOCKET" "$ENGINE_PID_FILE"
  if [[ "$ENGINE_PROFILE" == release ]]; then
    cargo_profile_args+=(--release)
  fi
  say "Building the Rust engine ($ENGINE_PROFILE)…"
  (cd "$REPO_DIR" && cargo build "${cargo_profile_args[@]}" -p pedal-engine --bin pedal-engine)
  say "Starting the engine (audio ${PEDAL_AUDIO_ENABLED:-0}, $ENGINE_PROFILE)…"
  PEDAL_AUDIO_ENABLED=${PEDAL_AUDIO_ENABLED:-0} \
    PEDAL_CONTROL_SOCKET="$CONTROL_SOCKET" \
    PEDAL_STATE_FILE="$STATE_FILE" \
    PEDAL_PRESET_FILE="$PRESET_FILE" \
    PEDAL_PREVIEW_FILE="$PREVIEW_FILE" \
    PEDAL_RIFF_DIRECTORY="$RIFF_DIRECTORY" \
    PEDAL_TEST_AUDIO_FILE="$TEST_AUDIO_FILE" \
    "$ENGINE_BINARY" >"$ENGINE_LOG" 2>&1 &
  pid=$!
  printf '%s\n' "$pid" >"$ENGINE_PID_FILE"
  if ! wait_for_socket "Engine" "$CONTROL_SOCKET" "$ENGINE_PID_FILE" "$ENGINE_LOG"; then
    stop_one "Engine" "$ENGINE_PID_FILE" "$CONTROL_SOCKET" "$ENGINE_BINARY"
    fail "Engine startup failed"
  fi
  ENGINE_STARTED_BY_SCRIPT=1
  ok "Engine ready at $CONTROL_SOCKET"
}

start_catalog() {
  local pid
  if pid=$(pid_from_file "$CATALOG_PID_FILE"); then
    ok "Catalog is already running (PID $pid)"
    return
  fi
  rm -f "$CATALOG_SOCKET" "$CATALOG_PID_FILE"
  say "Starting the local tone catalog…"
  (
    # Keep cloud credentials in the catalog/Tone Maker process only; Flutter
    # never receives them through its process environment.
    if [[ -f "$SECRETS_FILE" ]]; then
      set -a
      # shellcheck disable=SC1090
      source "$SECRETS_FILE"
      set +a
    fi
    PYTHONPATH="$REPO_DIR/services/catalog" \
      PEDAL_TONE3000_TOKEN_FILE="$TOKEN_FILE" \
      PEDAL_RIFF_DIRECTORY="$RIFF_DIRECTORY" \
      PEDAL_BACKING_TRACK_DIRECTORY="$BACKING_TRACK_DIRECTORY" \
      python3 -m pedal_catalog.cli serve \
      --models "$MODELS_DIR" \
      --database "$CATALOG_DB" \
      --socket "$CATALOG_SOCKET"
  ) >"$CATALOG_LOG" 2>&1 &
  pid=$!
  printf '%s\n' "$pid" >"$CATALOG_PID_FILE"
  if ! wait_for_socket "Catalog" "$CATALOG_SOCKET" "$CATALOG_PID_FILE" "$CATALOG_LOG"; then
    stop_one "Catalog" "$CATALOG_PID_FILE" "$CATALOG_SOCKET" "pedal_catalog.cli"
    if [[ "$ENGINE_STARTED_BY_SCRIPT" == 1 ]]; then
      stop_one "Engine" "$ENGINE_PID_FILE" "$CONTROL_SOCKET" "$ENGINE_BINARY"
    fi
    fail "Catalog startup failed"
  fi
  ok "Catalog ready at $CATALOG_SOCKET"
}

stop_one() {
  local name=$1 pid_file=$2 socket=$3 marker=$4 pid attempt command_line
  if ! pid=$(pid_from_file "$pid_file"); then
    rm -f "$pid_file" "$socket"
    printf '  %s was not running\n' "$name"
    return
  fi
  command_line=$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)
  if [[ "$command_line" != *"$marker"* ]]; then
    warn "$name PID file points to an unrelated process; refusing to stop PID $pid"
    rm -f "$pid_file"
    return
  fi
  kill "$pid"
  for attempt in {1..40}; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.05
  done
  if kill -0 "$pid" 2>/dev/null; then
    warn "$name did not stop promptly; leaving PID $pid untouched"
    return
  fi
  rm -f "$pid_file" "$socket"
  ok "$name stopped"
}

up() {
  start_engine
  start_catalog
  printf '\n'
  ok "Backend is ready. Run: tools/dev.sh ui"
  printf '  Models: %s\n  Logs:   %s\n' "$MODELS_DIR" "$RUNTIME_DIR"
}

down() {
  stop_one "Catalog" "$CATALOG_PID_FILE" "$CATALOG_SOCKET" "pedal_catalog.cli"
  stop_one "Engine" "$ENGINE_PID_FILE" "$CONTROL_SOCKET" "$ENGINE_BINARY"
}

run_ui() {
  up
  say "Launching the Flutter UI. Press Ctrl+C to close it."
  cd "$REPO_DIR/apps/pedal_ui"
  PEDAL_CONTROL_SOCKET="$CONTROL_SOCKET" \
    PEDAL_CATALOG_SOCKET="$CATALOG_SOCKET" \
    flutter run -d linux --release
}

demo() {
  trap down EXIT
  run_ui
}

doctor() {
  say "Pedal development status"
  service_status "Engine" "$ENGINE_PID_FILE" "$CONTROL_SOCKET"
  service_status "Catalog" "$CATALOG_PID_FILE" "$CATALOG_SOCKET"
  printf '\n'
  command -v cargo >/dev/null && ok "Rust: $(cargo --version)" || warn "cargo is missing"
  command -v flutter >/dev/null && ok "Flutter: $(flutter --version --machine | python3 -c 'import json,sys; print(json.load(sys.stdin)["frameworkVersion"])')" || warn "Flutter is missing"
  command -v python3 >/dev/null && ok "Python: $(python3 --version)" || warn "python3 is missing"
  local count
  count=$(find "$MODELS_DIR" -maxdepth 1 -type f -iname '*.nam' | wc -l)
  printf '  Local NAM models: %s\n' "${count//[[:space:]]/}"
  if [[ -n "${PEDAL_TONE3000_PUBLISHABLE_KEY:-}" && -n "${PEDAL_TONE3000_REDIRECT_URI:-}" ]]; then
    ok "TONE3000 environment is configured"
  else
    printf '  TONE3000 is disabled (local tones still work)\n'
  fi
  printf '  Runtime directory: %s\n' "$RUNTIME_DIR"
}

logs() {
  local selection=${1:-all}
  case "$selection" in
  engine) tail -n 80 -F "$ENGINE_LOG" ;;
  catalog) tail -n 80 -F "$CATALOG_LOG" ;;
  all) tail -n 80 -F "$ENGINE_LOG" "$CATALOG_LOG" ;;
  *) fail "Choose logs for 'engine', 'catalog', or 'all'" ;;
  esac
}

run_tests() {
  cd "$REPO_DIR"
  make check
  cargo test --workspace
}

usage() {
  cat <<'EOF'
Pedal V0 development harness

Usage: tools/dev.sh COMMAND

  demo              Start everything, launch Flutter, stop on exit
  up                 Start the real Rust engine (without audio) and catalog
  ui                 Start missing services and launch Flutter
  down               Stop development services
  status | doctor    Show services, toolchains, models, and TONE3000 setup
  logs [all|engine|catalog]
                     Follow recent service logs
  test               Run the complete automated verification suite
  help               Show this message

Useful overrides:
  PEDAL_AUDIO_ENABLED=1 tools/dev.sh demo       # use a real audio interface
  PEDAL_AUDIO_DEVICE=hw:CARD=USB tools/dev.sh demo
  PEDAL_AUDIO_CAPTURE_DEVICE=hw:CARD=USB PEDAL_AUDIO_PLAYBACK_DEVICE=pipewire \
    PEDAL_AUDIO_ENABLED=1 tools/dev.sh demo      # split input/output for testing

TONE3000 browsing is enabled automatically when both of these are exported:
  PEDAL_TONE3000_PUBLISHABLE_KEY
  PEDAL_TONE3000_REDIRECT_URI
EOF
}

case "${1:-help}" in
demo) demo ;;
up) up ;;
ui) run_ui ;;
down) down ;;
status | doctor) doctor ;;
logs) logs "${2:-all}" ;;
test) run_tests ;;
help | -h | --help) usage ;;
*)
  usage
  fail "Unknown command: $1"
  ;;
esac
