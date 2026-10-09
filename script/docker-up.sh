#!/bin/bash
#
# EasyTier Docker Compose 一键启动（自动创建挂载目录）
#
# 用法:
#   bash docker-up.sh                    # 默认：只启动 node
#   bash docker-up.sh node --dir /vol1/1000/docker/easytier
#   bash docker-up.sh console --pull
#   bash docker-up.sh all
#
# 远程（需已有 docker-compose.yml 或同目录 clone）:
#   curl -fsSL https://github.com/229033891/EasyTier/raw/main/script/docker-up.sh \
#     | bash -s node --dir /vol1/1000/docker/easytier
#
set -euo pipefail

MODE="${1:-node}"
shift || true

DEPLOY_DIR="${ET_DOCKER_DIR:-}"
DO_PULL=0
COMPOSE_FILE="docker-compose.yml"

log() { printf '[docker-up] %s\n' "$*"; }
err() { printf '[docker-up][ERROR] %s\n' "$*" >&2; }

usage() {
  cat <<'EOF'
用法: docker-up.sh [node|console|all] [选项]

  node      只启动组网节点（默认）
  console   只启动 Web 控制台
  all       同时启动控制台与节点

选项:
  --dir PATH     compose 所在目录（默认: 当前目录，或含 docker-compose.yml 的目录）
  --pull         启动前先 docker compose pull
  -h, --help     显示帮助

环境变量:
  ET_DOCKER_DIR  同 --dir

示例:
  bash docker-up.sh node --dir /vol1/1000/docker/easytier --pull
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    node|console|all)
      MODE="$1"
      shift
      ;;
    --dir)
      [[ $# -ge 2 ]] || { err "缺少 --dir 参数"; exit 1; }
      DEPLOY_DIR="$2"
      shift 2
      ;;
    --pull)
      DO_PULL=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      err "未知参数: $1"
      usage
      exit 1
      ;;
  esac
done

resolve_deploy_dir() {
  if [[ -n "$DEPLOY_DIR" ]]; then
    DEPLOY_DIR="$(cd "$DEPLOY_DIR" 2>/dev/null && pwd)" || {
      err "目录不存在: $DEPLOY_DIR"
      exit 1
    }
    return 0
  fi
  if [[ -f "./${COMPOSE_FILE}" ]]; then
    DEPLOY_DIR="$(pwd)"
    return 0
  fi
  local script_dir
  script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-.}")" 2>/dev/null && pwd || pwd)"
  if [[ -f "${script_dir}/../${COMPOSE_FILE}" ]]; then
    DEPLOY_DIR="$(cd "${script_dir}/.." && pwd)"
    return 0
  fi
  err "未找到 ${COMPOSE_FILE}，请用 --dir 指定部署目录"
  exit 1
}

# Docker 需连 daemon；普通用户无权限时自动加 sudo
docker_ok() {
  docker info >/dev/null 2>&1
}

find_compose_cmd() {
  local prefix=()
  if ! docker_ok; then
    if command -v sudo >/dev/null 2>&1 && sudo -n docker info >/dev/null 2>&1; then
      prefix=(sudo)
      log "当前用户无 Docker 权限，使用 sudo"
    elif command -v sudo >/dev/null 2>&1; then
      prefix=(sudo)
      log "当前用户无 Docker 权限，将使用 sudo（可能提示输入密码）"
    else
      err "无法连接 Docker（permission denied）。请用 root，或: sudo usermod -aG docker \$USER 后重新登录"
      exit 1
    fi
  fi

  if "${prefix[@]}" docker compose version >/dev/null 2>&1; then
    COMPOSE=("${prefix[@]}" docker compose)
    return 0
  fi
  if command -v docker-compose >/dev/null 2>&1; then
    COMPOSE=("${prefix[@]}" docker-compose)
    return 0
  fi
  err "未找到 docker compose 或 docker-compose"
  exit 1
}

# 仅解析 volumes: 下的 bind mount，避免把 ports / devices / command 误当成目录
ensure_bind_mount_dirs() {
  local compose_path="$1"
  local host

  while IFS= read -r host; do
    [[ -n "$host" ]] || continue
    case "$host" in
      /dev/*|/etc/machine-id|/proc/*|/sys/*) continue ;;
    esac
    if [[ "$host" != /* ]]; then
      host="${DEPLOY_DIR}/${host#./}"
    fi
    if [[ ! -d "$host" ]]; then
      log "创建目录: $host"
      mkdir -p "$host"
    fi
  done < <(
    awk '
      /^[[:space:]]*#/ { next }
      /^[[:space:]]+volumes:[[:space:]]*$/ { v=1; next }
      # 同级其它 key 结束 volumes 段
      /^[[:space:]]+[a-zA-Z_][a-zA-Z0-9_]*:[[:space:]]*/ {
        if ($0 !~ /^[[:space:]]+volumes:/) v=0
      }
      v && /^[[:space:]]+-[[:space:]]+/ {
        line=$0
        sub(/^[[:space:]]+-[[:space:]]+/, "", line)
        sub(/[[:space:]]+#.*$/, "", line)
        gsub(/^["'\'']|["'\'']$/, "", line)
        # host:container[:ro]
        n = split(line, a, ":")
        if (n < 2) next
        h = a[1]
        # 跳过纯数字端口映射、空路径
        if (h ~ /^[0-9]+$/ || h == "") next
        print h
      }
    ' "$compose_path"
  )
}

compose_profiles() {
  case "$MODE" in
    console|all) printf '%s\n' console ;;
  esac
}

main() {
  resolve_deploy_dir
  find_compose_cmd

  local compose_path="${DEPLOY_DIR}/${COMPOSE_FILE}"
  [[ -f "$compose_path" ]] || {
    err "缺少文件: $compose_path"
    exit 1
  }

  log "部署目录: ${DEPLOY_DIR}"
  ensure_bind_mount_dirs "$compose_path"

  local -a args=(-f "$compose_path")
  local profile

  while IFS= read -r profile; do
    [[ -n "$profile" ]] || continue
    args+=(--profile "$profile")
  done < <(compose_profiles)

  cd "$DEPLOY_DIR"

  if [[ "$DO_PULL" -eq 1 ]]; then
    log "拉取镜像..."
    "${COMPOSE[@]}" "${args[@]}" pull
  fi

  case "$MODE" in
    node)
      log "启动 node..."
      "${COMPOSE[@]}" "${args[@]}" up -d node
      ;;
    console)
      log "启动 console..."
      "${COMPOSE[@]}" "${args[@]}" up -d console
      ;;
    all)
      log "启动 console + node..."
      "${COMPOSE[@]}" "${args[@]}" up -d
      ;;
    *)
      err "未知模式: $MODE"
      usage
      exit 1
      ;;
  esac

  log "完成。查看状态: cd ${DEPLOY_DIR} && ${COMPOSE[*]} ${args[*]} ps"
}

main
