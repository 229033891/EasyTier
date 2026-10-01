#!/bin/bash
# EasyTier 安装脚本静态检查（shellcheck + bash 语法 + 解析器冒烟）
# 用法: bash check-easytier-install.sh

set -euo pipefail

TOOL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET="${TOOL_DIR}/easytier-install.sh"

[[ -f "$TARGET" ]] || { echo "未找到: $TARGET" >&2; exit 1; }

echo "[check] bash -n ${TARGET}"
bash -n "$TARGET"

if command -v shellcheck >/dev/null 2>&1; then
  echo "[check] shellcheck ${TARGET}"
  # SC1091: source; SC2034: 全局配置变量
  shellcheck -x -e SC1091,SC2034 "$TARGET"
else
  echo "[warn] 未安装 shellcheck，已跳过（Debian/Ubuntu: apt install shellcheck）"
fi

echo "[check] parser smoke tests"
# Extract host/URL helpers only (normalize … validate_config_token).
HELPERS="$(awk '
  /^normalize_host_input\(\)/ {keep=1}
  /^json_escape_string\(\)/ {exit}
  keep {print}
' "$TARGET")"
# Stub err used by validate_config_token
err() { echo "$*" >&2; }
# shellcheck disable=SC1090
eval "$HELPERS"

fail=0
assert_eq() {
  local got="$1" want="$2" label="$3"
  if [[ "$got" != "$want" ]]; then
    echo "[fail] $label: got='$got' want='$want'" >&2
    fail=1
  fi
}

assert_eq "$(normalize_host_input 'udp://example.com:22020/admin')" "example.com" "udp URL host"
assert_eq "$(normalize_host_input 'tcp://[2001:db8::1]:22020/tok')" "2001:db8::1" "tcp IPv6 URL host"
assert_eq "$(normalize_host_input '2001:db8::1')" "2001:db8::1" "bare IPv6"
assert_eq "$(normalize_host_input 'https://a.b/c')" "a.b" "https host"
assert_eq "$(normalize_host_input 'host.example:443')" "host.example" "host:port"

parse_config_server_url 'udp://h:22020/admin'
assert_eq "$CS_HOST" "h" "parse host"
assert_eq "$CS_PORT" "22020" "parse port"
assert_eq "$CS_TOKEN" "admin" "parse token"
assert_eq "$CS_SCHEME" "udp" "parse scheme"

parse_config_server_url '"tcp://[2001:db8::2]:22020/my.token"'
assert_eq "$CS_HOST" "2001:db8::2" "quoted IPv6 host"
assert_eq "$CS_TOKEN" "my.token" "quoted token"

assert_eq "$(build_config_server_url udp 2001:db8::1 22020 admin)" \
  "udp://[2001:db8::1]:22020/admin" "build IPv6 URL"
assert_eq "$(build_config_server_url 'udp,tcp' example.com 22020 admin)" \
  "udp://example.com:22020/admin" "build from listen list"
assert_eq "$(primary_config_scheme 'tcp,ws')" "tcp" "primary scheme"

validate_config_token admin || { echo "[fail] validate admin" >&2; fail=1; }
if validate_config_token 'bad token'; then
  echo "[fail] expected reject for space in token" >&2
  fail=1
fi

if (( fail )); then
  echo "[fail] parser smoke tests failed" >&2
  exit 1
fi

echo "[ok] easytier-install.sh 检查通过"
