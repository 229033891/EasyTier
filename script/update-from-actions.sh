#!/usr/bin/env bash
#
# 从 GitHub Actions（ET Linux）产物更新本机已安装的 ET-web / ET-core
#
# 默认对应：ET Linux #19
#   https://github.com/229033891/EasyTier/actions/runs/37098657441
#   commit 45685e8 — fix(ci): unblock ET Test after 2.7.1 bump
#
# 用法（在 Ubuntu 服务器上，root）：
#   # 1) 安装 gh 并登录（一次性）
#   #    curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
#   #      | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg
#   #    或：apt install gh && gh auth login
#
#   # 2) 执行（默认更新 /opt/easytier）
#   sudo bash update-from-actions.sh
#
#   # 或指定其它 run：
#   sudo RUN_ID=37098657441 bash update-from-actions.sh
#   sudo RUN_URL='https://github.com/229033891/EasyTier/actions/runs/37098657441' \
#     bash update-from-actions.sh
#
# 环境变量：
#   GITHUB_REPO   默认 229033891/EasyTier
#   RUN_ID        Actions run id（优先于 RUN_URL）
#   RUN_URL       Actions 页面 URL（自动解析 run id）
#   INSTALL_PATH  默认 /opt/easytier
#   ARTIFACT_NAME 默认 ET-linux-x86_64（含 ET-core / ET-cli / ET-web-embed）
#   GH_TOKEN      若未安装 gh，可用 PAT（需 actions:read）走 API 下载
#
set -euo pipefail

GITHUB_REPO="${GITHUB_REPO:-229033891/EasyTier}"
INSTALL_PATH="${INSTALL_PATH:-/opt/easytier}"
ARTIFACT_NAME="${ARTIFACT_NAME:-ET-linux-x86_64}"
RUN_ID="${RUN_ID:-}"
RUN_URL="${RUN_URL:-https://github.com/229033891/EasyTier/actions/runs/37098657441}"

RED='\e[1;31m'
GREEN='\e[1;32m'
YELLOW='\e[1;33m'
RES='\e[0m'

log()  { echo -e "${GREEN}[+]${RES} $*"; }
warn() { echo -e "${YELLOW}[!]${RES} $*"; }
err()  { echo -e "${RED}[x]${RES} $*" >&2; exit 1; }

require_root() {
  [[ "$(id -u)" -eq 0 ]] || err "请用 root 运行：sudo bash $0"
}

resolve_run_id() {
  if [[ -n "$RUN_ID" ]]; then
    return 0
  fi
  if [[ -n "$RUN_URL" && "$RUN_URL" =~ /actions/runs/([0-9]+) ]]; then
    RUN_ID="${BASH_REMATCH[1]}"
    return 0
  fi
  err "请设置 RUN_ID 或 RUN_URL（Actions 页面链接）"
}

need_cmds() {
  local c
  for c in curl unzip systemctl; do
    command -v "$c" >/dev/null 2>&1 || err "缺少命令: $c（apt install -y $c）"
  done
}

download_with_gh() {
  command -v gh >/dev/null 2>&1 || return 1
  if ! gh auth status -h github.com &>/dev/null; then
    warn "已安装 gh 但未登录；尝试 GH_TOKEN / 其它方式"
    return 1
  fi
  log "使用 gh 下载 artifact: ${ARTIFACT_NAME}（run ${RUN_ID}）"
  rm -rf "$WORKDIR"
  mkdir -p "$WORKDIR"
  (
    cd "$WORKDIR"
    gh run download "$RUN_ID" \
      --repo "$GITHUB_REPO" \
      --name "$ARTIFACT_NAME" \
      --dir "./out"
  )
}

download_with_api() {
  local token="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
  [[ -n "$token" ]] || return 1

  log "使用 GitHub API + token 下载 artifact: ${ARTIFACT_NAME}"
  local api="https://api.github.com/repos/${GITHUB_REPO}/actions/runs/${RUN_ID}/artifacts"
  local art_id
  art_id="$(curl -fsSL -H "Authorization: Bearer ${token}" -H "Accept: application/vnd.github+json" \
    "$api" | python3 -c "
import json,sys
name=sys.argv[1]
data=json.load(sys.stdin)
for a in data.get('artifacts',[]):
  if a.get('name')==name and not a.get('expired'):
    print(a['id']); break
else:
  sys.exit(2)
" "$ARTIFACT_NAME")" || err "未找到未过期 artifact: ${ARTIFACT_NAME}"

  rm -rf "$WORKDIR"
  mkdir -p "$WORKDIR"
  curl -fsSL -L \
    -H "Authorization: Bearer ${token}" \
    -H "Accept: application/vnd.github+json" \
    "https://api.github.com/repos/${GITHUB_REPO}/actions/artifacts/${art_id}/zip" \
    -o "${WORKDIR}/artifact.zip"
  unzip -oq "${WORKDIR}/artifact.zip" -d "${WORKDIR}/out"
}

