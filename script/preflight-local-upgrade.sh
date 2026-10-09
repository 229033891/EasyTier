#!/bin/bash
#
# EasyTier 本地升级预检：检查「当前已安装」与「待安装包」状态/配置。
#
# 用法（已是 root，或用 sudo）:
#   bash /root/ET-linux-x86_64/preflight-local-upgrade.sh
#   bash preflight-local-upgrade.sh /root/ET-linux-x86_64
#
# 环境变量:
#   INSTALL_PATH  已安装目录，默认 /opt/easytier
#   PKG_DIR       新包目录，默认 /root/ET-linux-x86_64（或第一个参数）
#   FIX_PERMS=1   预检时给新包补上 +x（默认 1；设 0 关闭）
#
set -euo pipefail

INSTALL_PATH="${INSTALL_PATH:-/opt/easytier}"
CONFIG_DIR="${INSTALL_PATH}/config"
PKG_DIR="${1:-${PKG_DIR:-/root/ET-linux-x86_64}}"
FIX_PERMS="${FIX_PERMS:-1}"

RED=$'\033[31m'
GRN=$'\033[32m'
YLW=$'\033[33m'
BLU=$'\033[34m'
DIM=$'\033[2m'
RST=$'\033[0m'

FAILS=0
WARNS=0

ok()   { echo "${GRN}[OK]${RST} $*"; }
warn() { echo "${YLW}[WARN]${RST} $*"; WARNS=$((WARNS + 1)); }
fail() { echo "${RED}[FAIL]${RST} $*"; FAILS=$((FAILS + 1)); }
info() { echo "${BLU}[INFO]${RST} $*"; }
hdr()  { echo; echo "======== $* ========"; }

# 从 install-options.env 安全读键（禁止 source）
read_opt() {
  local key="$1" f="${INSTALL_PATH}/install-options.env" line val
  [[ -f "$f" ]] || return 0
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" =~ ^[[:space:]]*# ]] && continue
    if [[ "$line" == "${key}="* ]]; then
      val="${line#*=}"
      val="${val%\"}"
      val="${val#\"}"
      val="${val%\'}"
      val="${val#\'}"
      printf '%s' "$val"
      return 0
    fi
  done <"$f"
}

ensure_exec() {
  local f="$1"
  [[ -f "$f" ]] || return 1
  if [[ -x "$f" ]]; then
    return 0
  fi
  if [[ "$FIX_PERMS" == "1" ]] && [[ "$(id -u)" -eq 0 ]]; then
    chmod a+x "$f" 2>/dev/null || chmod 755 "$f" 2>/dev/null || true
  fi
  [[ -x "$f" ]]
}

file_info() {
  local f="$1"
  if [[ -e "$f" ]]; then
    local sz mt mode
    sz="$(du -h "$f" 2>/dev/null | awk '{print $1}')"
    mt="$(stat -c '%y' "$f" 2>/dev/null | cut -d. -f1 || true)"
    mode="$(stat -c '%a' "$f" 2>/dev/null || true)"
    if [[ -x "$f" || -L "$f" ]]; then
      ok "$f  size=${sz}  mtime=${mt}  mode=${mode}"
    else
      warn "$f 存在但不可执行  size=${sz}  mode=${mode}  （安装时用 install -m 755）"
    fi
  else
    fail "缺失: $f"
  fi
}

# 打印并返回版本第一行到 stdout 的调用方用命令替换捕获
bin_version_line() {
  local bin="$1" out=""
  [[ -f "$bin" ]] || return 0
  ensure_exec "$bin" || return 0
  out="$("$bin" -V 2>/dev/null || "$bin" --version 2>/dev/null || true)"
  echo "$out" | head -1
}

show_bin_version() {
  local bin="$1" ver
  ver="$(bin_version_line "$bin")"
  if [[ -n "$ver" ]]; then
    info "版本: ${ver}"
  else
    info "版本: (无法读取 -V；文件可能无执行位)"
  fi
  if command -v file >/dev/null 2>&1 && [[ -f "$bin" ]]; then
    info "ELF: $(file -b "$bin" 2>/dev/null | head -1)"
  fi
}

unit_present() {
  local unit="$1"
  systemctl cat "$unit" &>/dev/null || [[ -f "/etc/systemd/system/${unit}" ]]
}

unit_active() {
  local unit="$1"
  systemctl is-active --quiet "$unit" 2>/dev/null
}

