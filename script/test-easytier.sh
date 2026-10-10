#!/usr/bin/env bash
# Local ET Test preflight for Linux / WSL (mirrors .github/workflows/test.yml).
# Windows: use script/easytier-test-fast.cmd / easytier-test-full.cmd instead.
#
# Usage:
#   ./script/test-easytier.sh              # fast: fmt + lock + clippy + features
#   ./script/test-easytier.sh --full       # + WASI + nextest (no three_node)
#   ./script/test-easytier.sh --full --three-node
#   ./script/test-easytier.sh --install-tools

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PROFILE=fast
THREE_NODE=0
INSTALL_TOOLS=0
SKIP_FMT=0
SKIP_CLIPPY=0
SKIP_FEATURES=0
SKIP_WASI=0
SKIP_TESTS=0

log() { printf '[et-test] %s\n' "$*"; }
die() { printf '[et-test] ERROR: %s\n' "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --full) PROFILE=full; shift ;;
    --fast) PROFILE=fast; shift ;;
    --three-node) THREE_NODE=1; shift ;;
    --install-tools) INSTALL_TOOLS=1; shift ;;
    --skip-fmt) SKIP_FMT=1; shift ;;
    --skip-clippy) SKIP_CLIPPY=1; shift ;;
    --skip-features) SKIP_FEATURES=1; shift ;;
    --skip-wasi) SKIP_WASI=1; shift ;;
    --skip-tests) SKIP_TESTS=1; shift ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *) die "unknown arg: $1" ;;
  esac
done

ensure_tool() {
  local bin="$1" crate="$2" sub="$3"
  if command -v "$bin" >/dev/null 2>&1 || cargo "$sub" --version >/dev/null 2>&1; then
    return 0
  fi
  if [[ "$INSTALL_TOOLS" -ne 1 ]]; then
    die "missing $bin — re-run with --install-tools, or: cargo install $crate"
  fi
  log "cargo install $crate"
  cargo install "$crate" --locked
}

LOG_DIR="$ROOT/artifacts/logs"
mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/easytier-test-$(date +%Y%m%d-%H%M%S).log"
exec > >(tee -a "$LOG_FILE") 2>&1
log "log file: $LOG_FILE"
log "profile=$PROFILE three_node=$THREE_NODE root=$ROOT"

command -v cargo >/dev/null || die "cargo not found"

if [[ "$SKIP_FMT" -eq 0 ]]; then
  log "==> cargo fmt --all -- --check"
  cargo fmt --all -- --check
fi

log "==> cargo metadata --locked"
cargo metadata --format-version 1 --locked >/dev/null

if [[ "$SKIP_CLIPPY" -eq 0 ]]; then
  log "==> cargo clippy --all-targets --features full --all -- -D warnings"
  cargo clippy --all-targets --features full --all -- -D warnings
fi

if [[ "$SKIP_FEATURES" -eq 0 ]]; then
  ensure_tool cargo-hack cargo-hack hack
  log "==> cargo hack check --package easytier --each-feature"
  cargo hack check --package easytier --each-feature --exclude-features macos-ne
fi

if [[ "$PROFILE" == "full" && "$SKIP_WASI" -eq 0 ]]; then
  if rustup target list --installed 2>/dev/null | grep -qx 'wasm32-wasip1'; then
    log "==> cargo check easytier-core (wasm32-wasip1)"
    cargo check --package easytier-core --lib --target wasm32-wasip1 \
      --features management-rpc,proxy-smoltcp-stack,ring-crypto,wasi-crypto-offload
  else
    log "skip WASI (rustup target add wasm32-wasip1)"
  fi
fi

if [[ "$PROFILE" == "full" && "$SKIP_TESTS" -eq 0 ]]; then
  ensure_tool cargo-nextest cargo-nextest nextest
  log "==> cargo nextest (not three_node)"
  cargo nextest run --package easytier --package easytier-core --features full \
    -E 'not test(tests::three_node)' --test-threads 1 --no-fail-fast

  if [[ "$THREE_NODE" -eq 1 ]]; then
    log "==> cargo nextest (three_node)"
    # Best-effort host prep (same ideas as CI; ignore failures if already configured).
    if command -v sudo >/dev/null 2>&1; then
      sudo modprobe tun 2>/dev/null || true
      sudo modprobe br_netfilter 2>/dev/null || true
      sudo sysctl -w net.bridge.bridge-nf-call-iptables=0 2>/dev/null || true
      sudo sysctl -w net.bridge.bridge-nf-call-ip6tables=0 2>/dev/null || true
      sudo sysctl -w net.ipv6.conf.lo.disable_ipv6=0 2>/dev/null || true
      sudo ip addr add 2001:db8::2/64 dev lo 2>/dev/null || true
    fi
    cargo nextest run --package easytier --package easytier-core --features full \
      -E 'test(tests::three_node) and not test(subnet_proxy_three_node_test)' \
      --test-threads 1 --no-fail-fast
    cargo nextest run --package easytier --package easytier-core --features full \
      -E 'test(subnet_proxy_three_node_test)' --test-threads 1 --no-fail-fast
  else
    log "skip three_node (pass --three-node on Linux/WSL)"
  fi
fi

log "ET Test local preflight OK"