find_bin_dir() {
  local d
  for d in \
    "${WORKDIR}/out" \
    "${WORKDIR}/out/${ARTIFACT_NAME}" \
    "${WORKDIR}/out/artifacts"; do
    if [[ -f "${d}/ET-core" || -f "${d}/ET-web-embed" ]]; then
      echo "$d"
      return 0
    fi
  done
  local found
  found="$(find "${WORKDIR}/out" -maxdepth 3 -type f \( -name ET-core -o -name ET-web-embed \) 2>/dev/null | head -1 || true)"
  [[ -n "$found" ]] || return 1
  dirname "$found"
}

backup_db() {
  local stamp
  stamp="$(date +%Y%m%d-%H%M%S)"
  local bak_dir="${INSTALL_PATH}/backups/pre-actions-${stamp}"
  mkdir -p "$bak_dir"
  local f
  for f in et.db et.db-wal et.db-shm; do
    if [[ -f "${INSTALL_PATH}/${f}" ]]; then
      cp -a "${INSTALL_PATH}/${f}" "${bak_dir}/"
      log "已备份 ${f} → ${bak_dir}/"
    fi
  done
  # 常见自定义路径
  if systemctl cat ET-web.service &>/dev/null; then
    local exec_line db_path
    exec_line="$(systemctl show ET-web.service -p ExecStart --value 2>/dev/null || true)"
    if [[ "$exec_line" =~ --db[= ]([^[:space:]]+) ]]; then
      db_path="${BASH_REMATCH[1]}"
      if [[ -f "$db_path" && "$db_path" != "${INSTALL_PATH}/et.db" ]]; then
        cp -a "$db_path" "${bak_dir}/$(basename "$db_path")"
        [[ -f "${db_path}-wal" ]] && cp -a "${db_path}-wal" "$bak_dir/" || true
        [[ -f "${db_path}-shm" ]] && cp -a "${db_path}-shm" "$bak_dir/" || true
        log "已备份自定义 db: ${db_path}"
      fi
    fi
  fi
}

stop_services() {
  for u in ET-web.service ET-core@node0.service ET-core@default.service; do
    if systemctl cat "$u" &>/dev/null; then
      log "停止 ${u}"
      systemctl stop "$u" 2>/dev/null || true
    fi
  done
}

install_bins() {
  local src="$1"
  mkdir -p "$INSTALL_PATH"
  [[ -f "${src}/ET-core" ]] || err "产物中缺少 ET-core（目录: ${src}）"
  [[ -f "${src}/ET-cli" ]] || warn "产物中缺少 ET-cli（可继续）"
  [[ -f "${src}/ET-web-embed" ]] || warn "产物中缺少 ET-web-embed（仅更新 core 时正常）"

  for b in ET-core ET-cli ET-web-embed ET-web; do
    if [[ -f "${src}/${b}" ]]; then
      install -m 755 "${src}/${b}" "${INSTALL_PATH}/${b}"
      log "已安装 ${INSTALL_PATH}/${b}"
    fi
  done
  ln -sfn "${INSTALL_PATH}/ET-core" /usr/sbin/ET-core 2>/dev/null || true
  ln -sfn "${INSTALL_PATH}/ET-cli" /usr/sbin/ET-cli 2>/dev/null || true
}

start_services() {
  systemctl daemon-reload 2>/dev/null || true
  for u in ET-web.service ET-core@node0.service ET-core@default.service; do
    if systemctl is-enabled "$u" &>/dev/null; then
      log "启动 ${u}"
      systemctl start "$u"
    elif systemctl cat "$u" &>/dev/null; then
      warn "${u} 存在但未 enable，尝试 start"
      systemctl start "$u" 2>/dev/null || true
    fi
  done
}

print_status() {
  echo
  log "服务状态："
  for u in ET-web.service ET-core@node0.service ET-core@default.service; do
    if systemctl cat "$u" &>/dev/null; then
      systemctl --no-pager --full status "$u" | head -n 8 || true
      echo
    fi
  done
  if [[ -x "${INSTALL_PATH}/ET-cli" ]]; then
    "${INSTALL_PATH}/ET-cli" --version 2>/dev/null || true
  fi
  if [[ -x "${INSTALL_PATH}/ET-core" ]]; then
    "${INSTALL_PATH}/ET-core" --version 2>/dev/null || true
  fi
}

main() {
  require_root
  need_cmds
  resolve_run_id
  WORKDIR="$(mktemp -d /tmp/et-actions-XXXXXX)"
  trap 'rm -rf "$WORKDIR"' EXIT

  log "仓库=${GITHUB_REPO}  run=${RUN_ID}  安装目录=${INSTALL_PATH}"
  log "artifact=${ARTIFACT_NAME}"

  if ! download_with_gh; then
    if ! download_with_api; then
      err "下载失败。请先: gh auth login   或设置 GH_TOKEN（需 actions:read）"
    fi
  fi

  local bin_dir
  bin_dir="$(find_bin_dir)" || err "解压后未找到 ET-core / ET-web-embed"
  log "产物目录: ${bin_dir}"
  ls -la "$bin_dir"

  [[ -d "$INSTALL_PATH" ]] || err "安装目录不存在: ${INSTALL_PATH}（是否曾用 easytier-install.sh 安装？）"

  backup_db
  stop_services
  install_bins "$bin_dir"
  start_services
  sleep 2
  print_status
  log "完成。数据库未改动；若控制台异常，可从 ${INSTALL_PATH}/backups/ 恢复。"
}

main "$@"
