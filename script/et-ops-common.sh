#!/bin/bash
#
# EasyTier 运维公共函数库（勿直接执行）
# 由 install.sh / update.sh 分别 source，二者互不调用。
#
# Release: https://github.com/229033891/EasyTier/releases
#
# 入口:
#   sudo bash install.sh          # 安装 / 备份 / 恢复 / 卸载等
#   sudo bash update.sh           # 从 Release 升级
#

# 允许被 source；若直接执行则提示
if [[ "${BASH_SOURCE[0]:-}" == "${0:-}" ]]; then
  echo "et-ops-common.sh 是函数库，请运行: sudo bash install.sh 或 sudo bash update.sh" >&2
  exit 1
fi

set -euo pipefail

INSTALL_PATH="${INSTALL_PATH:-/opt/easytier}"
CONFIG_DIR="${INSTALL_PATH}/config"
# 固定从本仓库 Release 拉最新包
GITHUB_REPO="${GITHUB_REPO:-229033891/EasyTier}"
RELEASES_PAGE="https://github.com/${GITHUB_REPO}/releases"
GH_PROXY="${GH_PROXY:-}"
NO_GH_PROXY="${NO_GH_PROXY:-false}"
# 国内镜像前缀（空格分隔，GitHub 不可达时依次尝试）
GH_MIRRORS="${GH_MIRRORS:-https://ghfast.top/ https://mirror.ghproxy.com/}"
GITHUB_API_LATEST="https://api.github.com/repos/${GITHUB_REPO}/releases/latest"
GITHUB_RELEASES_LATEST="${RELEASES_PAGE}/latest"

# 默认端口：Web/API 与配置下发共用 22020（TCP / UDP），节点 P2P 11010
API_PORT="${API_PORT:-22020}"
CONFIG_PORT="${CONFIG_PORT:-22020}"
# Server listen protocols (comma-separated). Matches easytier-web default `udp,tcp`.
# Client --config-server URL always uses a single scheme (see primary_config_scheme).
CONFIG_PROTOCOL="${CONFIG_PROTOCOL:-udp,tcp}"
CORE_PORT="${CORE_PORT:-11010}"
# core 默认 listeners 额外端口（wg/ws、wss）；与 default.conf 保持一致
WG_PORT="${WG_PORT:-11011}"
WSS_PORT="${WSS_PORT:-11012}"

MODE=""
AUTO=false
PUBLIC_HOST=""
WITH_NODE="yes"
SERVER_HOST=""
# Config-server URL path token (`udp://host:port/<token>`). Default matches built-in admin.
CONFIG_TOKEN="admin"
# Deprecated alias kept for CLI compat (--web-username).
WEB_USERNAME=""
ADMIN_PASSWORD=""
ADMIN_PASSWORD_PROVIDED=false
DEFAULT_ADMIN_USER="admin"
DEFAULT_ADMIN_PASS="admin"
BACKUP_DIR=""
ENABLE_DAILY_BACKUP="yes"
BACKUP_RETENTION_DAYS="${BACKUP_RETENTION_DAYS:-30}"
BACKUP_HOUR="${BACKUP_HOUR:-3}"
CONFIGURE_FIREWALL="no"
ENABLE_FIREWALL="no"
# NAT/出口转发：内核 ip_forward + MASQUERADE，并启用 enable_exit_node
ENABLE_NAT="no"
NAT_WAN_IFACE="${NAT_WAN_IFACE:-}"
NAT_PROMPT_DONE=false
NGINX_HTTPS_PROXY="no"
NGINX_SSL_PORT="${NGINX_SSL_PORT:-443}"
INSTALL_DEPS="yes"
RESTORE_FILE=""
INSTALLED_VERSION=""
DOWNLOAD_PRESET=false
if [[ -n "${GH_PROXY:-}" ]] || [[ "${NO_GH_PROXY:-false}" == "true" ]]; then
  DOWNLOAD_PRESET=true
fi

RED='\033[1;31m'
GREEN='\033[1;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log()  { echo -e "${GREEN}[INFO]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
err()  { echo -e "${RED}[ERROR]${NC} $*" >&2; }

usage() {
  cat <<EOF
EasyTier Linux 运维脚本

Release: ${RELEASES_PAGE}

独立入口（互不调用，共用 et-ops-common.sh）:
  sudo bash install.sh     # 安装 / 备份 / 恢复 / 卸载 / 健康检查
  sudo bash update.sh      # 从 Release 升级已安装实例

install.sh 常用命令:
  install      安装并配置 systemd 服务（server / client / core）
  restore      从 easytier-db-*.tar.gz 恢复 et.db
  backup       立即备份服务端数据库（server 模式）
  healthcheck  健康检查（API / DNS / 端口监听）
  uninstall    卸载服务与二进制（保留 config 备份）
  status       查看服务状态
  help         显示帮助

update.sh 常用参数:
  --auto                 非交互
  --enable-nat / --no-nat
  --nat-wan IFACE
  --configure-firewall
  --public-host DOMAIN
  --no-gh-proxy / --gh-proxy URL

交互安装时会依次询问:
  - 部署模式（server / client / core）
  - 域名、端口、备份、防火墙、NAT、下载源

自动化安装示例:
  sudo bash install.sh install --mode server --auto --public-host DOMAIN
  sudo bash install.sh install --mode client --auto --server-host DOMAIN --config-token TOKEN
  sudo bash install.sh install --mode core --auto --enable-nat
  sudo bash update.sh --auto

环境变量（可选）:
  INSTALL_PATH、GITHUB_REPO、GH_MIRRORS、GH_PROXY、NO_GH_PROXY、NAT_WAN_IFACE

完整说明见 docs/ops/deploy-install.md
EOF
}

require_root() {
  if [[ "$(id -u)" -ne 0 ]]; then
    err "请使用 root 运行: sudo bash $0 ..."
    exit 1
  fi
}

is_interactive() {
  [[ "$AUTO" != "true" && -t 0 ]]
}

prompt_main_menu() {
  echo
  echo "=========================================="
  echo " EasyTier 部署工具"
  echo " Release: ${RELEASES_PAGE}"
  echo "=========================================="
  echo
  echo "请选择操作:"
  echo "  1) 安装 EasyTier"
  echo "  2) 更新到最新版本"
  echo "  3) 立即备份数据库"
  echo "  4) 从备份恢复"
  echo "  5) 健康检查"
  echo "  6) 查看状态"
  echo "  7) 卸载"
  echo "  0) 退出"
  echo
  read -rp "请输入 [1]: " choice
  choice="${choice:-1}"
  case "$choice" in
    1) require_root; cmd_install ;;
    2) require_root; cmd_update ;;
    3) require_root; cmd_backup ;;
    4) require_root; cmd_restore ;;
    5) cmd_healthcheck ;;
    6) cmd_status ;;
    7) require_root; cmd_uninstall ;;
    0) log "已退出"; exit 0 ;;
    *) err "无效选择: $choice"; exit 1 ;;
  esac
}

prompt_download_source() {
  [[ "$DOWNLOAD_PRESET" == "true" ]] && return 0
  [[ "$AUTO" == "true" ]] && return 0
  [[ ! -t 0 ]] && return 0

  echo
  echo "下载源选择:"
  if github_is_reachable; then
    echo "  （检测到 GitHub 可访问）"
  else
    echo "  （检测到 GitHub 不可达，建议选择镜像）"
  fi
  echo "  1) 自动（推荐）- 先直连，失败再试国内镜像"
  echo "  2) 仅直连 GitHub（适合海外 VPS）"
  echo "  3) ghfast.top 镜像"
  echo "  4) ghproxy 镜像"
  echo "  5) 自定义镜像前缀"
  read -rp "请选择 [1]: " c
  c="${c:-1}"
  case "$c" in
    1) NO_GH_PROXY=false; GH_PROXY="" ;;
    2) NO_GH_PROXY=true; GH_PROXY="" ;;
    3) NO_GH_PROXY=false; GH_PROXY="https://ghfast.top/" ;;
    4) NO_GH_PROXY=false; GH_PROXY="https://mirror.ghproxy.com/" ;;
    5)
      read -rp "镜像前缀 URL（如 https://ghfast.top/）: " GH_PROXY
      [[ -n "$GH_PROXY" ]] || { err "镜像地址不能为空"; exit 1; }
      NO_GH_PROXY=false
      ;;
    *) err "无效选择: $c"; exit 1 ;;
  esac
  log "下载源: $(describe_download_source)"
}

describe_download_source() {
  if [[ "$NO_GH_PROXY" == "true" ]]; then
    echo "仅直连 GitHub"
  elif [[ -n "$GH_PROXY" ]]; then
    echo "镜像 ${GH_PROXY}"
  else
    echo "自动（GitHub + 国内镜像回退）"
  fi
}

validate_port() {
  local name="$1"
  local port="$2"
  [[ "$port" =~ ^[0-9]+$ ]] && (( port >= 1 && port <= 65535 )) || {
    err "${name} 端口无效: ${port}"
    exit 1
  }
}

validate_backup_hour() {
  [[ "$BACKUP_HOUR" =~ ^[0-9]+$ ]] && (( BACKUP_HOUR >= 0 && BACKUP_HOUR <= 23 )) || {
    err "备份时刻无效: ${BACKUP_HOUR}（须 0-23）"
    exit 1
  }
}

