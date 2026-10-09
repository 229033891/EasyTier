#!/bin/bash
# EasyTier 安装/升级脚本静态检查（shellcheck + bash 语法 + 解析器冒烟）
# 用法: bash check-install.sh

set -euo pipefail

TOOL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET="${TOOL_DIR}/et-ops-common.sh"
INSTALL_SH="${TOOL_DIR}/install.sh"
UPDATE_SH="${TOOL_DIR}/update.sh"

[[ -f "$TARGET" ]] || { echo "未找到: $TARGET" >&2; exit 1; }
[[ -f "$INSTALL_SH" ]] || { echo "未找到: $INSTALL_SH" >&2; exit 1; }
[[ -f "$UPDATE_SH" ]] || { echo "未找到: $UPDATE_SH" >&2; exit 1; }

echo "[check] bash -n ${TARGET}"
bash -n "$TARGET"
echo "[check] bash -n ${INSTALL_SH}"
bash -n "$INSTALL_SH"
echo "[check] bash -n ${UPDATE_SH}"
bash -n "$UPDATE_SH"

# 入口脚本不得互相调用（忽略注释行）
if grep -vE '^[[:space:]]*#' "$INSTALL_SH" | grep -qE '(^|[[:space:]])(exec|bash|source|\.)[[:space:]]+[^[:space:]]*update\.sh'; then
  echo "[fail] install.sh 不得调用 update.sh" >&2
  exit 1
fi
if grep -vE '^[[:space:]]*#' "$UPDATE_SH" | grep -qE '(^|[[:space:]])(exec|bash|source|\.)[[:space:]]+[^[:space:]]*install\.sh'; then
  echo "[fail] update.sh 不得调用 install.sh" >&2
  exit 1
fi
grep -q 'et-ops-common.sh' "$INSTALL_SH" || {
  echo "[fail] install.sh 未引用 et-ops-common.sh" >&2
  exit 1
}
grep -q 'et-ops-common.sh' "$UPDATE_SH" || {
  echo "[fail] update.sh 未引用 et-ops-common.sh" >&2
  exit 1
}

if command -v shellcheck >/dev/null 2>&1; then
  echo "[check] shellcheck ${TARGET}"
  shellcheck -x -e SC1091,SC2034 "$TARGET"
else
  echo "[warn] 未安装 shellcheck，已跳过（Debian/Ubuntu: apt install shellcheck）"
fi

echo "[check] parser smoke tests"
HELPERS="$(awk '
  /^normalize_host_input\(\)/ {keep=1}
  /^json_escape_string\(\)/ {exit}
  keep {print}
' "$TARGET")"
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

grep -q '229033891/EasyTier' "$TARGET" || {
  echo "[fail] GITHUB_REPO 未指向 229033891/EasyTier" >&2
  fail=1
}
grep -q 'RELEASES_PAGE' "$TARGET" || {
  echo "[fail] 缺少 RELEASES_PAGE" >&2
  fail=1
}
grep -q 'setup_nat_forwarding' "$TARGET" || {
  echo "[fail] 缺少 setup_nat_forwarding" >&2
  fail=1
}
grep -q 'reconcile_nat_from_runtime' "$TARGET" || {
  echo "[fail] 缺少 reconcile_nat_from_runtime" >&2
  fail=1
}
grep -q 'parse_config_protocol_needs' "$TARGET" || {
  echo "[fail] 缺少 parse_config_protocol_needs" >&2
  fail=1
}

HELPERS_FW="$(awk '
  /^parse_config_protocol_needs\(\)/ {keep=1}
  /^should_open_extra_core_listeners\(\)/ {exit}
  keep {print}
' "$TARGET")"
eval "$HELPERS_FW"
CONFIG_PROTOCOL='udp,tcp'
parse_config_protocol_needs
assert_eq "$NEED_CONFIG_UDP" "1" "udp,tcp needs udp"
assert_eq "$NEED_CONFIG_TCP" "1" "udp,tcp needs tcp"
CONFIG_PROTOCOL='udp'
parse_config_protocol_needs
assert_eq "$NEED_CONFIG_UDP" "1" "udp-only needs udp"
assert_eq "$NEED_CONFIG_TCP" "0" "udp-only no tcp"

if (( fail )); then
  echo "[fail] parser smoke tests failed" >&2
  exit 1
fi

echo "[ok] install.sh / update.sh / et-ops-common.sh 检查通过（入口互不调用）"
