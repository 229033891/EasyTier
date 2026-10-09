#!/bin/bash
#
# EasyTier Linux 安装入口（独立脚本，不调用 update.sh）
# 共享逻辑: et-ops-common.sh（同目录；缺失时从仓库 raw 拉取）
#
# Release: https://github.com/229033891/EasyTier/releases
#
# 用法:
#   sudo bash install.sh
#   sudo bash install.sh install
#   curl -fsSL https://github.com/229033891/EasyTier/raw/main/script/install.sh \
#     | sudo bash -s install
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
  # curl|bash 或单文件场景：下载公共库到临时文件再 source
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

install_main() {
  if [[ $# -eq 0 ]]; then
    prompt_main_menu
    return
  fi
  local cmd="$1"
  shift || true
  case "$cmd" in
    install)   require_root; cmd_install "$@" ;;
    # 使用库内 cmd_update，绝不 exec update.sh
    update)    require_root; cmd_update "$@" ;;
    restore)   cmd_restore "$@" ;;
    backup)    cmd_backup "$@" ;;
    healthcheck) cmd_healthcheck "$@" ;;
    uninstall) cmd_uninstall ;;
    status)    cmd_status ;;
    help|-h|--help) usage ;;
    *) err "未知命令: $cmd"; usage; exit 1 ;;
  esac
}

install_main "$@"