unit_status() {
  local unit="$1" label="$2"
  if ! command -v systemctl >/dev/null 2>&1; then
    warn "无 systemctl，跳过服务检查"
    return
  fi
  if unit_present "$unit"; then
    local enabled active
    enabled="$(systemctl is-enabled "$unit" 2>/dev/null || echo missing)"
    active="$(systemctl is-active "$unit" 2>/dev/null || echo unknown)"
    if [[ "$active" == "active" ]]; then
      ok "${label}: ${unit}  enabled=${enabled}  active=${active}"
    else
      warn "${label}: ${unit}  enabled=${enabled}  active=${active}"
    fi
    local exec_line
    exec_line="$(systemctl show -p ExecStart --value "$unit" 2>/dev/null || true)"
    [[ -n "$exec_line" ]] && info "  ExecStart: ${exec_line}"
  else
    info "${label}: 未安装 unit ${unit}"
  fi
}

listen_ports() {
  local api_port config_port
  api_port="$(read_opt API_PORT)"
  config_port="$(read_opt CONFIG_PORT)"
  api_port="${api_port:-22020}"
  config_port="${config_port:-22020}"

  hdr "监听端口（API=${api_port} 配置下发=${config_port} + 11010-11012）"
  if command -v ss >/dev/null 2>&1; then
    # 勿因 awk 无匹配行 + pipefail 导致整脚本退出
    set +e
    ss -lntup 2>/dev/null | awk -v a="$api_port" -v c="$config_port" '
      NR==1 {print; next}
      $0 ~ (":" a "\\b") || $0 ~ (":" c "\\b") || /:(11010|11011|11012)\b/ {print}
    '
    set -e
  elif command -v netstat >/dev/null 2>&1; then
    netstat -lntup 2>/dev/null | grep -E ":(${api_port}|${config_port}|11010|11011|11012)\\b" || true
  else
    warn "无 ss/netstat"
  fi
}

# ---------- 新包 ----------
hdr "待安装包目录: ${PKG_DIR}"
if [[ ! -d "$PKG_DIR" ]]; then
  fail "目录不存在: ${PKG_DIR}"
  echo "请把 ET-cli / ET-core / ET-web-embed 放到该目录，或传入正确路径。"
  exit 1
fi

NEW_CORE_VER=""
NEW_WEB_VER=""
need_ok=0
for name in ET-core ET-cli ET-web-embed; do
  if [[ -f "${PKG_DIR}/${name}" ]]; then
    if [[ "$FIX_PERMS" == "1" ]] && [[ "$(id -u)" -eq 0 ]] && [[ ! -x "${PKG_DIR}/${name}" ]]; then
      chmod 755 "${PKG_DIR}/${name}" 2>/dev/null || true
      info "已为新包补执行位: ${PKG_DIR}/${name}"
    fi
    file_info "${PKG_DIR}/${name}"
    show_bin_version "${PKG_DIR}/${name}"
    case "$name" in
      ET-core)
        NEW_CORE_VER="$(bin_version_line "${PKG_DIR}/${name}")"
        [[ -x "${PKG_DIR}/${name}" ]] && need_ok=$((need_ok + 1))
        ;;
      ET-cli)
        [[ -x "${PKG_DIR}/${name}" ]] && need_ok=$((need_ok + 1))
        ;;
      ET-web-embed)
        NEW_WEB_VER="$(bin_version_line "${PKG_DIR}/${name}")"
        ;;
    esac
  else
    if [[ "$name" == "ET-web-embed" ]]; then
      warn "可选缺失: ${PKG_DIR}/${name}（仅 server/控制台角色需要）"
    else
      fail "必需缺失: ${PKG_DIR}/${name}"
    fi
  fi
done
if [[ "$need_ok" -lt 2 ]]; then
  fail "新包不完整：需要可执行的 ET-core 与 ET-cli（当前可执行数=${need_ok}）"
else
  ok "新包核心二进制齐全且可执行（ET-core + ET-cli）"
fi

# ---------- 已安装 ----------
hdr "当前安装: ${INSTALL_PATH}"
OLD_CORE_VER=""
OLD_WEB_VER=""
if [[ -d "$INSTALL_PATH" ]]; then
  ok "安装目录存在"
else
  warn "尚未安装到 ${INSTALL_PATH}（首次安装请用 install.sh）"
fi

