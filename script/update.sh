#!/bin/bash
#
# EasyTier Linux 升级入口（独立脚本，不调用 install.sh）
# 共享逻辑: et-ops-common.sh（同目录；缺失时从仓库 raw 拉取）
# 从 Release 拉取最新包并替换二进制，保留配置/数据库。
#
# Release: https://github.com/229033891/EasyTier/releases
#
# 用法:
#   sudo bash update.sh
#   sudo bash update.sh --auto
#   sudo bash update.sh --enable-nat
#   sudo bash update.sh --configure-firewall
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd || true)"
ET_COMMON_URL="${ET_COMMON_URL:-https://raw.githubusercontent.com/229033891/EasyTier/main/script/et-ops-common.sh}"

load_et_ops_common() {
  local common="" tmp=""
  if [[ -n "${SCRIPT_DIR:-}" && -f "${SCRIPT_DIR}/et-ops-common.sh" ]]; then
    common="${SCRIPT_DIR}/et-ops-common.sh"
    # shellcheck source=et-ops-common.sh
    source "$common"
    return 0
  fi
  command -v curl >/dev/null 2>&1 || {
    echo "[ERROR] 缺少 et-ops-common.sh 且无 curl，无法继续" >&2
    exit 1
  }
  tmp="$(mktemp)"
  if ! curl -fsSL --connect-timeout 20 --max-time 120 "$ET_COMMON_URL" -o "$tmp"; then
    rm -f "$tmp"
    echo "[ERROR] 无法下载 et-ops-common.sh: ${ET_COMMON_URL}" >&2
    exit 1
  fi
  # shellcheck disable=SC1090
  source "$tmp"
  rm -f "$tmp"
}

load_et_ops_common

update_main() {
  if [[ "${1:-}" == "help" || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
  fi
  if [[ "${1:-}" == "update" ]]; then
    shift || true
  fi
  require_root
  cmd_update "$@"
}

update_main "$@"