normalize_host_input() {
  local raw="$1"
  raw="$(echo "$raw" | tr -d '[:space:]')"
  [[ -n "$raw" ]] || { echo ""; return 0; }

  # Full tunnel / HTTP URL → extract host only (never leave scheme like "udp").
  if [[ "$raw" =~ ^(https?|udp|tcp|ws|wss):// ]]; then
    parse_config_server_url "$raw"
    echo "${CS_HOST}"
    return 0
  fi

  # Strip accidental http(s) leftovers and path/query.
  raw="$(echo "$raw" | sed -E 's#^https?://##I')"
  raw="${raw%%/*}"
  raw="${raw%%\?*}"

  # [ipv6] or [ipv6]:port
  if [[ "$raw" == \[*\]* ]]; then
    echo "$raw" | sed -E 's/^\[([^]]+)\].*/\1/'
    return 0
  fi

  # Bare IPv6 (multiple colons) — do not cut on ':'
  if [[ "$raw" == *:*:* ]]; then
    echo "$raw"
    return 0
  fi

  # hostname / IPv4 / host:port
  echo "${raw%%:*}"
}

# Parse config-server URL into CS_SCHEME / CS_HOST / CS_PORT / CS_TOKEN (may be empty).
parse_config_server_url() {
  local raw="$1"
  CS_SCHEME=""
  CS_HOST=""
  CS_PORT=""
  CS_TOKEN=""
  raw="$(echo "$raw" | tr -d '[:space:]')"
  # systemctl / shell may leave surrounding quotes
  raw="${raw#\"}"
  raw="${raw%\"}"
  raw="${raw#\'}"
  raw="${raw%\'}"
  [[ -n "$raw" ]] || return 0

  if [[ "$raw" =~ ^([A-Za-z][A-Za-z0-9+.-]*)://(.+)$ ]]; then
    CS_SCHEME="$(echo "${BASH_REMATCH[1]}" | tr '[:upper:]' '[:lower:]')"
    raw="${BASH_REMATCH[2]}"
  fi

  if [[ "$raw" == */* ]]; then
    local path_part="${raw#*/}"
    CS_TOKEN="${path_part%%/*}"
    CS_TOKEN="${CS_TOKEN%%\?*}"
    raw="${raw%%/*}"
  fi

  if [[ "$raw" == \[* ]]; then
    CS_HOST="$(echo "$raw" | sed -E 's/^\[([^]]+)\].*/\1/')"
    local rest="${raw#*\]}"
    if [[ "$rest" == :* ]]; then
      CS_PORT="${rest#:}"
      CS_PORT="${CS_PORT%%/*}"
    fi
  elif [[ "$raw" == *:*:* ]]; then
    CS_HOST="$raw"
  else
    CS_HOST="${raw%%:*}"
    if [[ "$raw" == *:* ]]; then
      CS_PORT="${raw#*:}"
      CS_PORT="${CS_PORT%%/*}"
    fi
  fi
}

# First scheme from comma list (client URLs must be single-scheme).
primary_config_scheme() {
  local spec="${1:-$CONFIG_PROTOCOL}"
  local first="${spec%%,*}"
  first="$(echo "$first" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
  [[ -n "$first" ]] || first="udp"
  echo "$first"
}

# Build udp|tcp://host:port/token (brackets IPv6). protocol may be a list → uses primary.
build_config_server_url() {
  local protocol
  protocol="$(primary_config_scheme "$1")"
  local host="$2"
  local port="$3"
  local token="$4"
  if [[ "$host" == *:* ]]; then
    echo "${protocol}://[${host}]:${port}/${token}"
  else
    echo "${protocol}://${host}:${port}/${token}"
  fi
}

# Same rules as easytier-web validate_config_token_value.
validate_config_token() {
  local token="$1"
  local label="${2:-Token}"
  if [[ -z "$token" ]]; then
    err "${label}不能为空"
    return 1
  fi
  if [[ "$token" == revoked_* ]]; then
    err "${label}不能使用 reserved 前缀 revoked_"
    return 1
  fi
  if (( ${#token} > 128 )); then
    err "${label}过长（最多 128 字符）"
    return 1
  fi
  if [[ ! "$token" =~ ^[A-Za-z0-9._-]+$ ]]; then
    err "${label}仅允许字母、数字、'.'、'_'、'-'"
    return 1
  fi
  return 0
}

json_escape_string() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\n'/\\n}"
  s="${s//$'\r'/\\r}"
  s="${s//$'\t'/\\t}"
  printf '%s' "$s"
}

# Sync deprecated WEB_USERNAME into CONFIG_TOKEN when set.
apply_config_token_alias() {
  if [[ -n "${WEB_USERNAME:-}" ]]; then
    CONFIG_TOKEN="$WEB_USERNAME"
  fi
}

# 合法域名或 IP（仅允许 hostname 安全字符，避免 env/sed 注入）
is_valid_host_format() {
  local host="$1"
  [[ -n "$host" ]] || return 1
  # 白名单：字母数字、点、连字符、冒号（IPv6）
  [[ "$host" =~ ^[a-zA-Z0-9.:-]+$ ]] || return 1
  if [[ "$host" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]]; then
    local o o1 o2 o3 o4
    IFS='.' read -r o1 o2 o3 o4 <<<"$host"
    for o in "$o1" "$o2" "$o3" "$o4"; do
      (( o >= 0 && o <= 255 )) || return 1
    done
    return 0
  fi
  if [[ "$host" == *:* ]]; then
    [[ "$host" =~ ^[0-9a-fA-F:.]+$ ]] && return 0
    return 1
  fi
  [[ "$host" =~ ^[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$ ]]
}

validate_host_format() {
  local label="$1"
  local host="$2"
  if is_valid_host_format "$host"; then
    return 0
  fi
  err "${label}格式无效: ${host}（须为合法域名或 IP，不含空格与特殊字符）"
  return 1
}

validate_required_host() {
  local label="$1"
  local host="$2"
  [[ -n "$host" ]] || { err "${label}不能为空"; return 1; }
  validate_host_format "$label" "$host"
}

# 从 install-options.env 安全读取（禁止 source，避免 shell 注入）
read_install_option() {
  local key="$1"
  local f="${INSTALL_PATH}/install-options.env"
  local line val
  [[ -f "$f" ]] || return 1
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ "$line" =~ ^[[:space:]]*# ]] && continue
    [[ "$line" =~ ^[[:space:]]*${key}=(.*)$ ]] || continue
    val="${BASH_REMATCH[1]}"
    val="${val#\"}"
    val="${val%\"}"
    val="${val#\'}"
    val="${val%\'}"
    printf '%s' "$val"
    return 0
  done <"$f"
  return 1
}

# systemd --api-host 协议优先于 install-options.env
reconcile_nginx_mode_from_api_host() {
  local api_host="$1"
  [[ -n "$api_host" ]] || return 0
  PUBLIC_HOST="$(normalize_host_input "$api_host")"
  if [[ "$api_host" == https://* ]]; then
    NGINX_HTTPS_PROXY=yes
  elif [[ "$api_host" == http://* ]]; then
    NGINX_HTTPS_PROXY=no
  fi
}

prompt_required_host() {
  local prompt="$1"
  local __varname="$2"
  local value=""
  while true; do
    read -rp "${prompt}: " value
    value="$(normalize_host_input "$value")"
    if [[ -z "$value" ]]; then
      warn "不能为空，请重新输入"
      continue
    fi
    if validate_host_format "$prompt" "$value"; then
      break
    fi
  done
  printf -v "$__varname" '%s' "$value"
}

build_release_download_url() {
  local base_url="$1"
  if [[ -n "$GH_PROXY" && "$NO_GH_PROXY" != "true" ]]; then
    gh_mirror_url "$GH_PROXY" "$base_url"
  else
    echo "$base_url"
  fi
}

gh_mirror_url() {
  local prefix="$1"
  local url="$2"
  prefix="${prefix%/}/"
  echo "${prefix}${url}"
}

github_is_reachable() {
  curl -fsS --connect-timeout 5 --max-time 10 \
    "https://api.github.com/zen" -o /dev/null 2>/dev/null
}

# 生成候选下载 URL：直连 + 镜像（GitHub 不可达时优先镜像）
build_download_candidates() {
  local url="$1"
  if [[ "$NO_GH_PROXY" == "true" ]]; then
    echo "$url"
    return 0
  fi
  if [[ -n "$GH_PROXY" ]]; then
    gh_mirror_url "$GH_PROXY" "$url"
    return 0
  fi

  local mirror reachable=true
  if ! github_is_reachable; then
    reachable=false
    warn "GitHub 不可达，将使用国内镜像下载"
  fi

  if [[ "$reachable" == "true" ]]; then
    echo "$url"
  fi
  local m
  for m in $GH_MIRRORS; do
    [[ -n "$m" ]] || continue
    gh_mirror_url "$m" "$url"
  done
  if [[ "$reachable" != "true" ]]; then
    echo "$url"
  fi
}

fetch_json_with_fallback() {
  local url="$1"
  local candidate body
  while IFS= read -r candidate; do
    [[ -n "$candidate" ]] || continue
    log "查询版本: ${candidate}"
    if body="$(curl -fsSL --connect-timeout 12 --max-time 30 "$candidate" 2>/dev/null)" && \
       [[ "$body" == *tag_name* ]]; then
      printf '%s' "$body"
      return 0
    fi
    warn "版本信息获取失败，尝试下一个源..."
  done < <(build_download_candidates "$url")
  return 1
}

fetch_file_with_fallback() {
  local url="$1"
  local dest="$2"
  local candidate n=0
  while IFS= read -r candidate; do
    [[ -n "$candidate" ]] || continue
    n=$((n + 1))
    log "下载 (#${n}): ${candidate}"
    if curl -fL --connect-timeout 20 --max-time 900 --progress-bar \
      "$candidate" -o "$dest" 2>/dev/null && \
      unzip -tq "$dest" >/dev/null 2>&1; then
      log "下载完成"
      return 0
    fi
    warn "下载失败或 zip 损坏，尝试下一个源..."
    rm -f "$dest"
  done < <(build_download_candidates "$url")
  return 1
}

remove_systemd_units() {
  # New ET-* units
  rm -f /etc/systemd/system/ET-web.service
  rm -f /etc/systemd/system/ET-core@node0.service
  rm -f /etc/systemd/system/ET-core@default.service
  rm -f /etc/systemd/system/ET-core@.service
  rm -f /etc/systemd/system/ET-backup.service
  rm -f /etc/systemd/system/ET-backup.timer
  rm -f /etc/systemd/system/ET-nat.service
  # Legacy easytier-* units (pre-rename installs)
  rm -f /etc/systemd/system/easytier-web.service
  rm -f /etc/systemd/system/easytier-core@node0.service
  rm -f /etc/systemd/system/easytier-core@default.service
  rm -f /etc/systemd/system/easytier-core@.service
  rm -f /etc/systemd/system/easytier-backup.service
  rm -f /etc/systemd/system/easytier-backup.timer
}

stop_legacy_easytier_services() {
  systemctl stop easytier-web.service 2>/dev/null || true
  systemctl stop easytier-core@node0.service 2>/dev/null || true
  systemctl stop easytier-core@default.service 2>/dev/null || true
  systemctl disable easytier-web.service 2>/dev/null || true
  systemctl disable easytier-core@node0.service 2>/dev/null || true
  systemctl disable easytier-core@default.service 2>/dev/null || true
}

detect_arch() {
  local platform
  platform="$(uname -m)"
  case "$platform" in
    x86_64|amd64) echo "x86_64" ;;
    aarch64|arm64) echo "aarch64" ;;
    armv7*|armv7) echo "armv7" ;;
    arm*) echo "arm" ;;
    *) err "不支持的架构: $platform"; exit 1 ;;
  esac
}

need_cmd() {
  for c in curl unzip systemctl; do
    command -v "$c" >/dev/null 2>&1 || { err "缺少命令: $c"; exit 1; }
  done
}

ensure_debian_packages() {
  [[ "$INSTALL_DEPS" == "yes" ]] || return 0
  command -v apt-get >/dev/null 2>&1 || return 0

  local missing=()
  command -v sqlite3 >/dev/null 2>&1 || missing+=(sqlite3)
  command -v curl >/dev/null 2>&1 || missing+=(curl)
  command -v unzip >/dev/null 2>&1 || missing+=(unzip)

  if [[ ${#missing[@]} -eq 0 ]]; then
    return 0
  fi

  log "安装依赖: ${missing[*]}"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y "${missing[@]}"
}

# 本机是否正在过滤流量（仅判断已启用的防火墙）
firewall_is_active() {
  if command -v ufw >/dev/null 2>&1; then
    ufw status 2>/dev/null | grep -qi "Status: active" && return 0
  fi
  if command -v firewall-cmd >/dev/null 2>&1 && systemctl is-active --quiet firewalld 2>/dev/null; then
    return 0
  fi
  return 1
}

detect_firewall_backend() {
  if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -qi "Status: active"; then
    echo "ufw"
    return 0
  fi
  if command -v firewall-cmd >/dev/null 2>&1 && systemctl is-active --quiet firewalld 2>/dev/null; then
    echo "firewalld"
    return 0
  fi
  echo "none"
}

# 解析 CONFIG_PROTOCOL（支持逗号列表，如 udp,tcp）→ NEED_CONFIG_UDP / NEED_CONFIG_TCP
parse_config_protocol_needs() {
  NEED_CONFIG_UDP=0
  NEED_CONFIG_TCP=0
  local proto
  local -a protos=()
  IFS=',' read -ra protos <<<"${CONFIG_PROTOCOL}"
  for proto in "${protos[@]}"; do
    proto="$(printf '%s' "$proto" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
    case "$proto" in
      udp) NEED_CONFIG_UDP=1 ;;
      tcp|ws|wss) NEED_CONFIG_TCP=1 ;;
    esac
  done
}

# 本机节点 / core 时额外放行默认 listeners（wg/ws/wss）
should_open_extra_core_listeners() {
  [[ "$MODE" == "core" ]] && return 0
  [[ "$MODE" == "server" && "$WITH_NODE" == "yes" ]] && return 0
  return 1
}

print_firewall_port_hint() {
  parse_config_protocol_needs
  echo
  log "若对外提供服务，请确保以下端口可达（本机 / 路由器 / 云安全组）:"
  if [[ "$NGINX_HTTPS_PROXY" == "yes" ]]; then
    echo "  TCP ${NGINX_SSL_PORT}          Web 控制台（Nginx HTTPS）"
    echo "  （Web 无需对外 TCP ${API_PORT}；Nginx 本机反代 127.0.0.1:${API_PORT}）"
  else
    echo "  TCP ${API_PORT}     Web / API"
  fi
  if (( NEED_CONFIG_UDP )); then
    echo "  UDP ${CONFIG_PORT}     配置下发（Client 直连，必须）"
  fi
  if (( NEED_CONFIG_TCP )); then
    echo "  TCP ${CONFIG_PORT}     配置下发（tcp/ws/wss）"
  fi
  if (( !NEED_CONFIG_UDP && !NEED_CONFIG_TCP )); then
    echo "  ${CONFIG_PROTOCOL} ${CONFIG_PORT}  配置下发（未知协议，请手动放行）"
  fi
  echo "  UDP/TCP ${CORE_PORT}  节点 P2P 组网"
  if should_open_extra_core_listeners; then
    echo "  UDP/TCP ${WG_PORT}   WireGuard / WebSocket"
    echo "  TCP ${WSS_PORT}      WebSocket Secure"
  fi
  echo
}

ufw_rule_exists() {
  local pattern="$1"
  ufw status numbered 2>/dev/null | grep -qF "$pattern"
}

configure_ufw_rules() {
  local force_enable="${1:-no}"

  if ! command -v ufw >/dev/null 2>&1; then
    warn "未安装 ufw，跳过（Debian/Ubuntu: apt install ufw）"
    return 1
  fi

  if [[ "$force_enable" == "yes" ]] || [[ "$ENABLE_FIREWALL" == "yes" ]]; then
    if ! ufw status 2>/dev/null | grep -qi "Status: active"; then
      log "启用 UFW 前先放行 SSH..."
      ufw allow OpenSSH >/dev/null 2>&1 || ufw allow 22/tcp comment 'SSH'
      ufw --force enable
      log "UFW 已启用"
    fi
  fi

  if ! ufw status 2>/dev/null | grep -qi "Status: active"; then
    return 1
  fi

  parse_config_protocol_needs

  if [[ "$NGINX_HTTPS_PROXY" == "yes" ]]; then
    if ! ufw_rule_exists "${NGINX_SSL_PORT}/tcp"; then
      ufw allow "${NGINX_SSL_PORT}/tcp" comment 'Nginx HTTPS EasyTier'
      log "UFW: 已放行 TCP ${NGINX_SSL_PORT}（Nginx HTTPS）"
    fi
    log "Web 经 Nginx 反代，跳过对外放行 Web TCP ${API_PORT}（配置下发端口仍按协议放行）"
  elif ! ufw_rule_exists "${API_PORT}/tcp"; then
    ufw allow "${API_PORT}/tcp" comment 'EasyTier Web API'
    log "UFW: 已放行 TCP ${API_PORT}"
  fi

  # 支持 udp,tcp 等多协议；与 Web 同端口时 ufw_rule_exists 会跳过重复
  if (( NEED_CONFIG_UDP )); then
    if ! ufw_rule_exists "${CONFIG_PORT}/udp"; then
      ufw allow "${CONFIG_PORT}/udp" comment 'EasyTier Config UDP'
      log "UFW: 已放行 UDP ${CONFIG_PORT}（配置下发）"
    fi
  fi
  if (( NEED_CONFIG_TCP )); then
    if ! ufw_rule_exists "${CONFIG_PORT}/tcp"; then
      ufw allow "${CONFIG_PORT}/tcp" comment 'EasyTier Config TCP'
      log "UFW: 已放行 TCP ${CONFIG_PORT}（配置下发）"
    fi
  fi

  if ! ufw_rule_exists "${CORE_PORT}/udp"; then
    ufw allow "${CORE_PORT}/udp" comment 'EasyTier P2P UDP'
    log "UFW: 已放行 UDP ${CORE_PORT}"
  fi
  if ! ufw_rule_exists "${CORE_PORT}/tcp"; then
    ufw allow "${CORE_PORT}/tcp" comment 'EasyTier P2P TCP'
    log "UFW: 已放行 TCP ${CORE_PORT}"
  fi

  if should_open_extra_core_listeners; then
    if ! ufw_rule_exists "${WG_PORT}/udp"; then
      ufw allow "${WG_PORT}/udp" comment 'EasyTier WG/WS UDP'
      log "UFW: 已放行 UDP ${WG_PORT}（WG/WS）"
    fi
    if ! ufw_rule_exists "${WG_PORT}/tcp"; then
      ufw allow "${WG_PORT}/tcp" comment 'EasyTier WG/WS TCP'
      log "UFW: 已放行 TCP ${WG_PORT}（WG/WS）"
    fi
    if ! ufw_rule_exists "${WSS_PORT}/tcp"; then
      ufw allow "${WSS_PORT}/tcp" comment 'EasyTier WSS'
      log "UFW: 已放行 TCP ${WSS_PORT}（WSS）"
    fi
  fi

  ufw status numbered 2>/dev/null | grep -E "EasyTier|Nginx|${API_PORT}|${CONFIG_PORT}|${CORE_PORT}|${WG_PORT}|${WSS_PORT}|${NGINX_SSL_PORT}" || true
  return 0
}

firewalld_port_exists() {
  local spec="$1"
  firewall-cmd --list-ports 2>/dev/null | grep -qF "$spec"
}

configure_firewalld_rules() {
  if ! command -v firewall-cmd >/dev/null 2>&1; then
    warn "未安装 firewalld，跳过"
    return 1
  fi
  if ! systemctl is-active --quiet firewalld 2>/dev/null; then
    return 1
  fi

  parse_config_protocol_needs
  local permanent=()
  if [[ "$NGINX_HTTPS_PROXY" == "yes" ]]; then
    if ! firewalld_port_exists "${NGINX_SSL_PORT}/tcp"; then
      permanent+=(--add-port="${NGINX_SSL_PORT}/tcp")
      log "firewalld: 将放行 TCP ${NGINX_SSL_PORT}（Nginx HTTPS）"
    fi
    log "Web 经 Nginx 反代，跳过对外放行 Web TCP ${API_PORT}（配置下发端口仍按协议放行）"
  elif ! firewalld_port_exists "${API_PORT}/tcp"; then
    permanent+=(--add-port="${API_PORT}/tcp")
    log "firewalld: 将放行 TCP ${API_PORT}"
  fi
  if (( NEED_CONFIG_UDP )) && ! firewalld_port_exists "${CONFIG_PORT}/udp"; then
    permanent+=(--add-port="${CONFIG_PORT}/udp")
    log "firewalld: 将放行 UDP ${CONFIG_PORT}（配置下发）"
  fi
  if (( NEED_CONFIG_TCP )) && ! firewalld_port_exists "${CONFIG_PORT}/tcp"; then
    permanent+=(--add-port="${CONFIG_PORT}/tcp")
    log "firewalld: 将放行 TCP ${CONFIG_PORT}（配置下发）"
  fi
  if ! firewalld_port_exists "${CORE_PORT}/udp"; then
    permanent+=(--add-port="${CORE_PORT}/udp")
  fi
  if ! firewalld_port_exists "${CORE_PORT}/tcp"; then
    permanent+=(--add-port="${CORE_PORT}/tcp")
  fi
  if should_open_extra_core_listeners; then
    if ! firewalld_port_exists "${WG_PORT}/udp"; then
      permanent+=(--add-port="${WG_PORT}/udp")
    fi
    if ! firewalld_port_exists "${WG_PORT}/tcp"; then
      permanent+=(--add-port="${WG_PORT}/tcp")
    fi
    if ! firewalld_port_exists "${WSS_PORT}/tcp"; then
      permanent+=(--add-port="${WSS_PORT}/tcp")
    fi
  fi

  for rule in "${permanent[@]}"; do
    firewall-cmd --permanent "$rule"
  done
  firewall-cmd --reload
  firewall-cmd --list-ports 2>/dev/null | tr ' ' '\n' | \
    grep -E "${API_PORT}|${CONFIG_PORT}|${CORE_PORT}|${WG_PORT}|${WSS_PORT}|${NGINX_SSL_PORT}" || true
  return 0
}

configure_server_firewall() {
  [[ "$MODE" == "server" ]] || return 0

  print_firewall_port_hint

  # 用户明确要求启用防火墙
  if [[ "$ENABLE_FIREWALL" == "yes" ]]; then
    if configure_ufw_rules yes; then
      log "本机防火墙（UFW）规则已配置"
      return 0
    fi
    if command -v firewall-cmd >/dev/null 2>&1 && \
       ! systemctl is-active --quiet firewalld 2>/dev/null; then
      log "尝试启动 firewalld..."
      systemctl start firewalld 2>/dev/null || true
    fi
    if configure_firewalld_rules; then
      log "本机防火墙（firewalld）规则已配置"
      return 0
    fi
    warn "无法启用/配置本机防火墙，请手动处理或配置上游安全组"
    return 0
  fi

  # 未要求配置本机防火墙
  if [[ "$CONFIGURE_FIREWALL" != "yes" ]]; then
    if firewall_is_active; then
      warn "检测到本机防火墙正在运行，但未自动放行。重新安装时可在交互步骤中选择放行"
    else
      log "本机防火墙未启用，已跳过本机端口规则（无需本机放行）"
    fi
    return 0
  fi

  # --configure-firewall：仅在本机防火墙已启用时写规则
  if ! firewall_is_active; then
    log "本机防火墙未启用，跳过本机端口规则（无需本机放行）"
    return 0
  fi

  local backend
  backend="$(detect_firewall_backend)"
  case "$backend" in
    ufw)
      configure_ufw_rules no && log "本机防火墙（UFW）规则已配置"
      ;;
    firewalld)
      configure_firewalld_rules && log "本机防火墙（firewalld）规则已配置"
      ;;
    *)
      warn "未识别的本机防火墙，请根据上方端口列表手动配置"
      ;;
  esac
}

# --- NAT / 出口转发 -----------------------------------------------------------

detect_default_wan_iface() {
  local iface=""
  if command -v ip >/dev/null 2>&1; then
    iface="$(ip -4 route show default 2>/dev/null | awk '/default/{for(i=1;i<=NF;i++) if($i=="dev"){print $(i+1); exit}}')"
    if [[ -z "$iface" ]]; then
      iface="$(ip -6 route show default 2>/dev/null | awk '/default/{for(i=1;i<=NF;i++) if($i=="dev"){print $(i+1); exit}}')"
    fi
  fi
  if [[ -n "$iface" && -d "/sys/class/net/${iface}" ]]; then
    echo "$iface"
    return 0
  fi
  return 1
}

resolve_nat_wan_iface() {
  local iface="${NAT_WAN_IFACE:-}"
  if [[ -n "$iface" ]]; then
    [[ -d "/sys/class/net/${iface}" ]] || {
      err "指定的 WAN 网卡不存在: ${iface}"
      return 1
    }
    echo "$iface"
    return 0
  fi
  iface="$(detect_default_wan_iface || true)"
  [[ -n "$iface" ]] || {
    err "无法自动检测默认路由网卡，请用 --nat-wan IFACE 指定"
    return 1
  }
  echo "$iface"
}

nat_comment() { echo "ET-NAT"; }

enable_kernel_ip_forward() {
  local conf="/etc/sysctl.d/99-easytier-forward.conf"
  cat >"$conf" <<'EOF'
# Managed by EasyTier install.sh — NAT / exit-node forwarding
net.ipv4.ip_forward=1
net.ipv6.conf.all.forwarding=1
EOF
  chmod 644 "$conf"
  # Apply only our keys (avoid failing on unrelated sysctl.d errors)
  sysctl -w net.ipv4.ip_forward=1 >/dev/null
  sysctl -w net.ipv6.conf.all.forwarding=1 >/dev/null 2>/dev/null || true
  local v4
  v4="$(sysctl -n net.ipv4.ip_forward 2>/dev/null || echo 0)"
  [[ "$v4" == "1" ]] || {
    err "启用 net.ipv4.ip_forward 失败（当前=${v4}）"
    return 1
  }
  log "已启用内核转发: net.ipv4.ip_forward=1（持久化 ${conf}）"
}

disable_kernel_ip_forward_dropin() {
  rm -f /etc/sysctl.d/99-easytier-forward.conf
  # 不强制关闭全局 forwarding（可能被其它服务依赖），仅移除本脚本 drop-in
}

# 幂等写入/清理 iptables 规则（专用 comment，可安全重复执行）
iptables_ensure() {
  local table="$1"; shift
  # remaining: rule args after -A/-D
  if [[ "$table" == "filter" ]]; then
    iptables -C "$@" >/dev/null 2>&1 || iptables -A "$@"
  else
    iptables -t "$table" -C "$@" >/dev/null 2>&1 || iptables -t "$table" -A "$@"
  fi
}

iptables_delete_if_exists() {
  local table="$1"; shift
  if [[ "$table" == "filter" ]]; then
    while iptables -C "$@" >/dev/null 2>&1; do
      iptables -D "$@" || break
    done
  else
    while iptables -t "$table" -C "$@" >/dev/null 2>&1; do
      iptables -t "$table" -D "$@" || break
    done
  fi
}

apply_iptables_nat() {
  local wan="$1"
  local c
  c="$(nat_comment)"
  command -v iptables >/dev/null 2>&1 || {
    err "需要 iptables 才能配置 NAT"
    return 1
  }
  # RELATED,ESTABLISHED 回程
  iptables_ensure filter FORWARD -m conntrack --ctstate RELATED,ESTABLISHED -m comment --comment "$c" -j ACCEPT
  # 允许发往 WAN 的转发
  iptables_ensure filter FORWARD -o "$wan" -m comment --comment "$c" -j ACCEPT
  # MASQUERADE
  iptables_ensure nat POSTROUTING -o "$wan" -m comment --comment "$c" -j MASQUERADE
  return 0
}

remove_iptables_nat() {
  local wan="$1"
  local c
  c="$(nat_comment)"
  command -v iptables >/dev/null 2>&1 || return 0
  iptables_delete_if_exists nat POSTROUTING -o "$wan" -m comment --comment "$c" -j MASQUERADE
  iptables_delete_if_exists filter FORWARD -o "$wan" -m comment --comment "$c" -j ACCEPT
  iptables_delete_if_exists filter FORWARD -m conntrack --ctstate RELATED,ESTABLISHED -m comment --comment "$c" -j ACCEPT
}

apply_firewalld_masquerade() {
  command -v firewall-cmd >/dev/null 2>&1 || return 0
  systemctl is-active --quiet firewalld 2>/dev/null || return 0
  if firewall-cmd --query-masquerade >/dev/null 2>&1; then
    log "firewalld: masquerade 已启用"
    return 0
  fi
  firewall-cmd --permanent --add-masquerade >/dev/null 2>&1 || true
  firewall-cmd --reload >/dev/null 2>&1 || true
  if firewall-cmd --query-masquerade >/dev/null 2>&1; then
    log "firewalld: 已启用 masquerade"
  else
    warn "firewalld masquerade 未能启用，依赖 iptables NAT 规则"
  fi
}

relax_ufw_forward_policy() {
  # UFW 默认 FORWARD=DROP 会阻断 NAT；仅在启用 UFW 时调整
  command -v ufw >/dev/null 2>&1 || return 0
  ufw status 2>/dev/null | grep -qi "Status: active" || return 0
  local conf="/etc/default/ufw"
  [[ -f "$conf" ]] || return 0
  if grep -qE '^DEFAULT_FORWARD_POLICY="ACCEPT"' "$conf" 2>/dev/null; then
    return 0
  fi
  if grep -qE '^DEFAULT_FORWARD_POLICY=' "$conf" 2>/dev/null; then
    sed -i 's/^DEFAULT_FORWARD_POLICY=.*/DEFAULT_FORWARD_POLICY="ACCEPT"/' "$conf"
  else
    echo 'DEFAULT_FORWARD_POLICY="ACCEPT"' >>"$conf"
  fi
  ufw reload >/dev/null 2>&1 || true
  log "UFW: DEFAULT_FORWARD_POLICY 已设为 ACCEPT（NAT 需要）"
}

write_nat_script() {
  cat >"${INSTALL_PATH}/ET-nat.sh" <<'EOF'
#!/bin/bash
# EasyTier NAT / 出口转发：apply | remove | status
# 由 install.sh 安装；规则带 comment=ET-NAT，可幂等重复执行。
set -euo pipefail

INSTALL_PATH="${INSTALL_PATH:-/opt/easytier}"
NAT_WAN_IFACE="${NAT_WAN_IFACE:-}"
COMMENT="ET-NAT"

log() { echo "[ET-nat] $*"; }
warn() { echo "[ET-nat] WARN: $*" >&2; }
err() { echo "[ET-nat] ERROR: $*" >&2; }

detect_wan() {
  local iface="${NAT_WAN_IFACE:-}"
  if [[ -n "$iface" && -d "/sys/class/net/${iface}" ]]; then
    echo "$iface"
    return 0
  fi
  iface="$(ip -4 route show default 2>/dev/null | awk '/default/{for(i=1;i<=NF;i++) if($i=="dev"){print $(i+1); exit}}')"
  if [[ -z "$iface" ]]; then
    iface="$(ip -6 route show default 2>/dev/null | awk '/default/{for(i=1;i<=NF;i++) if($i=="dev"){print $(i+1); exit}}')"
  fi
  [[ -n "$iface" && -d "/sys/class/net/${iface}" ]] || return 1
  echo "$iface"
}

ensure_forward() {
  sysctl -w net.ipv4.ip_forward=1 >/dev/null
  sysctl -w net.ipv6.conf.all.forwarding=1 >/dev/null 2>/dev/null || true
}

ipt_add() {
  local table="$1"; shift
  if [[ "$table" == "filter" ]]; then
    iptables -C "$@" >/dev/null 2>&1 || iptables -A "$@"
  else
    iptables -t "$table" -C "$@" >/dev/null 2>&1 || iptables -t "$table" -A "$@"
  fi
}

ipt_del() {
  local table="$1"; shift
  if [[ "$table" == "filter" ]]; then
    while iptables -C "$@" >/dev/null 2>&1; do iptables -D "$@" || break; done
  else
    while iptables -t "$table" -C "$@" >/dev/null 2>&1; do iptables -t "$table" -D "$@" || break; done
  fi
}

cmd_apply() {
  command -v iptables >/dev/null 2>&1 || { err "缺少 iptables"; exit 1; }
  command -v ip >/dev/null 2>&1 || { err "缺少 ip (iproute2)"; exit 1; }
  local wan
  wan="$(detect_wan)" || { err "无法检测 WAN 网卡"; exit 1; }
  ensure_forward
  ipt_add filter FORWARD -m conntrack --ctstate RELATED,ESTABLISHED -m comment --comment "$COMMENT" -j ACCEPT
  ipt_add filter FORWARD -o "$wan" -m comment --comment "$COMMENT" -j ACCEPT
  ipt_add nat POSTROUTING -o "$wan" -m comment --comment "$COMMENT" -j MASQUERADE
  printf '%s\n' "$wan" >"${INSTALL_PATH}/.nat-wan"
  log "NAT 已应用: WAN=${wan} MASQUERADE + FORWARD"
}

cmd_remove() {
  command -v iptables >/dev/null 2>&1 || exit 0
  local wan=""
  [[ -f "${INSTALL_PATH}/.nat-wan" ]] && wan="$(tr -d '[:space:]' <"${INSTALL_PATH}/.nat-wan" || true)"
  [[ -n "$wan" ]] || wan="$(detect_wan || true)"
  if [[ -n "$wan" ]]; then
    ipt_del nat POSTROUTING -o "$wan" -m comment --comment "$COMMENT" -j MASQUERADE
    ipt_del filter FORWARD -o "$wan" -m comment --comment "$COMMENT" -j ACCEPT
  fi
  ipt_del filter FORWARD -m conntrack --ctstate RELATED,ESTABLISHED -m comment --comment "$COMMENT" -j ACCEPT
  rm -f "${INSTALL_PATH}/.nat-wan"
  log "NAT 规则已移除"
}

cmd_status() {
  local wan="" fwd="0"
  [[ -f "${INSTALL_PATH}/.nat-wan" ]] && wan="$(tr -d '[:space:]' <"${INSTALL_PATH}/.nat-wan" || true)"
  [[ -n "$wan" ]] || wan="$(detect_wan || true)"
  fwd="$(sysctl -n net.ipv4.ip_forward 2>/dev/null || echo 0)"
  echo "wan=${wan:-unknown}"
  echo "ip_forward=${fwd}"
  if [[ -n "$wan" ]] && command -v iptables >/dev/null 2>&1 && \
     iptables -t nat -C POSTROUTING -o "$wan" -m comment --comment "$COMMENT" -j MASQUERADE >/dev/null 2>&1; then
    echo "masquerade=yes"
    exit 0
  fi
  echo "masquerade=no"
  exit 1
}

case "${1:-}" in
  apply) cmd_apply ;;
  remove) cmd_remove ;;
  status) cmd_status ;;
  *) echo "Usage: $0 apply|remove|status" >&2; exit 1 ;;
esac
EOF
  chmod 755 "${INSTALL_PATH}/ET-nat.sh"
}

setup_nat_forwarding() {
  [[ "$ENABLE_NAT" == "yes" ]] || return 0

  command -v ip >/dev/null 2>&1 || {
    err "开启 NAT 需要 iproute2（ip 命令）"
    return 1
  }
  if ! command -v iptables >/dev/null 2>&1; then
    if [[ "$INSTALL_DEPS" == "yes" ]] && command -v apt-get >/dev/null 2>&1; then
      log "安装依赖: iptables"
      export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq
      apt-get install -y iptables
    fi
  fi
  command -v iptables >/dev/null 2>&1 || {
    err "开启 NAT 需要 iptables"
    return 1
  }

  local wan
  wan="$(resolve_nat_wan_iface)" || return 1
  NAT_WAN_IFACE="$wan"
  log "NAT WAN 网卡: ${wan}"

  enable_kernel_ip_forward || return 1
  write_nat_script

  cat >/etc/systemd/system/ET-nat.service <<EOF
[Unit]
Description=EasyTier NAT / exit-node forwarding
After=network-online.target
Wants=network-online.target
Before=ET-core@default.service ET-core@node0.service

[Service]
Type=oneshot
RemainAfterExit=yes
Environment=INSTALL_PATH=${INSTALL_PATH}
Environment=NAT_WAN_IFACE=${NAT_WAN_IFACE}
ExecStart=${INSTALL_PATH}/ET-nat.sh apply
ExecStop=${INSTALL_PATH}/ET-nat.sh remove

[Install]
WantedBy=multi-user.target
EOF

  systemctl daemon-reload
  systemctl enable ET-nat.service
  if ! systemctl restart ET-nat.service; then
    err "ET-nat.service 启动失败"
    systemctl status ET-nat.service --no-pager 2>/dev/null || true
    return 1
  fi

  apply_firewalld_masquerade
  relax_ufw_forward_policy

  if ! "${INSTALL_PATH}/ET-nat.sh" status >/dev/null 2>&1; then
    err "NAT 规则校验失败"
    return 1
  fi
  log "NAT/出口转发已启用并持久化（ET-nat.service）"
}

remove_nat_forwarding() {
  if [[ -x "${INSTALL_PATH}/ET-nat.sh" ]]; then
    "${INSTALL_PATH}/ET-nat.sh" remove 2>/dev/null || true
  fi
  systemctl stop ET-nat.service 2>/dev/null || true
  systemctl disable ET-nat.service 2>/dev/null || true
  rm -f /etc/systemd/system/ET-nat.service
  disable_kernel_ip_forward_dropin
}

verify_nat_forwarding() {
  # 返回 0=ok；未启用时跳过
  local enabled="no"
  if [[ -f "${INSTALL_PATH}/install-options.env" ]]; then
    enabled="$(read_install_option ENABLE_NAT 2>/dev/null || true)"
  fi
  if systemctl is-enabled ET-nat.service &>/dev/null || systemctl is-active --quiet ET-nat.service 2>/dev/null; then
    enabled="yes"
  fi
  [[ "$enabled" == "yes" || "$ENABLE_NAT" == "yes" ]] || return 0

  local fail=0
  local fwd
  fwd="$(sysctl -n net.ipv4.ip_forward 2>/dev/null || echo 0)"
  if [[ "$fwd" == "1" ]]; then
    health_print ok "IP 转发" "net.ipv4.ip_forward=1"
  else
    health_print fail "IP 转发" "net.ipv4.ip_forward=${fwd}（期望 1）"
    fail=1
  fi

  if systemctl is-active --quiet ET-nat.service 2>/dev/null; then
    health_print ok "NAT 服务" "ET-nat.service 已激活"
  else
    health_print fail "NAT 服务" "ET-nat.service 未激活"
    fail=1
  fi

  if [[ -x "${INSTALL_PATH}/ET-nat.sh" ]] && "${INSTALL_PATH}/ET-nat.sh" status >/dev/null 2>&1; then
    local st wan
    st="$("${INSTALL_PATH}/ET-nat.sh" status 2>/dev/null || true)"
    wan="$(printf '%s\n' "$st" | awk -F= '/^wan=/{print $2}')"
    health_print ok "MASQUERADE" "WAN=${wan:-?} 规则存在"
  else
    health_print fail "MASQUERADE" "未检测到 ET-NAT 规则"
    fail=1
  fi

  return "$fail"
}

prompt_nat_forwarding() {
  [[ "$AUTO" == "true" ]] && return 0
  [[ ! -t 0 ]] && return 0
  # 已通过 CLI 明确指定则不再询问
  [[ "${NAT_PROMPT_DONE:-}" == "true" ]] && return 0

  echo
  echo "NAT / 出口节点转发:"
  echo "  开启后将: 1) 内核 ip_forward  2) iptables MASQUERADE"
  echo "            3) EasyTier enable_exit_node + proxy_forward_by_system"
  echo "  适用：本机作为其他节点的上网出口（VPS 出口机）"
  read -rp "是否开启 NAT/出口转发? [y/N]: " yn
  if [[ "$yn" =~ ^[Yy]$ ]]; then
    ENABLE_NAT="yes"
    local auto_wan=""
    auto_wan="$(detect_default_wan_iface || true)"
    if [[ -n "$auto_wan" ]]; then
      read -rp "WAN 网卡 [${auto_wan}，回车=自动]: " p
      [[ -n "$p" ]] && NAT_WAN_IFACE="$p"
    else
      read -rp "WAN 网卡（必填，如 eth0/ens3）: " p
      [[ -n "$p" ]] || { err "开启 NAT 须指定 WAN 网卡"; exit 1; }
      NAT_WAN_IFACE="$p"
    fi
  else
    ENABLE_NAT="no"
  fi
  NAT_PROMPT_DONE=true
}

nat_status_line() {
  if [[ "$ENABLE_NAT" == "yes" ]]; then
    echo "NAT/出口转发: 已开启（WAN=${NAT_WAN_IFACE:-auto}，enable_exit_node）"
  else
    echo "NAT/出口转发: 未开启"
  fi
}

md5_hex() {
  if command -v md5sum >/dev/null 2>&1; then
    echo -n "$1" | md5sum | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    echo -n "$1" | openssl md5 -r | awk '{print $1}'
  else
    err "设置管理员密码需要 md5sum 或 openssl"
    return 1
  fi
}

validate_admin_password() {
  local pass="$1"
  [[ ${#pass} -ge 8 ]] || { err "管理员密码至少 8 位"; return 1; }
}

generate_strong_password() {
  local pass=""
  if command -v openssl >/dev/null 2>&1; then
    pass="$(openssl rand -base64 32 | tr -d '/+=' | head -c 20)"
  else
    pass="$(tr -dc 'A-Za-z0-9' </dev/urandom | head -c 20)"
  fi
  [[ ${#pass} -ge 8 ]] || pass="${pass}Et$(date +%s | tail -c 4)"
  echo "$pass"
}

validate_server_ports() {
  [[ "$MODE" == "server" ]] || return 0
  if [[ "$API_PORT" == "$CONFIG_PORT" ]]; then
    case "$CONFIG_PROTOCOL" in
      tcp|ws|wss)
        err "Web/API（TCP ${API_PORT}）与配置下发（${CONFIG_PROTOCOL} ${CONFIG_PORT}）同端口会冲突，请改端口或配置协议改为 udp"
        exit 1
        ;;
    esac
  fi
}

apply_defaults() {
  BACKUP_DIR="${BACKUP_DIR:-${INSTALL_PATH}/backups}"
  apply_config_token_alias
  case "$(printf '%s' "$ENABLE_NAT" | tr '[:upper:]' '[:lower:]')" in
    yes|y|true|1) ENABLE_NAT="yes" ;;
    no|n|false|0|"") ENABLE_NAT="no" ;;
    *) err "无效 ENABLE_NAT: ${ENABLE_NAT}（须 yes/no）"; exit 1 ;;
  esac
  validate_port "API" "$API_PORT"
  validate_port "配置下发" "$CONFIG_PORT"
  validate_port "节点" "$CORE_PORT"
  if [[ "$MODE" == "server" && "$NGINX_HTTPS_PROXY" == "yes" ]]; then
    validate_port "HTTPS" "$NGINX_SSL_PORT"
  fi
  validate_backup_hour
  validate_server_ports
  validate_config_token "$CONFIG_TOKEN" "接入 Token" || exit 1
  if [[ "$MODE" == "server" ]]; then
    PUBLIC_HOST="$(normalize_host_input "$PUBLIC_HOST")"
    validate_required_host "控制台域名" "$PUBLIC_HOST" || exit 1
    if [[ -z "$ADMIN_PASSWORD" ]]; then
      ADMIN_PASSWORD="$(generate_strong_password)"
    fi
    validate_admin_password "$ADMIN_PASSWORD"
  elif [[ "$MODE" == "client" ]]; then
    # Allow pasting a full config-server URL into --server-host
    if [[ "$SERVER_HOST" =~ ^(udp|tcp|ws|wss):// ]]; then
      parse_config_server_url "$SERVER_HOST"
      if [[ -n "$CS_HOST" ]]; then SERVER_HOST="$CS_HOST"; fi
      if [[ -n "$CS_PORT" ]]; then CONFIG_PORT="$CS_PORT"; fi
      if [[ -n "$CS_SCHEME" ]]; then CONFIG_PROTOCOL="$CS_SCHEME"; fi
      if [[ -n "$CS_TOKEN" ]]; then CONFIG_TOKEN="$CS_TOKEN"; fi
    else
      # Client dial URL must be single-scheme; collapse listen lists like udp,tcp
      CONFIG_PROTOCOL="$(primary_config_scheme)"
    fi
    SERVER_HOST="$(normalize_host_input "$SERVER_HOST")"
    validate_required_host "控制台地址" "$SERVER_HOST" || exit 1
    validate_config_token "$CONFIG_TOKEN" "接入 Token" || exit 1
  fi
  if [[ "$ENABLE_NAT" == "yes" && -n "${NAT_WAN_IFACE:-}" ]]; then
    [[ -d "/sys/class/net/${NAT_WAN_IFACE}" ]] || {
      err "NAT WAN 网卡不存在: ${NAT_WAN_IFACE}"
      exit 1
    }
  fi
  return 0
}

print_admin_credentials() {
  local api_url
  api_url="$(api_host_url "$PUBLIC_HOST")"
  echo
  echo -e "${YELLOW}========================================${NC}"
  echo -e "${YELLOW} EasyTier 管理员凭据（请妥善保存）${NC}"
  echo -e "${YELLOW}========================================${NC}"
  echo -e "  用户名: ${GREEN}${DEFAULT_ADMIN_USER}${NC}"
  echo -e "  密码:   ${GREEN}${ADMIN_PASSWORD}${NC}"
  echo -e "  控制台: ${GREEN}${api_url}${NC}"
  echo -e "${YELLOW}========================================${NC}"
  echo
}

save_admin_credentials() {
  local api_url
  api_url="$(api_host_url "$PUBLIC_HOST")"
  {
    printf 'username=%s\n' "$DEFAULT_ADMIN_USER"
    printf 'password=%s\n' "$ADMIN_PASSWORD"
    printf 'console=%s\n' "$api_url"
    printf 'generated_at=%s\n' "$(date -Iseconds 2>/dev/null || date)"
  } >"${INSTALL_PATH}/admin-credentials.txt"
  chmod 600 "${INSTALL_PATH}/admin-credentials.txt"
  log "凭据已写入 ${INSTALL_PATH}/admin-credentials.txt（权限 600）"
}

resolve_backup_dir() {
  BACKUP_DIR="${BACKUP_DIR:-${INSTALL_PATH}/backups}"
}

write_backup_script() {
  cat >"${INSTALL_PATH}/ET-backup.sh" <<'EOF'
#!/bin/bash
# EasyTier 数据库定时/手动备份脚本
set -euo pipefail

INSTALL_PATH="${INSTALL_PATH:-/opt/easytier}"
BACKUP_DIR="${BACKUP_DIR:-${INSTALL_PATH}/backups}"
DB_PATH="${INSTALL_PATH}/et.db"
RETENTION_DAYS="${BACKUP_RETENTION_DAYS:-30}"
CRED_FILE="${INSTALL_PATH}/admin-credentials.txt"

log() { echo "[ET-backup] $*"; }
warn() { echo "[ET-backup] WARN: $*" >&2; }

mkdir -p "$BACKUP_DIR"
chmod 700 "$BACKUP_DIR" 2>/dev/null || true

if [[ ! -f "$DB_PATH" ]]; then
  warn "数据库不存在: $DB_PATH（跳过备份）"
  exit 0
fi

STAMP="$(date +%Y%m%d-%H%M%S)"
ARCHIVE="${BACKUP_DIR}/easytier-db-${STAMP}.tar.gz"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

DB_BACKUP="${TMP_DIR}/et.db"
if command -v sqlite3 >/dev/null 2>&1; then
  sqlite3 "$DB_PATH" ".backup '${DB_BACKUP}'"
else
  warn "未安装 sqlite3，使用文件复制（建议安装 sqlite3 以获得在线热备）"
  cp -a "$DB_PATH" "$DB_BACKUP"
  for ext in -wal -shm; do
    [[ -f "${DB_PATH}${ext}" ]] && cp -a "${DB_PATH}${ext}" "${TMP_DIR}/et.db${ext}" || true
  done
fi

if [[ -f "$CRED_FILE" ]]; then
  cp -a "$CRED_FILE" "${TMP_DIR}/admin-credentials.txt"
fi

tar -czf "$ARCHIVE" -C "$TMP_DIR" .
chmod 600 "$ARCHIVE"
log "已备份: $ARCHIVE ($(du -h "$ARCHIVE" | awk '{print $1}'))"

if [[ "$RETENTION_DAYS" =~ ^[0-9]+$ ]] && [[ "$RETENTION_DAYS" -gt 0 ]]; then
  find "$BACKUP_DIR" -maxdepth 1 -type f -name 'easytier-db-*.tar.gz' -mtime +"$RETENTION_DAYS" -delete
fi
EOF
  chmod 755 "${INSTALL_PATH}/ET-backup.sh"
}

setup_daily_backup() {
  resolve_backup_dir
  write_backup_script
  mkdir -p "$BACKUP_DIR"
  chmod 700 "$BACKUP_DIR"

  cat >/etc/systemd/system/ET-backup.service <<EOF
[Unit]
Description=EasyTier database backup
After=ET-web.service

[Service]
Type=oneshot
Environment=INSTALL_PATH=${INSTALL_PATH}
Environment=BACKUP_DIR=${BACKUP_DIR}
Environment=BACKUP_RETENTION_DAYS=${BACKUP_RETENTION_DAYS}
ExecStart=${INSTALL_PATH}/ET-backup.sh
EOF

  cat >/etc/systemd/system/ET-backup.timer <<EOF
[Unit]
Description=Daily EasyTier database backup

[Timer]
OnCalendar=*-*-* ${BACKUP_HOUR}:00:00
Persistent=true
RandomizedDelaySec=300

[Install]
WantedBy=timers.target
EOF

  systemctl daemon-reload
  systemctl enable ET-backup.timer
  systemctl start ET-backup.timer
  log "每日备份已启用: ${BACKUP_DIR}（每天 ${BACKUP_HOUR}:00，保留 ${BACKUP_RETENTION_DAYS} 天）"
}

remove_daily_backup() {
  systemctl stop ET-backup.timer 2>/dev/null || true
  systemctl disable ET-backup.timer 2>/dev/null || true
  systemctl stop easytier-backup.timer 2>/dev/null || true
  systemctl disable easytier-backup.timer 2>/dev/null || true
  rm -f /etc/systemd/system/ET-backup.service
  rm -f /etc/systemd/system/ET-backup.timer
  rm -f /etc/systemd/system/easytier-backup.service
  rm -f /etc/systemd/system/easytier-backup.timer
  systemctl daemon-reload 2>/dev/null || true
}

run_backup_now() {
  resolve_backup_dir
  if [[ ! -x "${INSTALL_PATH}/ET-backup.sh" ]]; then
    write_backup_script
  fi
  INSTALL_PATH="$INSTALL_PATH" BACKUP_DIR="$BACKUP_DIR" \
    BACKUP_RETENTION_DAYS="$BACKUP_RETENTION_DAYS" \
    "${INSTALL_PATH}/ET-backup.sh"
}

print_service_boot_check() {
  local unit="$1"
  local label="$2"
  local enabled active
  enabled="$(systemctl is-enabled "$unit" 2>/dev/null || echo "missing")"
  active="$(systemctl is-active "$unit" 2>/dev/null || echo "unknown")"

  local enabled_text active_text
  case "$enabled" in
    enabled) enabled_text="${GREEN}已启用开机自启${NC}" ;;
    disabled) enabled_text="${YELLOW}未启用开机自启${NC}" ;;
    *) enabled_text="${RED}未安装 (${enabled})${NC}" ;;
  esac
  case "$active" in
    active) active_text="${GREEN}运行中${NC}" ;;
    inactive) active_text="${YELLOW}未运行${NC}" ;;
    failed) active_text="${RED}失败${NC}" ;;
    *) active_text="${YELLOW}${active}${NC}" ;;
  esac
  echo -e "  ${label}: ${enabled_text} | ${active_text} (${unit})"
}