for name in ET-core ET-cli ET-web-embed ET-web; do
  if [[ -e "${INSTALL_PATH}/${name}" ]]; then
    file_info "${INSTALL_PATH}/${name}"
    show_bin_version "${INSTALL_PATH}/${name}"
    case "$name" in
      ET-core) OLD_CORE_VER="$(bin_version_line "${INSTALL_PATH}/${name}")" ;;
      ET-web-embed) OLD_WEB_VER="$(bin_version_line "${INSTALL_PATH}/${name}")" ;;
    esac
  else
    info "未安装文件: ${INSTALL_PATH}/${name}"
  fi
done

for link in /usr/sbin/ET-core /usr/sbin/ET-cli; do
  if [[ -L "$link" || -e "$link" ]]; then
    info "符号链接: $link -> $(readlink -f "$link" 2>/dev/null || echo '?')"
  fi
done

# ---------- 配置 ----------
hdr "配置与数据"
MODE_OPT="$(read_opt MODE)"
WITH_NODE_OPT="$(read_opt WITH_NODE)"
PUBLIC_HOST_OPT="$(read_opt PUBLIC_HOST)"
API_PORT_OPT="$(read_opt API_PORT)"
CONFIG_PORT_OPT="$(read_opt CONFIG_PORT)"
CONFIG_PROTOCOL_OPT="$(read_opt CONFIG_PROTOCOL)"

if [[ -f "${INSTALL_PATH}/install-options.env" ]]; then
  ok "找到 install-options.env（安装角色/域名等）"
  echo "${DIM}----- install-options.env（敏感值已打码） -----${RST}"
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" =~ ^[[:space:]]*# ]] && continue
    case "$line" in
      *PASSWORD*|*TOKEN*|*SECRET*|*PASS*)
        echo "  ${line%%=*}=******"
        ;;
      *)
        echo "  $line"
        ;;
    esac
  done <"${INSTALL_PATH}/install-options.env"
else
  warn "无 install-options.env（可能不是本脚本安装，或手工部署）"
fi

if [[ -d "$CONFIG_DIR" ]]; then
  ok "配置目录: ${CONFIG_DIR}"
  ls -la "$CONFIG_DIR" 2>/dev/null | sed 's/^/  /' || true
else
  info "无配置目录 ${CONFIG_DIR}"
fi

if [[ -f "${INSTALL_PATH}/et.db" ]]; then
  ok "SQLite: ${INSTALL_PATH}/et.db  ($(du -h "${INSTALL_PATH}/et.db" | awk '{print $1}'))"
else
  info "无 et.db（仅 Web 控制台角色会有）"
fi

# ---------- systemd ----------
hdr "systemd 服务"
unit_status "ET-web.service" "Web 控制台"
unit_status "ET-core@node0.service" "本机节点(server 附带)"
unit_status "ET-core@default.service" "客户端/core 节点"
unit_status "ET-nat.service" "NAT / 出口转发"
unit_status "ET-backup.timer" "每日备份定时器"

NODE0_ON=0
DEFAULT_ON=0
WEB_ON=0
unit_active ET-core@node0.service && NODE0_ON=1
unit_active ET-core@default.service && DEFAULT_ON=1
unit_active ET-web.service && WEB_ON=1

if [[ "$NODE0_ON" -eq 1 && "$DEFAULT_ON" -eq 1 ]]; then
  fail "同时运行 ET-core@node0 与 ET-core@default（命令相同会 peer id conflict）"
  info "server+本机节点一般只留 node0："
  info "  systemctl disable --now ET-core@default.service"
  info "  rm -f /etc/systemd/system/ET-core@default.service && systemctl daemon-reload"
fi

# 推断角色
hdr "推断当前角色"
role="unknown"
if unit_present ET-web.service || [[ -f /etc/systemd/system/ET-web.service ]]; then
  role="server"
elif unit_present ET-core@default.service || [[ -f /etc/systemd/system/ET-core@default.service ]]; then
  role="client/core"
elif [[ -x "${INSTALL_PATH}/ET-core" ]]; then
  role="binaries-only"
fi
info "角色推测: ${role}  (install-options MODE=${MODE_OPT:-?} WITH_NODE=${WITH_NODE_OPT:-?})"
if [[ "$role" == "server" && "${WITH_NODE_OPT}" == "yes" ]]; then
  info "建议保留: ET-web.service + ET-core@node0.service"
fi

listen_ports