verify_boot_services() {
  echo
  log "开机自启与服务状态自检:"
  local found=false

  if [[ -f /etc/systemd/system/ET-web.service ]] || \
     systemctl cat ET-web.service &>/dev/null; then
    found=true
    print_service_boot_check "ET-web.service" "Web 控制台"
    if [[ -f /etc/systemd/system/ET-core@node0.service ]] || \
       systemctl cat ET-core@node0.service &>/dev/null; then
      print_service_boot_check "ET-core@node0.service" "本机节点"
    fi
    if systemctl cat ET-backup.timer &>/dev/null; then
      print_service_boot_check "ET-backup.timer" "每日备份"
    fi
  fi

  if [[ -f /etc/systemd/system/ET-core@default.service ]] || \
     systemctl cat ET-core@default.service &>/dev/null; then
    found=true
    print_service_boot_check "ET-core@default.service" "客户端节点"
  fi

  if [[ "$found" != "true" ]]; then
    warn "未检测到 EasyTier systemd 服务，可能尚未安装"
  fi
  echo
}

_parse_web_exec_start() {
  local exec_line="$1"
  local val
  [[ -n "$exec_line" ]] || return 0
  val="$(printf '%s\n' "$exec_line" | sed -n 's/.*--api-server-port[[:space:]]\+\([0-9]\+\).*/\1/p' | head -1)"
  if [[ -n "$val" ]]; then API_PORT="$val"; fi
  val="$(printf '%s\n' "$exec_line" | sed -n 's/.*--config-server-port[[:space:]]\+\([0-9]\+\).*/\1/p' | head -1)"
  if [[ -n "$val" ]]; then CONFIG_PORT="$val"; fi
  val="$(printf '%s\n' "$exec_line" | sed -n 's/.*--config-server-protocol[[:space:]]\+\([^[:space:]\\]\+\).*/\1/p' | head -1)"
  if [[ -n "$val" ]]; then CONFIG_PROTOCOL="$val"; fi
  val="$(printf '%s\n' "$exec_line" | sed -n 's/.*--api-host[[:space:]]\+"*\([^"[:space:]]*\).*/\1/p' | head -1)"
  if [[ -n "$val" ]]; then
    SYSTEMD_API_HOST="$val"
    reconcile_nginx_mode_from_api_host "$val"
  fi
  return 0
}

_parse_core_config_server() {
  local cs_line="$1"
  local val
  [[ -n "$cs_line" ]] || return 0
  val="$(printf '%s\n' "$cs_line" | sed -n 's/.*--config-server[[:space:]]\+\([^[:space:]]*\).*/\1/p' | head -1)"
  [[ -n "$val" ]] || return 0
  val="${val#\"}"; val="${val%\"}"; val="${val#\'}"; val="${val%\'}"
  parse_config_server_url "$val"
  if [[ -n "$CS_HOST" ]]; then SERVER_HOST="$CS_HOST"; fi
  if [[ -n "$CS_PORT" ]]; then CONFIG_PORT="$CS_PORT"; fi
  if [[ -n "$CS_SCHEME" ]]; then CONFIG_PROTOCOL="$CS_SCHEME"; fi
  if [[ -n "$CS_TOKEN" ]]; then CONFIG_TOKEN="$CS_TOKEN"; fi
  return 0
}

load_runtime_config_from_systemd() {
  # Set by _parse_web_exec_start (must not be local — helper writes this name)
  SYSTEMD_API_HOST=""
  local exec_line cs_line
  # Prefer ET-* units; fall back to legacy easytier-* for in-place rename migration
  if systemctl cat ET-web.service &>/dev/null; then
    exec_line="$(systemctl show ET-web.service -p ExecStart --value 2>/dev/null || true)"
    _parse_web_exec_start "$exec_line"
  elif systemctl cat easytier-web.service &>/dev/null; then
    exec_line="$(systemctl show easytier-web.service -p ExecStart --value 2>/dev/null || true)"
    _parse_web_exec_start "$exec_line"
  fi

  load_install_options

  # systemd --api-host 协议为准，覆盖 install-options.env 中的 NGINX_HTTPS_PROXY
  if [[ -n "${SYSTEMD_API_HOST:-}" ]]; then
    reconcile_nginx_mode_from_api_host "$SYSTEMD_API_HOST"
  else
    PUBLIC_HOST="$(normalize_host_input "${PUBLIC_HOST:-}")"
  fi

  if systemctl cat ET-core@default.service &>/dev/null; then
    cs_line="$(systemctl show ET-core@default.service -p ExecStart --value 2>/dev/null || true)"
    _parse_core_config_server "$cs_line"
  elif systemctl cat easytier-core@default.service &>/dev/null; then
    cs_line="$(systemctl show easytier-core@default.service -p ExecStart --value 2>/dev/null || true)"
    _parse_core_config_server "$cs_line"
  fi

  BACKUP_DIR="${BACKUP_DIR:-${INSTALL_PATH}/backups}"
  return 0
}

is_server_installed() {
  systemctl cat ET-web.service &>/dev/null || \
    [[ -f /etc/systemd/system/ET-web.service ]] || \
    systemctl cat easytier-web.service &>/dev/null || \
    [[ -f /etc/systemd/system/easytier-web.service ]]
}

unit_present() {
  local unit="$1"
  systemctl cat "$unit" &>/dev/null || [[ -f "/etc/systemd/system/${unit}" ]]
}

stop_easytier_services() {
  systemctl stop ET-web.service 2>/dev/null || true
  systemctl stop ET-core@node0.service 2>/dev/null || true
  systemctl stop ET-core@default.service 2>/dev/null || true
  stop_legacy_easytier_services
}

start_easytier_services() {
  if systemctl is-enabled ET-web.service &>/dev/null; then
    systemctl start ET-web.service 2>/dev/null || true
  fi
  if systemctl is-enabled ET-core@node0.service &>/dev/null; then
    systemctl start ET-core@node0.service 2>/dev/null || true
  fi
  if systemctl is-enabled ET-core@default.service &>/dev/null; then
    systemctl start ET-core@default.service 2>/dev/null || true
  fi
}

restart_easytier_services() {
  if systemctl is-enabled ET-web.service &>/dev/null; then
    systemctl restart ET-web.service 2>/dev/null || true
  fi
  if systemctl is-enabled ET-core@node0.service &>/dev/null; then
    systemctl restart ET-core@node0.service 2>/dev/null || true
  fi
  if systemctl is-enabled ET-core@default.service &>/dev/null; then
    systemctl restart ET-core@default.service 2>/dev/null || true
  fi
}

health_print() {
  local status="$1"
  local label="$2"
  local detail="$3"
  if [[ "$status" == "ok" ]]; then
    echo -e "  ${GREEN}✓${NC} ${label}: ${detail}"
  else
    echo -e "  ${RED}✗${NC} ${label}: ${detail}"
  fi
}

port_is_listening() {
  local port="$1"
  local proto="$2"
  if command -v ss >/dev/null 2>&1; then
    if [[ "$proto" == t ]]; then
      ss -H -ltn 2>/dev/null | grep -qE ":${port}([^0-9]|$)"
    else
      ss -H -lun 2>/dev/null | grep -qE ":${port}([^0-9]|$)"
    fi
    return
  fi
  if command -v netstat >/dev/null 2>&1; then
    if [[ "$proto" == t ]]; then
      netstat -ltn 2>/dev/null | grep -qE ":${port}([^0-9]|$)"
    else
      netstat -lun 2>/dev/null | grep -qE ":${port}([^0-9]|$)"
    fi
    return
  fi
  return 1
}

resolve_dns() {
  local host="$1"
  local result=""
  host="$(normalize_host_input "$host")"
  [[ -n "$host" ]] || return 1

  # Literal IPs need no lookup
  if [[ "$host" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] || [[ "$host" == *:* ]]; then
    echo "$host"
    return 0
  fi

  if command -v getent >/dev/null 2>&1; then
    result="$(getent ahosts "$host" 2>/dev/null | awk '{print $1}' | head -1 || true)"
    [[ -n "$result" ]] && { echo "$result"; return 0; }
  fi
  if command -v dig >/dev/null 2>&1; then
    result="$(dig +short A "$host" 2>/dev/null | grep -E '^[0-9.]+$' | head -1 || true)"
    if [[ -z "$result" ]]; then
      result="$(dig +short AAAA "$host" 2>/dev/null | grep -E ':' | head -1 || true)"
    fi
    [[ -n "$result" ]] && { echo "$result"; return 0; }
  fi
  if command -v host >/dev/null 2>&1; then
    result="$(host "$host" 2>/dev/null | awk '/has address|has IPv6/{print $NF; exit}' || true)"
    [[ -n "$result" ]] && { echo "$result"; return 0; }
  fi
  return 1
}

run_health_check() {
  load_runtime_config_from_systemd
  # systemd --api-host 协议优先于 install-options.env 中的 NGINX_HTTPS_PROXY

  if ! is_server_installed && \
     ! systemctl cat ET-core@default.service &>/dev/null && \
     ! systemctl cat ET-core@node0.service &>/dev/null; then
    warn "未检测到 EasyTier 服务，请先 install"
    return 1
  fi

  echo
  log "健康检查:"
  local fail=0

  if is_server_installed; then
    # captcha 路由可能已移除；根路径 / 或 login 任一返回业务码即视为 API 存活
    local api_ok=no api_url="" code="000"
    for api_url in \
      "http://127.0.0.1:${API_PORT}/" \
      "http://127.0.0.1:${API_PORT}/api/v1/auth/login" \
      "http://127.0.0.1:${API_PORT}/api/v1/auth/captcha"; do
      # 不用 curl -f：401/404/405 也说明服务在听
      code="$(curl -sS -o /dev/null -w '%{http_code}' --connect-timeout 5 "$api_url" 2>/dev/null || echo 000)"
      if [[ "$code" =~ ^(200|204|301|302|401|403|404|405)$ ]]; then
        api_ok=yes
        break
      fi
    done
    if [[ "$api_ok" == "yes" ]]; then
      health_print ok "Web API" "本机 http://127.0.0.1:${API_PORT}/ 可访问（HTTP ${code}）"
    else
      health_print fail "Web API" "本机 http://127.0.0.1:${API_PORT}/ 不可访问"
      fail=1
    fi

    if port_is_listening "$API_PORT" t; then
      if [[ "$NGINX_HTTPS_PROXY" == "yes" ]]; then
        health_print ok "本机 Web" "TCP ${API_PORT} 监听（Nginx 反代后端）"
      else
        health_print ok "端口监听" "TCP ${API_PORT} 正在监听"
      fi
    else
      health_print fail "端口监听" "TCP ${API_PORT} 未监听"
      fail=1
    fi

    local proto need_udp=0 need_tcp=0
    IFS=',' read -ra _protos <<<"$CONFIG_PROTOCOL"
    for proto in "${_protos[@]}"; do
      proto="$(echo "$proto" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
      case "$proto" in
        udp) need_udp=1 ;;
        tcp|ws|wss) need_tcp=1 ;;
      esac
    done
    if (( need_udp )); then
      if port_is_listening "$CONFIG_PORT" u; then
        health_print ok "配置下发 UDP" "UDP ${CONFIG_PORT} 正在监听"
      else
        health_print fail "配置下发 UDP" "UDP ${CONFIG_PORT} 未监听"
        fail=1
      fi
    fi
    if (( need_tcp )); then
      if port_is_listening "$CONFIG_PORT" t; then
        health_print ok "配置下发 TCP" "TCP ${CONFIG_PORT} 正在监听"
      else
        health_print fail "配置下发 TCP" "TCP ${CONFIG_PORT} 未监听"
        fail=1
      fi
    fi
    if (( !need_udp && !need_tcp )); then
      health_print fail "配置下发" "未知协议: ${CONFIG_PROTOCOL}"
      fail=1
    fi

    local domain
    domain="$(normalize_host_input "${PUBLIC_HOST:-}")"
    if [[ -n "$domain" ]]; then
      local resolved
      resolved="$(resolve_dns "$domain" || true)"
      if [[ -n "$resolved" ]]; then
        health_print ok "DNS 解析" "${domain} -> ${resolved}"
      else
        health_print fail "DNS 解析" "${domain} 无法解析（请检查 DNS / NAT）"
        fail=1
      fi
    else
      health_print fail "DNS 解析" "未配置控制台域名"
      fail=1
    fi

    if systemctl is-active --quiet ET-web.service 2>/dev/null; then
      health_print ok "服务状态" "ET-web.service 运行中"
    else
      health_print fail "服务状态" "ET-web.service 未运行"
      fail=1
    fi

    if [[ "$NGINX_HTTPS_PROXY" == "yes" ]]; then
      verify_nginx_https_proxy yes || fail=1
    fi
  fi

  # 仅当 default 实例已 enable，或（无 web / 无 node0 的纯 client）时检查 @default
  # server+node0 场景常残留 disabled 的 @default 单元，不应判失败
  if systemctl is-enabled ET-core@default.service &>/dev/null || \
     { ! is_server_installed && ! systemctl is-enabled ET-core@node0.service &>/dev/null && \
       (systemctl cat ET-core@default.service &>/dev/null || [[ -f /etc/systemd/system/ET-core@.service ]]); }; then
    if systemctl is-active --quiet ET-core@default.service 2>/dev/null; then
      health_print ok "客户端节点" "ET-core@default 运行中"
    else
      health_print fail "客户端节点" "ET-core@default 未运行"
      fail=1
    fi
    local cs_host
    cs_host="$(normalize_host_input "${SERVER_HOST:-}")"
    if [[ -n "$cs_host" && "$cs_host" != "127.0.0.1" && "$cs_host" != "localhost" ]]; then
      local cs_resolved
      cs_resolved="$(resolve_dns "$cs_host" || true)"
      if [[ -n "$cs_resolved" ]]; then
        health_print ok "控制台 DNS" "${cs_host} -> ${cs_resolved}"
      else
        health_print fail "控制台 DNS" "${cs_host} 无法解析"
        fail=1
      fi
    elif [[ -n "$cs_host" ]]; then
      health_print ok "控制台地址" "${cs_host}（本机）"
    else
      health_print fail "控制台 DNS" "未配置控制台地址"
      fail=1
    fi
  fi

  if systemctl cat ET-core@node0.service &>/dev/null; then
    if systemctl is-active --quiet ET-core@node0.service 2>/dev/null; then
      health_print ok "本机节点" "ET-core@node0 运行中"
    else
      health_print fail "本机节点" "ET-core@node0 未运行"
      fail=1
    fi
  fi

  if ! verify_nat_forwarding; then
    fail=1
  fi

  echo
  if [[ "$fail" -eq 0 ]]; then
    log "健康检查通过"
  else
    warn "健康检查存在失败项，请根据上方提示排查"
  fi
  echo
  return "$fail"
}

resolve_tag_from_latest_redirect() {
  # 跟随 https://github.com/<repo>/releases/latest -> /tag/<tag>
  local loc
  loc="$(curl -fsSLI --connect-timeout 12 --max-time 30 "$GITHUB_RELEASES_LATEST" 2>/dev/null \
    | tr -d '\r' \
    | awk 'BEGIN{IGNORECASE=1} /^location:/ {print $2; exit}')"
  if [[ -z "$loc" ]]; then
    return 1
  fi
  printf '%s' "$loc" | sed -n 's#.*/tag/\([^/[:space:]]*\).*#\1#p'
}

get_latest_release_tag() {
  local json tag
  log "查询最新 Release: ${RELEASES_PAGE}"
  if json="$(fetch_json_with_fallback "$GITHUB_API_LATEST" 2>/dev/null)"; then
    tag="$(printf '%s' "$json" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"$/\1/')"
    tag="$(printf '%s' "$tag" | tr -d '[:space:]')"
    if [[ -n "$tag" ]]; then
      echo "$tag"
      return 0
    fi
  fi
  warn "GitHub API 不可用，尝试跟随 releases/latest 重定向..."
  tag="$(resolve_tag_from_latest_redirect || true)"
  tag="$(printf '%s' "$tag" | tr -d '[:space:]')"
  [[ -n "$tag" ]] || {
    err "无法获取最新 release（请检查 ${RELEASES_PAGE}）"
    return 1
  }
  echo "$tag"
}

verify_binary_bundle() {
  local inner="$1"
  [[ -x "${inner}/ET-core" && -x "${inner}/ET-cli" ]] || {
    err "release 包缺少 ET-core 或 ET-cli"
    return 1
  }
}

download_and_install_binaries() {
  local arch="$1"
  local tag="${2:-}"
  [[ -n "$tag" ]] || tag="$(get_latest_release_tag)"

  # 包名：ET-linux-<arch>-<tag>.zip（例: ET-linux-x86_64-v2.7.49.zip）
  # 优先 tag 直链，失败再试 releases/latest/download（同仓库最新）。
  local asset="ET-linux-${arch}-${tag}.zip"
  local base_tag="${RELEASES_PAGE}/download/${tag}/${asset}"
  local base_latest="${RELEASES_PAGE}/latest/download/${asset}"

  log "准备下载 ${tag} (${arch}) from ${RELEASES_PAGE} ..."
  mkdir -p "$INSTALL_PATH"
  rm -rf /tmp/easytier-install-extract /tmp/easytier-install.zip
  if ! fetch_file_with_fallback "$base_tag" /tmp/easytier-install.zip; then
    warn "按 tag 下载失败，改试 latest/download ..."
    if ! fetch_file_with_fallback "$base_latest" /tmp/easytier-install.zip; then
      err "无法从 ${RELEASES_PAGE} 下载 ${asset}（请检查网络或更换下载源）"
      return 1
    fi
  fi
  unzip -oq /tmp/easytier-install.zip -d /tmp/easytier-install-extract
  # Release zip 通常含 ET-linux-<arch>/；Actions 产物解压后也可能是扁平目录
  local inner=""
  local candidate
  for candidate in \
    "/tmp/easytier-install-extract/ET-linux-${arch}" \
    "/tmp/easytier-install-extract"; do
    if [[ -x "${candidate}/ET-core" && -x "${candidate}/ET-cli" ]]; then
      inner="$candidate"
      break
    fi
  done
  if [[ -z "$inner" ]]; then
    candidate="$(find /tmp/easytier-install-extract -maxdepth 2 -type f -name ET-core 2>/dev/null | head -1 || true)"
    if [[ -n "$candidate" ]]; then
      inner="$(dirname "$candidate")"
    fi
  fi
  [[ -n "$inner" ]] || {
    err "解压后未找到 ET-core（期望 ET-linux-${arch}/ 或包根目录）"
    return 1
  }
  verify_binary_bundle "$inner"

  install -m 755 "$inner/ET-core" "${INSTALL_PATH}/ET-core"
  install -m 755 "$inner/ET-cli" "${INSTALL_PATH}/ET-cli"
  if [[ -f "$inner/ET-web-embed" ]]; then
    install -m 755 "$inner/ET-web-embed" "${INSTALL_PATH}/ET-web-embed"
  fi
  if [[ -f "$inner/ET-web" ]]; then
    install -m 755 "$inner/ET-web" "${INSTALL_PATH}/ET-web"
  fi
  rm -rf /tmp/easytier-install.zip /tmp/easytier-install-extract
  mkdir -p "$CONFIG_DIR"
  ln -sfn "${INSTALL_PATH}/ET-core" /usr/sbin/ET-core 2>/dev/null || true
  ln -sfn "${INSTALL_PATH}/ET-cli" /usr/sbin/ET-cli 2>/dev/null || true
  INSTALLED_VERSION="$tag"
  log "二进制已更新到 ${INSTALL_PATH}（版本 ${tag}）"
}

wait_for_web_api() {
  local url="http://127.0.0.1:${API_PORT}/api/v1/auth/captcha"
  local i
  for i in $(seq 1 30); do
    if curl -fsS -o /dev/null "$url" 2>/dev/null; then
      return 0
    fi
    sleep 1
  done
  return 1
}

# 使用内置 admin/admin 登录后改密（不经过注册 API）
# 与当前 Web 前端一致：提交明文密码（服务端 argon2；仍兼容历史 MD5 客户端哈希）。
change_admin_password() {
  local new_pass="$1"
  local cookie_file
  local pass_default_json pass_new_json
  cookie_file="$(mktemp)"
  chmod 600 "$cookie_file"
  pass_default_json="$(json_escape_string "$DEFAULT_ADMIN_PASS")"
  pass_new_json="$(json_escape_string "$new_pass")"

  if ! wait_for_web_api; then
    warn "Web API 未就绪，跳过自动改密。请尽快用 admin/admin 登录并修改密码。"
    rm -f "$cookie_file"
    return 1
  fi

  local api_base="http://127.0.0.1:${API_PORT}/api/v1/auth"
  if ! curl -fsS -c "$cookie_file" -b "$cookie_file" \
    -H 'Content-Type: application/json' \
    -d "{\"username\":\"${DEFAULT_ADMIN_USER}\",\"password\":\"${pass_default_json}\"}" \
    "${api_base}/login" >/dev/null 2>&1; then
    # Fallback: legacy clients that still POST md5(password)
    local md5_default
    md5_default="$(md5_hex "$DEFAULT_ADMIN_PASS" 2>/dev/null || true)"
    if [[ -z "$md5_default" ]] || ! curl -fsS -c "$cookie_file" -b "$cookie_file" \
      -H 'Content-Type: application/json' \
      -d "{\"username\":\"${DEFAULT_ADMIN_USER}\",\"password\":\"${md5_default}\"}" \
      "${api_base}/login" >/dev/null 2>&1; then
      warn "默认 admin 登录失败（可能已改过密码），跳过自动改密"
      rm -f "$cookie_file"
      return 1
    fi
  fi

  if curl -fsS -c "$cookie_file" -b "$cookie_file" \
    -H 'Content-Type: application/json' \
    -X PUT \
    -d "{\"new_password\":\"${pass_new_json}\"}" \
    "${api_base}/password" >/dev/null 2>&1; then
    log "已将默认 admin 密码修改完成"
    rm -f "$cookie_file"
    return 0
  fi

  warn "改密 API 失败，请手动登录 Web 控制台修改密码"
  rm -f "$cookie_file"
  return 1
}

download_easytier() {
  local arch="$1"
  log "获取最新版本..."
  download_and_install_binaries "$arch"
}

api_host_url() {
  local host="$1"
  if [[ "$NGINX_HTTPS_PROXY" == "yes" ]]; then
    local domain
    domain="$(normalize_host_input "$host")"
    echo "https://${domain}"
    return 0
  fi
  if [[ "$host" == http://* || "$host" == https://* ]]; then
    echo "$host"
  else
    echo "http://${host}:${API_PORT}"
  fi
}

save_install_options() {
  {
    printf 'MODE=%s\n' "$MODE"
    printf 'ENABLE_NAT=%s\n' "$ENABLE_NAT"
    printf 'NAT_WAN_IFACE=%s\n' "${NAT_WAN_IFACE:-}"
    if [[ "$MODE" == "server" ]]; then
      printf 'NGINX_HTTPS_PROXY=%s\n' "$NGINX_HTTPS_PROXY"
      printf 'NGINX_SSL_PORT=%s\n' "$NGINX_SSL_PORT"
      printf 'PUBLIC_HOST=%s\n' "$(normalize_host_input "$PUBLIC_HOST")"
      printf 'API_PORT=%s\n' "$API_PORT"
      printf 'CONFIG_PORT=%s\n' "$CONFIG_PORT"
      printf 'CONFIG_PROTOCOL=%s\n' "$CONFIG_PROTOCOL"
      printf 'WITH_NODE=%s\n' "$WITH_NODE"
    elif [[ "$MODE" == "client" ]]; then
      printf 'SERVER_HOST=%s\n' "$(normalize_host_input "$SERVER_HOST")"
      printf 'CONFIG_PORT=%s\n' "$CONFIG_PORT"
      printf 'CONFIG_PROTOCOL=%s\n' "$CONFIG_PROTOCOL"
    fi
  } >"${INSTALL_PATH}/install-options.env"
  chmod 600 "${INSTALL_PATH}/install-options.env"
}

load_install_options() {
  local val
  # 注意：在 set -e 下，函数末尾不能是失败的 `[[ ... ]] && ...`（否则会静默退出）
  val="$(read_install_option MODE || true)"
  if [[ -n "$val" ]]; then MODE="$val"; fi
  val="$(read_install_option WITH_NODE || true)"
  if [[ -n "$val" ]]; then WITH_NODE="$val"; fi
  val="$(read_install_option ENABLE_NAT || true)"
  if [[ -n "$val" ]]; then ENABLE_NAT="$val"; fi
  val="$(read_install_option NAT_WAN_IFACE || true)"
  if [[ -n "$val" ]]; then NAT_WAN_IFACE="$val"; fi
  val="$(read_install_option NGINX_HTTPS_PROXY || true)"
  if [[ -n "$val" ]]; then NGINX_HTTPS_PROXY="$val"; fi
  val="$(read_install_option NGINX_SSL_PORT || true)"
  if [[ -n "$val" ]]; then NGINX_SSL_PORT="$val"; fi
  val="$(read_install_option PUBLIC_HOST || true)"
  if [[ -n "$val" ]]; then PUBLIC_HOST="$val"; fi
  val="$(read_install_option SERVER_HOST || true)"
  if [[ -n "$val" ]]; then SERVER_HOST="$val"; fi
  val="$(read_install_option API_PORT || true)"
  if [[ -n "$val" ]]; then API_PORT="$val"; fi
  val="$(read_install_option CONFIG_PORT || true)"
  if [[ -n "$val" ]]; then CONFIG_PORT="$val"; fi
  val="$(read_install_option CONFIG_PROTOCOL || true)"
  if [[ -n "$val" ]]; then CONFIG_PROTOCOL="$val"; fi
  return 0
}

# 从运行态推断 NAT（install-options / ET-nat / unit 标志）
reconcile_nat_from_runtime() {
  local exec_line=""
  if [[ "$ENABLE_NAT" != "yes" ]]; then
    if systemctl is-enabled ET-nat.service &>/dev/null || \
       systemctl is-active --quiet ET-nat.service 2>/dev/null; then
      ENABLE_NAT="yes"
    fi
  fi
  if [[ "$ENABLE_NAT" != "yes" ]]; then
    exec_line="$(systemctl show ET-core@default.service -p ExecStart --value 2>/dev/null || true)"
    exec_line+=" $(systemctl show ET-core@node0.service -p ExecStart --value 2>/dev/null || true)"
    if [[ -f /etc/systemd/system/ET-core@.service ]]; then
      exec_line+=" $(tr '\n' ' ' </etc/systemd/system/ET-core@.service)"
    fi
    if [[ "$exec_line" == *"--enable-exit-node"* ]]; then
      ENABLE_NAT="yes"
    fi
  fi
  if [[ "$ENABLE_NAT" == "yes" && -z "${NAT_WAN_IFACE:-}" && -f "${INSTALL_PATH}/.nat-wan" ]]; then
    NAT_WAN_IFACE="$(tr -d '[:space:]' <"${INSTALL_PATH}/.nat-wan" || true)"
  fi
  return 0
}

# core 轻量模式：ExecStart 使用 -c config，而非 --config-server
# 优先看运行中的实例 ExecStart，避免残留的 ET-core@.service 模板误判 client
is_core_standalone_install() {
  local exec_line unit_file="/etc/systemd/system/ET-core@.service"
  exec_line="$(systemctl show ET-core@default.service -p ExecStart --value 2>/dev/null || true)"
  if [[ -n "$exec_line" ]]; then
    [[ "$exec_line" == *"--config-server"* ]] && return 1
    if [[ "$exec_line" == *"-c "* || "$exec_line" == *"--config "* ]]; then
      return 0
    fi
    # 实例存在但既不是 -c 也不是 --config-server：保守视为非 standalone
    return 1
  fi
  # 无 default 实例时，再看模板（纯 core 安装）
  if [[ -f "$unit_file" ]]; then
    if grep -q -- '--config-server' "$unit_file" 2>/dev/null; then
      return 1
    fi
    if grep -qE -- '(^|[[:space:]])(-c|--config)[[:space:]]' "$unit_file" 2>/dev/null; then
      return 0
    fi
  fi
  # 最后才信 install-options 的 MODE=core（且没有 web）
  if [[ "${MODE:-}" == "core" ]] && ! is_server_installed; then
    return 0
  fi
  return 1
}

# strict=yes 时检查失败返回 1（healthcheck）；安装时传 no 仅告警
verify_nginx_https_proxy() {
  local strict="${1:-no}"
  [[ "$NGINX_HTTPS_PROXY" == "yes" ]] || return 0

  local domain fails=0
  domain="$(normalize_host_input "$PUBLIC_HOST")"

  echo
  log "Nginx HTTPS 反代检查:"

  if systemctl is-active --quiet nginx 2>/dev/null || pgrep -x nginx >/dev/null 2>&1; then
    health_print ok "Nginx 服务" "运行中"
  else
    health_print fail "Nginx 服务" "未运行（请安装并配置 Nginx）"
    fails=1
  fi

  if port_is_listening "$NGINX_SSL_PORT" t; then
    health_print ok "HTTPS 监听" "TCP ${NGINX_SSL_PORT} 正在监听"
  else
    health_print fail "HTTPS 监听" "TCP ${NGINX_SSL_PORT} 未监听（Nginx 需 listen ${NGINX_SSL_PORT} ssl）"
    fails=1
  fi

  if firewall_is_active; then
    local fw_backend
    fw_backend="$(detect_firewall_backend)"
    case "$fw_backend" in
      ufw)
        if ufw status 2>/dev/null | awk -v p="${NGINX_SSL_PORT}" '$1 == p"/tcp" { found=1 } END { exit !found }'; then
          health_print ok "UFW" "已放行 TCP ${NGINX_SSL_PORT}"
        else
          health_print fail "UFW" "未放行 TCP ${NGINX_SSL_PORT}"
          fails=1
        fi
        ;;
      firewalld)
        if firewalld_port_exists "${NGINX_SSL_PORT}/tcp"; then
          health_print ok "firewalld" "已放行 TCP ${NGINX_SSL_PORT}"
        else
          health_print fail "firewalld" "未放行 TCP ${NGINX_SSL_PORT}"
          fails=1
        fi
        ;;
    esac
  fi

  local pub_base="https://${domain}/"
  local pub_code
  pub_code="$(curl -sS -o /dev/null -w '%{http_code}' --connect-timeout 10 "$pub_base" 2>/dev/null || echo 000)"
  if [[ "$pub_code" =~ ^(200|204|301|302|401|403|404|405)$ ]]; then
    health_print ok "HTTPS 反代" "${pub_base} 可访问（证书校验通过，HTTP ${pub_code}）"
  else
    pub_code="$(curl -sS -k -o /dev/null -w '%{http_code}' --connect-timeout 10 "$pub_base" 2>/dev/null || echo 000)"
    if [[ "$pub_code" =~ ^(200|204|301|302|401|403|404|405)$ ]]; then
      health_print fail "HTTPS 证书" "反代可达但证书校验失败（请检查证书链/域名/SNI）"
      health_print ok "HTTPS 反代" "${pub_base} 可访问（curl -k，HTTP ${pub_code}）"
      fails=1
    else
      health_print fail "HTTPS 反代" "${pub_base} 不可达（检查 Nginx/NAT/证书）"
      fails=1
    fi
  fi

  health_print ok "配置下发" "UDP ${CONFIG_PORT} 仍需对外放行（不经 Nginx）"

  if [[ "$strict" == "yes" && "$fails" -gt 0 ]]; then
    return 1
  fi
  return 0
}

write_web_service() {
  local public_host="$1"
  local api_url
  api_url="$(api_host_url "$public_host")"
  cat >/etc/systemd/system/ET-web.service <<EOF
[Unit]
Description=EasyTier Web Console (ET-web-embed)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=${INSTALL_PATH}
ExecStart=${INSTALL_PATH}/ET-web-embed \\
  --db ${INSTALL_PATH}/et.db \\
  --api-server-port ${API_PORT} \\
  --api-host "${api_url}" \\
  --config-server-port ${CONFIG_PORT} \\
  --config-server-protocol ${CONFIG_PROTOCOL} \\
  --disable-registration
Restart=on-failure
RestartSec=5
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF
}

write_core_service() {
  local config_server="$1"
  local instance="${2:-default}"
  local after_web="${3:-no}"
  local unit_after="After=network-online.target"
  local nat_after=""
  if [[ "$after_web" == "yes" ]]; then
    unit_after="After=network-online.target ET-web.service"
  fi
  if [[ "$ENABLE_NAT" == "yes" ]]; then
    unit_after="${unit_after} ET-nat.service"
    nat_after="Wants=ET-nat.service"
  fi
  local nat_flags=""
  if [[ "$ENABLE_NAT" == "yes" ]]; then
    nat_flags="$(printf '\n  --enable-exit-node \\\n  --proxy-forward-by-system')"
  fi
  # URL is validated (safe charset); quote for systemd word splitting safety.
  cat >/etc/systemd/system/ET-core@${instance}.service <<EOF
[Unit]
Description=EasyTier Core Node (${instance})
${unit_after}
Wants=network-online.target
${nat_after}

[Service]
Type=simple
WorkingDirectory=${INSTALL_PATH}
ExecStart=${INSTALL_PATH}/ET-core --config-server "${config_server}"${nat_flags}
Restart=on-failure
RestartSec=5
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF
}

install_server() {
  [[ -f "$INSTALL_PATH/ET-web-embed" ]] || { err "包内无 ET-web-embed，请检查 release zip"; exit 1; }

  # Stop/disable any pre-rename units so ports are free and only ET-* remain
  stop_legacy_easytier_services
  rm -f /etc/systemd/system/easytier-web.service \
    /etc/systemd/system/easytier-core@node0.service \
    /etc/systemd/system/easytier-core@default.service \
    /etc/systemd/system/easytier-core@.service

  write_web_service "$PUBLIC_HOST"

  systemctl daemon-reload
  systemctl enable ET-web.service
  systemctl restart ET-web.service

  if change_admin_password "$ADMIN_PASSWORD"; then
    save_admin_credentials
    print_admin_credentials
  else
    warn "自动改密失败，请使用默认 ${DEFAULT_ADMIN_USER}/${DEFAULT_ADMIN_PASS} 登录后手动改密"
  fi

  if [[ "$ENABLE_NAT" == "yes" ]]; then
    setup_nat_forwarding || {
      err "NAT 配置失败"
      exit 1
    }
  fi

  if [[ "$WITH_NODE" == "yes" ]]; then
    local local_cs
    local_cs="$(build_config_server_url "$CONFIG_PROTOCOL" "127.0.0.1" "$CONFIG_PORT" "$CONFIG_TOKEN")"
    write_core_service "$local_cs" "node0" "yes"
    systemctl enable ET-core@node0.service
    systemctl restart ET-core@node0.service
  elif [[ "$ENABLE_NAT" == "yes" ]]; then
    warn "已开启主机 NAT，但未部署本机 ET-core；请在实际出口节点启用 enable_exit_node"
  fi

  local api_url host_clean reg_note pass_note backup_note="" cs_url nat_note
  api_url="$(api_host_url "$PUBLIC_HOST")"
  host_clean="$(normalize_host_input "$PUBLIC_HOST")"
  cs_url="$(build_config_server_url "$CONFIG_PROTOCOL" "$host_clean" "$CONFIG_PORT" "$CONFIG_TOKEN")"
  nat_note="$(nat_status_line)"
  if [[ "$ENABLE_DAILY_BACKUP" == "yes" ]]; then
    backup_note="备份目录: ${BACKUP_DIR}（每日 ${BACKUP_HOUR}:00，保留 ${BACKUP_RETENTION_DAYS} 天）"
  else
    backup_note="每日备份: 未启用（可重新运行安装脚本启用）"
  fi
  reg_note="Web 公开注册: 已关闭（仅 admin 登录）"
  if [[ "$ADMIN_PASSWORD_PROVIDED" == "true" ]]; then
    pass_note="admin 密码: 使用安装参数 --admin-password 指定"
  else
    pass_note="admin 密码: 已自动生成（见上方输出或 admin-credentials.txt）"
  fi
  cat >"${INSTALL_PATH}/INSTALL_INFO.txt" <<EOF
EasyTier Server 安装完成
========================
Web 控制台: ${api_url}
配置下发监听: ${CONFIG_PROTOCOL} :${CONFIG_PORT}
客户端连接示例: ${cs_url}
接入 Token: ${CONFIG_TOKEN}
${reg_note}
${pass_note}
凭据文件: ${INSTALL_PATH}/admin-credentials.txt
${nat_note}

下一步:
1. 浏览器打开 ${api_url}，使用 admin 账号登录（内置账号，无需注册）
2. 在 Web「接入 Token」中确认/管理 Token（默认与用户名 admin 相同）
3. 其他节点 client 连接（任选 udp 或 tcp，按网络环境）:
   ${cs_url}

本机节点: ET-core@node0 -> $(build_config_server_url "$CONFIG_PROTOCOL" "127.0.0.1" "$CONFIG_PORT" "$CONFIG_TOKEN")
数据库: ${INSTALL_PATH}/et.db
${backup_note}
EOF

  if [[ "$NGINX_HTTPS_PROXY" == "yes" ]]; then
    cat >>"${INSTALL_PATH}/INSTALL_INFO.txt" <<EOF
Nginx HTTPS: 是（对外 https://${host_clean}，本机反代 127.0.0.1:${API_PORT}）
重要: 须手动部署 Nginx 并 reload，外网 https://${host_clean} 才可访问
Nginx 示例见 docs/easytier-deploy.md（server_name 改为 ${host_clean}）
宝塔目标: /www/server/panel/vhost/nginx/${host_clean}.conf
防火墙: TCP ${NGINX_SSL_PORT}（Web）、UDP ${CONFIG_PORT}（配置）、节点 ${CORE_PORT}
USG: 放行 TCP ${NGINX_SSL_PORT} + UDP ${CONFIG_PORT}，无需对外 TCP ${API_PORT}
EOF
  else
    cat >>"${INSTALL_PATH}/INSTALL_INFO.txt" <<EOF
防火墙: TCP ${API_PORT}、${CONFIG_PROTOCOL} ${CONFIG_PORT}、节点 ${CORE_PORT}
EOF
  fi

  save_install_options

  if [[ "$ENABLE_DAILY_BACKUP" == "yes" ]]; then
    setup_daily_backup
    run_backup_now
  fi

  configure_server_firewall

  if [[ "$NGINX_HTTPS_PROXY" == "yes" ]]; then
    warn "Nginx HTTPS 模式：请手动部署 Nginx，反代到 http://127.0.0.1:${API_PORT}"
    warn "示例配置见 docs/easytier-deploy.md；宝塔可放到 /www/server/panel/vhost/nginx/${host_clean}.conf"
    if ! verify_nginx_https_proxy no; then
      warn "Nginx HTTPS 尚未就绪，部署后执行: nginx -t && systemctl reload nginx"
      warn "然后运行: sudo bash install.sh healthcheck"
    fi
  fi

  log "Server 模式安装完成"
  cat "${INSTALL_PATH}/INSTALL_INFO.txt"
}

install_client() {
  local host_clean config_server
  host_clean="$(normalize_host_input "$SERVER_HOST")"
  config_server="$(build_config_server_url "$CONFIG_PROTOCOL" "$host_clean" "$CONFIG_PORT" "$CONFIG_TOKEN")"

  stop_legacy_easytier_services
  rm -f /etc/systemd/system/easytier-core@default.service \
    /etc/systemd/system/easytier-core@.service

  if [[ "$ENABLE_NAT" == "yes" ]]; then
    setup_nat_forwarding || {
      err "NAT 配置失败"
      exit 1
    }
  fi

  write_core_service "$config_server" "default" "no"
  systemctl daemon-reload
  systemctl enable ET-core@default.service
  systemctl restart ET-core@default.service
  save_install_options

  cat >"${INSTALL_PATH}/INSTALL_INFO.txt" <<EOF
EasyTier Client 安装完成
========================
Config Server: ${config_server}
接入 Token: ${CONFIG_TOKEN}
$(nat_status_line)

管理命令:
  systemctl status ET-core@default
  systemctl restart ET-core@default
  ET-cli peer

请确保控制台「接入 Token」中已存在 Token「${CONFIG_TOKEN}」（默认内置 admin），并在 Web 中配置网络。
EOF
  log "Client 模式安装完成"
  cat "${INSTALL_PATH}/INSTALL_INFO.txt"
}

write_core_standalone_service() {
  local unit_after="After=network-online.target"
  local nat_wants=""
  local nat_flags=""
  if [[ "$ENABLE_NAT" == "yes" ]]; then
    unit_after="After=network-online.target ET-nat.service"
    nat_wants="Wants=ET-nat.service"
    nat_flags="$(printf '\n  --enable-exit-node \\\n  --proxy-forward-by-system')"
  fi
  cat >/etc/systemd/system/ET-core@.service <<EOF
[Unit]
Description=EasyTier Core (%i)
Wants=network-online.target
${nat_wants}
${unit_after}
StartLimitIntervalSec=0

[Service]
Type=simple
WorkingDirectory=${INSTALL_PATH}
ExecStart=${INSTALL_PATH}/ET-core -c ${INSTALL_PATH}/config/%i.conf${nat_flags}
Restart=always
RestartSec=1s
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF
}

patch_core_config_nat_flags() {
  local conf="$1"
  [[ -f "$conf" ]] || return 0
  if [[ "$ENABLE_NAT" == "yes" ]]; then
    if grep -qE '^[[:space:]]*enable_exit_node[[:space:]]*=' "$conf"; then
      sed -i 's/^[[:space:]]*enable_exit_node[[:space:]]*=.*/enable_exit_node = true/' "$conf"
    else
      printf '\nenable_exit_node = true\n' >>"$conf"
    fi
    if grep -qE '^[[:space:]]*proxy_forward_by_system[[:space:]]*=' "$conf"; then
      sed -i 's/^[[:space:]]*proxy_forward_by_system[[:space:]]*=.*/proxy_forward_by_system = true/' "$conf"
    else
      # Prefer placing near enable_exit_node inside [flags]
      if grep -qE '^[[:space:]]*enable_exit_node[[:space:]]*=' "$conf"; then
        sed -i '/^[[:space:]]*enable_exit_node[[:space:]]*=/a proxy_forward_by_system = true' "$conf"
      else
        printf 'proxy_forward_by_system = true\n' >>"$conf"
      fi
    fi
  fi
}

write_core_default_config() {
  local exit_node="false"
  local proxy_sys="false"
  if [[ "$ENABLE_NAT" == "yes" ]]; then
    exit_node="true"
    proxy_sys="true"
  fi
  mkdir -p "$CONFIG_DIR"
  if [[ -f "${CONFIG_DIR}/default.conf" ]]; then
    log "保留已有配置: ${CONFIG_DIR}/default.conf"
    patch_core_config_nat_flags "${CONFIG_DIR}/default.conf"
    return 0
  fi
  cat >"${CONFIG_DIR}/default.conf" <<EOF
instance_name = "default"
dhcp = true
listeners = [
    "tcp://0.0.0.0:${CORE_PORT}",
    "udp://0.0.0.0:${CORE_PORT}",
    "wg://0.0.0.0:${WG_PORT}",
    "ws://0.0.0.0:${WG_PORT}/",
    "wss://0.0.0.0:${WSS_PORT}/",
]
exit_nodes = []
rpc_portal = "0.0.0.0:0"

[[peer]]
uri = "tcp://public.easytier.top:11010"

[network_identity]
network_name = "default"
network_secret = "default"

[flags]
default_protocol = "udp,tcp"
dev_name = ""
enable_encryption = true
enable_ipv6 = true
mtu = 1380
latency_first = false
enable_exit_node = ${exit_node}
proxy_forward_by_system = ${proxy_sys}
no_tun = false
use_smoltcp = false
foreign_network_whitelist = "*"
disable_p2p = false
p2p_only = false
relay_all_peer_rpc = false
disable_tcp_hole_punching = false
disable_udp_hole_punching = false
EOF
}

install_core() {
  write_core_default_config
  # 避免与 server/client 的 ET-core@default（--config-server）冲突
  systemctl stop ET-core@default.service 2>/dev/null || true
  systemctl disable ET-core@default.service 2>/dev/null || true
  stop_legacy_easytier_services
  rm -f /etc/systemd/system/ET-core@default.service \
    /etc/systemd/system/easytier-core@default.service \
    /etc/systemd/system/easytier@.service

  if [[ "$ENABLE_NAT" == "yes" ]]; then
    setup_nat_forwarding || {
      err "NAT 配置失败"
      exit 1
    }
  fi

  write_core_standalone_service
  systemctl daemon-reload
  systemctl enable ET-core@default.service
  systemctl restart ET-core@default.service
  save_install_options

  cat >"${INSTALL_PATH}/INSTALL_INFO.txt" <<EOF
EasyTier Core 轻量安装完成
========================
版本: ${INSTALLED_VERSION:-latest}
安装目录: ${INSTALL_PATH}
配置文件: ${CONFIG_DIR}/default.conf
默认端口: ${CORE_PORT}(UDP+TCP)
默认网络名/密钥: default（请尽快修改）
$(nat_status_line)

Release: ${RELEASES_PAGE}

管理:
  systemctl status ET-core@default
  systemctl restart ET-core@default
  systemctl stop ET-core@default
  systemctl status ET-nat.service

多实例: 在 ${CONFIG_DIR}/ 下新增 xxx.conf 后:
  systemctl enable --now ET-core@xxx
EOF
  log "Core 模式安装完成"
  cat "${INSTALL_PATH}/INSTALL_INFO.txt"
}

prompt_interactive() {
  echo
  echo "=========================================="
  echo " EasyTier 一键安装"
  echo "=========================================="
  echo

  if [[ -z "$MODE" ]]; then
    echo "请选择部署模式:"
    echo "  1) server - 控制台 + 可选本机节点（hk VPS 等）"
    echo "  2) client - 仅节点，连接远程控制台（shlt/shdx 等）"
    echo "  3) core   - 轻量仅 ET-core（本地 config，无 Web 控制台）"
    read -rp "请输入 [1/2/3]: " choice
    case "$choice" in
      1|"") MODE="server" ;;
      2) MODE="client" ;;
      3) MODE="core" ;;
      *) err "无效选择"; exit 1 ;;
    esac
    echo
  fi

  if [[ "$MODE" == "server" ]]; then
    prompt_required_host "控制台域名" PUBLIC_HOST
    read -rp "Web/API 端口 (TCP) [${API_PORT}]: " p; [[ -n "$p" ]] && API_PORT="$p"
    read -rp "配置下发端口 [${CONFIG_PORT}]: " p; [[ -n "$p" ]] && CONFIG_PORT="$p"
    read -rp "配置协议（可逗号多选，如 udp,tcp）[${CONFIG_PROTOCOL}]: " p; [[ -n "$p" ]] && CONFIG_PROTOCOL="$p"
    read -rp "是否通过 Nginx HTTPS 443 反代 Web 控制台? [y/N]: " yn
    if [[ "$yn" =~ ^[Yy] ]]; then
      NGINX_HTTPS_PROXY=yes
      read -rp "HTTPS 端口 [${NGINX_SSL_PORT}]: " p
      [[ -n "$p" ]] && NGINX_SSL_PORT="$p"
      log "将使用 https://域名 对外访问，无需对外放行 Web TCP ${API_PORT}；UDP ${CONFIG_PORT} 配置下发仍须放行"
      log "Nginx 须手动部署并反代到 http://127.0.0.1:${API_PORT}，示例见 docs/ops/deploy-install.md"
    else
      NGINX_HTTPS_PROXY=no
    fi
    read -rp "本机同时运行 ET-core? [Y/n]: " yn
    [[ "$yn" =~ ^[Nn] ]] && WITH_NODE="no" || WITH_NODE="yes"
    read -rp "本机节点接入 Token [${CONFIG_TOKEN}]: " u
    if [[ -n "$u" ]]; then
      validate_config_token "$u" "接入 Token" || exit 1
      CONFIG_TOKEN="$u"
    fi
    read -rp "admin 密码 [回车=自动生成强密码]: " pass
    if [[ -n "$pass" ]]; then
      ADMIN_PASSWORD="$pass"
      ADMIN_PASSWORD_PROVIDED=true
    fi
    read -rp "启用每日数据库备份? [Y/n]: " yn
    [[ "$yn" =~ ^[Nn] ]] && ENABLE_DAILY_BACKUP="no" || ENABLE_DAILY_BACKUP="yes"
    if [[ "$ENABLE_DAILY_BACKUP" == "yes" ]]; then
      read -rp "备份保留天数 [${BACKUP_RETENTION_DAYS}]: " d
      [[ -n "$d" ]] && BACKUP_RETENTION_DAYS="$d"
      read -rp "每日备份时刻 0-23 [${BACKUP_HOUR}]: " h
      [[ -n "$h" ]] && BACKUP_HOUR="$h"
    fi
    if firewall_is_active; then
      local backend
      backend="$(detect_firewall_backend)"
      read -rp "本机防火墙(${backend})已启用，添加 EasyTier 放行规则? [Y/n]: " yn
      [[ "$yn" =~ ^[Nn] ]] && CONFIGURE_FIREWALL="no" || CONFIGURE_FIREWALL="yes"
    else
      read -rp "本机防火墙未启用，是否启用并添加放行规则? [y/N]: " yn
      if [[ "$yn" =~ ^[Yy] ]]; then
        ENABLE_FIREWALL="yes"
        CONFIGURE_FIREWALL="yes"
      else
        log "跳过本机防火墙配置"
        CONFIGURE_FIREWALL="no"
      fi
    fi
    echo
    if [[ "$ADMIN_PASSWORD_PROVIDED" != "true" ]]; then
      log "未指定密码时将自动生成 admin 强密码并在安装完成后打印"
    fi
  elif [[ "$MODE" == "client" ]]; then
    prompt_required_host "控制台地址" SERVER_HOST
    read -rp "配置下发端口 [${CONFIG_PORT}]: " p; [[ -n "$p" ]] && CONFIG_PORT="$p"
    read -rp "客户端连接协议 udp/tcp/ws [$(primary_config_scheme)]: " p
    if [[ -n "$p" ]]; then
      CONFIG_PROTOCOL="$p"
    else
      # Prefer single scheme for client URLs when default is a listen list
      CONFIG_PROTOCOL="$(primary_config_scheme)"
    fi
    read -rp "接入 Token [${CONFIG_TOKEN}]: " u
    if [[ -n "$u" ]]; then
      validate_config_token "$u" "接入 Token" || exit 1
      CONFIG_TOKEN="$u"
    fi
  else
    # core: 仅确认安装路径
    read -rp "安装目录 [${INSTALL_PATH}]: " p
    if [[ -n "$p" ]]; then
      INSTALL_PATH="$p"
      CONFIG_DIR="${INSTALL_PATH}/config"
    fi
  fi

  prompt_nat_forwarding
  prompt_download_source
}

prompt_restore_file() {
  resolve_backup_dir
  local -a files=()
  local f
  while IFS= read -r f; do
    [[ -n "$f" ]] && files+=("$f")
  done < <(find "$BACKUP_DIR" -maxdepth 1 -type f -name 'easytier-db-*.tar.gz' -printf '%T@ %p\n' 2>/dev/null \
    | sort -rn | cut -d' ' -f2- || ls -t "${BACKUP_DIR}"/easytier-db-*.tar.gz 2>/dev/null || true)

  if [[ ${#files[@]} -eq 0 ]]; then
    err "备份目录无 easytier-db-*.tar.gz: ${BACKUP_DIR}"
    exit 1
  fi

  if [[ ${#files[@]} -eq 1 ]]; then
    RESTORE_FILE="${files[0]}"
    log "使用备份: $(basename "$RESTORE_FILE")"
    return 0
  fi

  echo
  echo "可选备份（新 → 旧）:"
  local i=1 idx
  for f in "${files[@]}"; do
    echo "  ${i}) $(basename "$f")"
    i=$((i + 1))
  done
  read -rp "请选择 [1]: " idx
  idx="${idx:-1}"
  [[ "$idx" =~ ^[0-9]+$ ]] && (( idx >= 1 && idx <= ${#files[@]} )) || {
    err "无效选择: $idx"
    exit 1
  }
  RESTORE_FILE="${files[$((idx - 1))]}"
  log "将恢复: $(basename "$RESTORE_FILE")"
}

confirm_or_cancel() {
  local prompt="$1"
  local confirm=""
  read -rp "$prompt" confirm
  if [[ "$confirm" =~ ^[Nn]$ ]]; then
    log "已取消"
    exit 0
  fi
}

parse_install_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --mode) MODE="$2"; shift 2 ;;
      --auto) AUTO=true; shift ;;
      --install-path) INSTALL_PATH="$2"; CONFIG_DIR="${INSTALL_PATH}/config"; shift 2 ;;
      --skip-folder-verify|--skip-folder-fix) shift ;; # 旧版 install.sh 兼容，已忽略
      --no-gh-proxy) NO_GH_PROXY=true; DOWNLOAD_PRESET=true; shift ;;
      --gh-proxy) GH_PROXY="$2"; NO_GH_PROXY=false; DOWNLOAD_PRESET=true; shift 2 ;;
      --gh-mirrors) GH_MIRRORS="$2"; NO_GH_PROXY=false; DOWNLOAD_PRESET=true; shift 2 ;;
      --public-host) PUBLIC_HOST="$2"; shift 2 ;;
      --server-host) SERVER_HOST="$2"; shift 2 ;;
      --api-port) API_PORT="$2"; shift 2 ;;
      --config-port) CONFIG_PORT="$2"; shift 2 ;;
      --config-protocol) CONFIG_PROTOCOL="$2"; shift 2 ;;
      --with-node) WITH_NODE="$2"; shift 2 ;;
      --config-token) CONFIG_TOKEN="$2"; shift 2 ;;
      --web-username) WEB_USERNAME="$2"; shift 2 ;; # deprecated alias for --config-token
      --admin-password) ADMIN_PASSWORD="$2"; ADMIN_PASSWORD_PROVIDED=true; shift 2 ;;
      --backup-dir) BACKUP_DIR="$2"; shift 2 ;;
      --no-daily-backup) ENABLE_DAILY_BACKUP="no"; shift ;;
      --backup-retention-days) BACKUP_RETENTION_DAYS="$2"; shift 2 ;;
      --backup-hour) BACKUP_HOUR="$2"; shift 2 ;;
      --nginx-https-proxy)
        case "$2" in
          yes|y|Y|true|1) NGINX_HTTPS_PROXY=yes ;;
          no|n|N|false|0) NGINX_HTTPS_PROXY=no ;;
          *) err "无效值: $2（须 yes 或 no）"; exit 1 ;;
        esac
        shift 2
        ;;
      --nginx-ssl-port) NGINX_SSL_PORT="$2"; shift 2 ;;
      --configure-firewall) CONFIGURE_FIREWALL="yes"; shift ;;
      --enable-firewall) ENABLE_FIREWALL="yes"; CONFIGURE_FIREWALL="yes"; shift ;;
      --enable-nat|--nat) ENABLE_NAT="yes"; NAT_PROMPT_DONE=true; shift ;;
      --no-nat) ENABLE_NAT="no"; NAT_PROMPT_DONE=true; shift ;;
      --nat-wan)
        NAT_WAN_IFACE="$2"
        ENABLE_NAT="yes"
        NAT_PROMPT_DONE=true
        shift 2
        ;;
      --no-install-deps) INSTALL_DEPS="no"; shift ;;
      -h|--help) usage; exit 0 ;;
      # 旧版: install /opt/easytier
      /*|[a-zA-Z]:*)
        INSTALL_PATH="$1"
        CONFIG_DIR="${INSTALL_PATH}/config"
        shift
        ;;
      *) err "未知参数: $1"; usage; exit 1 ;;
    esac
  done
}

parse_common_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --install-path) INSTALL_PATH="$2"; CONFIG_DIR="${INSTALL_PATH}/config"; shift 2 ;;
      --backup-dir) BACKUP_DIR="$2"; shift 2 ;;
      --backup-retention-days) BACKUP_RETENTION_DAYS="$2"; shift 2 ;;
      --file) RESTORE_FILE="$2"; shift 2 ;;
      --auto) AUTO=true; shift ;;
      --public-host) PUBLIC_HOST="$2"; shift 2 ;;
      --server-host) SERVER_HOST="$2"; shift 2 ;;
      --config-token) CONFIG_TOKEN="$2"; shift 2 ;;
      --config-protocol) CONFIG_PROTOCOL="$2"; shift 2 ;;
      --config-port) CONFIG_PORT="$2"; shift 2 ;;
      --enable-nat|--nat) ENABLE_NAT="yes"; NAT_PROMPT_DONE=true; shift ;;
      --no-nat) ENABLE_NAT="no"; NAT_PROMPT_DONE=true; shift ;;
      --nat-wan)
        NAT_WAN_IFACE="$2"
        ENABLE_NAT="yes"
        NAT_PROMPT_DONE=true
        shift 2
        ;;
      --configure-firewall) CONFIGURE_FIREWALL="yes"; shift ;;
      --enable-firewall) ENABLE_FIREWALL="yes"; CONFIGURE_FIREWALL="yes"; shift ;;
      --no-gh-proxy) NO_GH_PROXY=true; DOWNLOAD_PRESET=true; shift ;;
      --gh-proxy) GH_PROXY="$2"; NO_GH_PROXY=false; DOWNLOAD_PRESET=true; shift 2 ;;
      --gh-mirrors) GH_MIRRORS="$2"; NO_GH_PROXY=false; DOWNLOAD_PRESET=true; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) err "未知参数: $1"; usage; exit 1 ;;
    esac
  done
}

pick_latest_backup_file() {
  resolve_backup_dir
  local latest
  latest="$(find "$BACKUP_DIR" -maxdepth 1 -type f -name 'easytier-db-*.tar.gz' -printf '%T@ %p\n' 2>/dev/null \
    | sort -rn | head -1 | cut -d' ' -f2- || true)"
  if [[ -z "$latest" ]]; then
    latest="$(ls -t "${BACKUP_DIR}"/easytier-db-*.tar.gz 2>/dev/null | head -1 || true)"
  fi
  [[ -n "$latest" ]] || return 1
  echo "$latest"
}

validate_restore_archive() {
  local archive="$1"
  [[ -f "$archive" ]] || { err "备份文件不存在: $archive"; exit 1; }
  [[ "$archive" == *.tar.gz ]] || { err "备份须为 easytier-db-*.tar.gz 格式"; exit 1; }
}

cmd_update() {
  require_root
  parse_common_args "$@"
  need_cmd
  [[ -d "$INSTALL_PATH" ]] || { err "未找到 ${INSTALL_PATH}，请先: sudo bash install.sh install"; exit 1; }
  if [[ ! -x "${INSTALL_PATH}/ET-core" && ! -x "${INSTALL_PATH}/easytier-core" ]]; then
    err "未找到 ET-core / easytier-core 二进制，请先 install"
    exit 1
  fi

  load_runtime_config_from_systemd
  # CLI --no-nat / --enable-nat 优先；否则从 install-options / 运行态恢复
  if [[ "$NAT_PROMPT_DONE" != "true" ]]; then
    reconcile_nat_from_runtime
  fi
  case "$(printf '%s' "$ENABLE_NAT" | tr '[:upper:]' '[:lower:]')" in
    yes|y|true|1) ENABLE_NAT="yes" ;;
    *) ENABLE_NAT="no" ;;
  esac

  # Remember which roles were installed (ET-* or legacy) so we can rewrite units
  local had_web=no had_node0=no had_default=no had_backup=no
  local core_standalone=no
  if unit_present ET-web.service || unit_present easytier-web.service; then
    had_web=yes
    MODE="${MODE:-server}"
  fi
  if unit_present ET-core@node0.service || unit_present easytier-core@node0.service; then
    had_node0=yes
    WITH_NODE="yes"
  fi
  if unit_present ET-core@default.service || unit_present easytier-core@default.service || \
     [[ -f /etc/systemd/system/ET-core@.service ]]; then
    # template-only core 也可能仅有 ET-core@.service + enabled instance
    if systemctl is-enabled ET-core@default.service &>/dev/null || \
       unit_present ET-core@default.service || unit_present easytier-core@default.service; then
      had_default=yes
    fi
  fi
  if unit_present ET-backup.timer || unit_present easytier-backup.timer; then
    had_backup=yes
  fi
  if is_core_standalone_install; then
    core_standalone=yes
    MODE="${MODE:-core}"
  elif [[ "$had_default" == "yes" && "$had_web" != "yes" ]]; then
    MODE="${MODE:-client}"
  fi

  if is_interactive; then
    prompt_download_source
    echo
    log "更新源: ${RELEASES_PAGE}"
    log "$(nat_status_line)"
    warn "更新将停止服务、从 Release 下载最新包并重启（保留配置/数据库）"
    confirm_or_cancel "确认更新? [Y/n]: "
  else
    log "更新源: ${RELEASES_PAGE}"
    log "$(nat_status_line)"
  fi

  if [[ -f "${INSTALL_PATH}/et.db" ]]; then
    log "更新前自动备份数据库..."
    run_backup_now
  fi

  local update_ok=no
  _update_on_exit() {
    if [[ "$update_ok" != "yes" ]]; then
      warn "更新失败或中断，尝试用旧二进制重新启动服务..."
      systemctl daemon-reload 2>/dev/null || true
      start_easytier_services || true
    fi
  }
  trap _update_on_exit EXIT

  log "停止 EasyTier 服务..."
  stop_easytier_services

  local arch
  arch="$(detect_arch)"
  download_and_install_binaries "$arch"

  # 与 install 一致：保留/刷新 NAT（幂等）
  if [[ "$ENABLE_NAT" == "yes" ]]; then
    setup_nat_forwarding || warn "NAT 刷新失败，请检查 ET-nat.service"
  fi

  # Rewrite systemd units（与 install 相同的 write_* 路径，含 NAT 标志）
  if [[ "$had_web" == "yes" ]]; then
    [[ -f "${INSTALL_PATH}/ET-web-embed" ]] || { err "更新包缺少 ET-web-embed"; exit 1; }
    if [[ -z "${PUBLIC_HOST:-}" ]]; then
      err "无法解析控制台地址，请加参数: --public-host <域名或IP>"
      exit 1
    fi
    write_web_service "$PUBLIC_HOST"
    systemctl enable ET-web.service
  fi
  if [[ "$had_node0" == "yes" ]]; then
    local local_cs
    local_cs="$(build_config_server_url "$CONFIG_PROTOCOL" "127.0.0.1" "$CONFIG_PORT" "$CONFIG_TOKEN")"
    write_core_service "$local_cs" "node0" "yes"
    systemctl enable ET-core@node0.service
  fi
  if [[ "$had_default" == "yes" ]]; then
    if [[ "$core_standalone" == "yes" ]]; then
      write_core_standalone_service
      patch_core_config_nat_flags "${CONFIG_DIR}/default.conf"
      systemctl enable ET-core@default.service
    else
      local host_clean config_server
      host_clean="$(normalize_host_input "${SERVER_HOST:-${PUBLIC_HOST:-127.0.0.1}}")"
      config_server="$(build_config_server_url "$CONFIG_PROTOCOL" "$host_clean" "$CONFIG_PORT" "$CONFIG_TOKEN")"
      write_core_service "$config_server" "default" "no"
      systemctl enable ET-core@default.service
    fi
  fi
  if [[ "$had_backup" == "yes" ]]; then
    setup_daily_backup
  fi

  # 可选：与 install 相同方式刷新本机防火墙（须显式传参）
  if [[ "$CONFIGURE_FIREWALL" == "yes" || "$ENABLE_FIREWALL" == "yes" ]]; then
    if [[ "$had_web" == "yes" ]]; then
      MODE="${MODE:-server}"
      configure_server_firewall
    fi
  fi

  # Drop legacy unit files and old binary names left from pre-rename installs
  rm -f /etc/systemd/system/easytier-web.service \
    /etc/systemd/system/easytier-core@node0.service \
    /etc/systemd/system/easytier-core@default.service \
    /etc/systemd/system/easytier-core@.service \
    /etc/systemd/system/easytier-backup.service \
    /etc/systemd/system/easytier-backup.timer
  rm -f "${INSTALL_PATH}/easytier-core" "${INSTALL_PATH}/easytier-cli" \
    "${INSTALL_PATH}/easytier-web-embed" "${INSTALL_PATH}/easytier-web" \
    "${INSTALL_PATH}/easytier-backup.sh"
  rm -f /usr/sbin/easytier-core /usr/sbin/easytier-cli

  save_install_options

  systemctl daemon-reload
  log "重启服务..."
  start_easytier_services
  if [[ "$ENABLE_NAT" == "yes" ]]; then
    systemctl start ET-nat.service 2>/dev/null || true
  fi

  update_ok=yes
  trap - EXIT

  log "更新完成，版本: ${INSTALLED_VERSION}（Release: ${RELEASES_PAGE}）"
  verify_boot_services
  if [[ "$core_standalone" == "yes" && "$had_web" != "yes" ]]; then
    if systemctl is-active --quiet ET-core@default.service 2>/dev/null; then
      log "ET-core@default 运行中"
    else
      warn "ET-core@default 未运行，请检查: systemctl status ET-core@default"
    fi
    verify_nat_forwarding || true
    return 0
  fi
  run_health_check || exit 1
}

cmd_restore() {
  require_root
  parse_common_args "$@"
  need_cmd
  [[ -f "${INSTALL_PATH}/et.db" ]] || { err "未找到 ${INSTALL_PATH}/et.db"; exit 1; }

  resolve_backup_dir
  local archive="$RESTORE_FILE"
  if [[ -z "$archive" ]]; then
    if is_interactive; then
      prompt_restore_file
      archive="$RESTORE_FILE"
    else
      archive="$(pick_latest_backup_file)" || { err "备份目录无 easytier-db-*.tar.gz: ${BACKUP_DIR}"; exit 1; }
      log "使用最新备份: $archive"
    fi
  fi
  validate_restore_archive "$archive"

  if [[ "$AUTO" != "true" ]]; then
    echo
    warn "将用备份恢复数据库，当前 et.db 会先备份为 et.db.pre-restore-*"
    read -rp "确认恢复 $(basename "$archive")? [y/N]: " confirm
    [[ "$confirm" =~ ^[Yy]$ ]] || { log "已取消"; exit 0; }
  fi

  log "恢复前备份当前数据库..."
  run_backup_now

  log "停止 EasyTier 服务..."
  stop_easytier_services

  local tmp_dir pre_db
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT

  tar -xzf "$archive" -C "$tmp_dir" --no-same-owner --no-wildcards
  [[ -f "${tmp_dir}/et.db" ]] || { err "备份包内无 et.db"; exit 1; }

  pre_db="${INSTALL_PATH}/et.db.pre-restore-$(date +%Y%m%d-%H%M%S)"
  cp -a "${INSTALL_PATH}/et.db" "$pre_db"
  rm -f "${INSTALL_PATH}/et.db-wal" "${INSTALL_PATH}/et.db-shm"
  cp -a "${tmp_dir}/et.db" "${INSTALL_PATH}/et.db"
  chmod 600 "${INSTALL_PATH}/et.db"

  if [[ -f "${tmp_dir}/admin-credentials.txt" ]]; then
    cp -a "${tmp_dir}/admin-credentials.txt" "${INSTALL_PATH}/admin-credentials.txt"
    chmod 600 "${INSTALL_PATH}/admin-credentials.txt"
    log "已恢复 admin-credentials.txt"
  fi

  log "已恢复 et.db（恢复前副本: ${pre_db}）"
  log "启动服务..."
  start_easytier_services

  if is_server_installed && ! systemctl is-active --quiet ET-web.service 2>/dev/null; then
    warn "Web 服务启动失败，回滚数据库..."
    cp -a "$pre_db" "${INSTALL_PATH}/et.db"
    rm -f "${INSTALL_PATH}/et.db-wal" "${INSTALL_PATH}/et.db-shm"
    chmod 600 "${INSTALL_PATH}/et.db"
    restart_easytier_services
    err "恢复失败，已回滚到恢复前数据库"
    exit 1
  fi

  verify_boot_services
  run_health_check || exit 1
}

cmd_healthcheck() {
  parse_common_args "$@"
  command -v curl >/dev/null 2>&1 || { err "需要 curl"; exit 1; }
  run_health_check || exit 1
}

cmd_backup() {
  require_root
  parse_common_args "$@"
  [[ -f "${INSTALL_PATH}/et.db" ]] || { err "未找到 ${INSTALL_PATH}/et.db，仅 server 模式需要备份"; exit 1; }
  run_backup_now
}

cmd_install() {
  parse_install_args "$@"
  if [[ "$AUTO" == "true" ]]; then
    # curl | bash -s install 无 TTY 时默认 core（兼容旧版轻量安装）
    [[ -n "$MODE" ]] || MODE="core"
    if [[ "$MODE" == "server" && -z "$PUBLIC_HOST" ]]; then
      err "非交互 server 安装须指定 --public-host DOMAIN"
      exit 1
    fi
    if [[ "$MODE" == "client" && -z "$SERVER_HOST" ]]; then
      err "非交互 client 安装须指定 --server-host DOMAIN"
      exit 1
    fi
  elif is_interactive; then
    prompt_interactive
  elif [[ -z "$MODE" ]]; then
    # 管道执行且未指定 --mode：默认轻量 core
    MODE="core"
    log "非交互环境，使用 --mode core（轻量安装）"
  fi
  [[ "$MODE" == "server" || "$MODE" == "client" || "$MODE" == "core" ]] || {
    err "模式必须是 server、client 或 core"
    exit 1
  }

  apply_defaults

  if is_interactive; then
    echo
    log "即将安装: 模式=${MODE}"
    if [[ "$MODE" == "server" ]]; then
      log "控制台域名=${PUBLIC_HOST}"
      if [[ "$NGINX_HTTPS_PROXY" == "yes" ]]; then
        log "Web 访问=https://$(normalize_host_input "$PUBLIC_HOST")（Nginx HTTPS ${NGINX_SSL_PORT}）"
      fi
    elif [[ "$MODE" == "client" ]]; then
      log "连接控制台=${SERVER_HOST}"
    else
      log "安装目录=${INSTALL_PATH}"
    fi
    log "$(nat_status_line)"
    log "下载源: $(describe_download_source)"
    log "Release: ${RELEASES_PAGE}"
    confirm_or_cancel "确认继续安装? [Y/n]: "
  fi

  log "开始安装（从 ${RELEASES_PAGE} 拉取最新版）..."
  warn "EasyTier 仍在快速迭代，请自行承担使用风险。"
  need_cmd
  ensure_debian_packages
  local arch
  arch="$(detect_arch)"
  download_easytier "$arch"

  if [[ "$MODE" == "server" ]]; then
    install_server
  elif [[ "$MODE" == "client" ]]; then
    install_client
  else
    install_core
  fi
  verify_boot_services
  if [[ "$MODE" == "core" ]]; then
    if systemctl is-active --quiet ET-core@default.service 2>/dev/null; then
      log "ET-core@default 运行中"
    else
      warn "ET-core@default 未运行，请检查: systemctl status ET-core@default"
    fi
    return 0
  fi
  if ! run_health_check; then
    if [[ "$MODE" == "server" && "$NGINX_HTTPS_PROXY" == "yes" ]]; then
      warn "健康检查未完全通过（常见原因：Nginx HTTPS 尚未部署）"
    else
      err "健康检查失败，请根据上方提示排查后重试: sudo bash install.sh healthcheck"
      exit 1
    fi
  fi
}

cmd_uninstall() {
  require_root
  if is_interactive; then
    echo
    warn "将停止服务并移除二进制；${INSTALL_PATH} 会备份为 ${INSTALL_PATH}.bak.*"
    read -rp "确认卸载? [y/N]: " confirm
    [[ "$confirm" =~ ^[Yy]$ ]] || { log "已取消"; exit 0; }
  fi
  log "停止服务..."
  remove_daily_backup
  remove_nat_forwarding
  stop_easytier_services
  systemctl disable ET-web.service 2>/dev/null || true
  systemctl disable ET-core@node0.service 2>/dev/null || true
  systemctl disable ET-core@default.service 2>/dev/null || true
  remove_systemd_units
  systemctl daemon-reload
  if [[ -d "$INSTALL_PATH" ]]; then
    local bak="${INSTALL_PATH}.bak.$(date +%Y%m%d%H%M%S)"
    mv "$INSTALL_PATH" "$bak"
    log "配置已备份到 $bak"
  fi
  rm -f /usr/sbin/ET-core /usr/sbin/ET-cli
  rm -f /usr/sbin/easytier-core /usr/sbin/easytier-cli
  log "卸载完成（已尝试清理 NAT 规则与 ET-nat.service）"
}

cmd_status() {
  verify_boot_services
  run_health_check || true
  systemctl status ET-web.service --no-pager 2>/dev/null || true
  systemctl status ET-core@node0.service --no-pager 2>/dev/null || true
  systemctl status ET-core@default.service --no-pager 2>/dev/null || true
  systemctl status ET-backup.timer --no-pager 2>/dev/null || true
  systemctl status ET-nat.service --no-pager 2>/dev/null || true
  if [[ -f "${INSTALL_PATH}/INSTALL_INFO.txt" ]]; then
    cat "${INSTALL_PATH}/INSTALL_INFO.txt"
  fi
  return 0
}

# 入口由 install.sh / update.sh 提供，本文件仅定义函数。