# ---------- 新旧对比 ----------
hdr "新旧二进制对比（版本优先，大小其次）"
for name in ET-core ET-cli ET-web-embed; do
  old="${INSTALL_PATH}/${name}"
  new="${PKG_DIR}/${name}"
  if [[ -f "$old" && -f "$new" ]]; then
    osz="$(stat -c '%s' "$old" 2>/dev/null || echo 0)"
    nsz="$(stat -c '%s' "$new" 2>/dev/null || echo 0)"
    info "${name}: 已安装 ${osz} bytes → 新包 ${nsz} bytes"
  elif [[ -f "$new" && ! -f "$old" ]]; then
    info "${name}: 仅新包存在（首次安装该组件）"
  fi
done

if [[ -n "$OLD_CORE_VER" || -n "$NEW_CORE_VER" ]]; then
  if [[ -n "$OLD_CORE_VER" && -n "$NEW_CORE_VER" ]]; then
    if [[ "$OLD_CORE_VER" == "$NEW_CORE_VER" ]]; then
      warn "ET-core 版本相同: ${OLD_CORE_VER}  （升级可能无实质变化）"
    else
      ok "ET-core: ${OLD_CORE_VER}  →  ${NEW_CORE_VER}"
    fi
  else
    info "ET-core 版本: 旧='${OLD_CORE_VER:-?}' 新='${NEW_CORE_VER:-?}'"
  fi
fi
if [[ -n "$OLD_WEB_VER" || -n "$NEW_WEB_VER" ]]; then
  if [[ -n "$OLD_WEB_VER" && -n "$NEW_WEB_VER" ]]; then
    if [[ "$OLD_WEB_VER" == "$NEW_WEB_VER" ]]; then
      warn "ET-web-embed 版本相同: ${OLD_WEB_VER}"
    else
      ok "ET-web-embed: ${OLD_WEB_VER}  →  ${NEW_WEB_VER}"
    fi
  fi
fi

# ---------- 下一步 ----------
hdr "建议的下一步（本脚本只检查，不安装）"
KEEP_CORE="ET-core@node0.service"
if [[ "$role" == "client/core" ]]; then
  KEEP_CORE="ET-core@default.service"
fi

cat <<EOF
1) 备份数据库（有 et.db 时）:
     mkdir -p /root/et-backup
     cp -a ${INSTALL_PATH}/et.db /root/et-backup/et.db.\$(date +%Y%m%d%H%M%S)

2) 手工替换（当前角色: ${role}）:
     systemctl stop ET-web.service ET-core@node0.service ET-core@default.service 2>/dev/null || true
     install -m 755 ${PKG_DIR}/ET-core ${INSTALL_PATH}/ET-core
     install -m 755 ${PKG_DIR}/ET-cli  ${INSTALL_PATH}/ET-cli
     [[ -f ${PKG_DIR}/ET-web-embed ]] && install -m 755 ${PKG_DIR}/ET-web-embed ${INSTALL_PATH}/ET-web-embed
     ln -sfn ${INSTALL_PATH}/ET-core /usr/sbin/ET-core
     ln -sfn ${INSTALL_PATH}/ET-cli  /usr/sbin/ET-cli
     systemctl daemon-reload
     systemctl start ET-web.service 2>/dev/null || true
     systemctl start ${KEEP_CORE} 2>/dev/null || true

3) 若曾误开双实例，升级后只留一个 core:
     # server + WITH_NODE=yes → 留 node0
     systemctl disable --now ET-core@default.service
     rm -f /etc/systemd/system/ET-core@default.service
     systemctl daemon-reload

4) 验证:
     systemctl is-active ET-web.service ET-core@node0.service ET-core@default.service
     ${INSTALL_PATH}/ET-core -V
     ${INSTALL_PATH}/ET-web-embed -V 2>/dev/null || true
EOF

if [[ -n "${PUBLIC_HOST_OPT}" ]]; then
  info "控制台域名: https://${PUBLIC_HOST_OPT}  (API_PORT=${API_PORT_OPT:-?} CONFIG=${CONFIG_PROTOCOL_OPT:-udp}://${CONFIG_PORT_OPT:-?})"
fi

echo
if [[ "$FAILS" -gt 0 ]]; then
  fail "预检结束：${FAILS} 个 FAIL，${WARNS} 个 WARN — 请先处理 FAIL 再升级"
  exit 1
fi
if [[ "$WARNS" -gt 0 ]]; then
  warn "预检结束：0 FAIL，${WARNS} 个 WARN — 确认无误后再升级"
  exit 0
fi
ok "预检完成，未发现 FAIL/WARN。"
