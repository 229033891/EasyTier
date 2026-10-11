#!/usr/bin/env bash
# Remove leftover EasyTier GUI files/units from the easytier-gui -> ET rename (Linux).
#
# Safe cleanup after upgrading to mainBinaryName=ET:
#   - stop legacy processes named easytier-gui
#   - remove / rewrite systemd units that still ExecStart=.../easytier-gui
#   - delete easytier-gui binary when ET exists beside it (or on PATH)
#   - optional removal of legacy desktop entries pointing at easytier-gui
#
# Does NOT uninstall ET, delete the ET binary, or remove user config.
#
# Usage:
#   sudo bash script/cleanup-legacy-gui.sh              # dry-run
#   sudo bash script/cleanup-legacy-gui.sh --apply
#   sudo bash script/cleanup-legacy-gui.sh --apply --also-remove-shim
set -euo pipefail

APPLY=0
ALSO_REMOVE_SHIM=0
EXTRA_DIRS=()

usage() {
  cat <<'EOF'
Usage: cleanup-legacy-gui.sh [--apply] [--also-remove-shim] [--dir DIR]...

  (default)           dry-run: print actions only
  --apply             perform changes (prefer root for systemd / /usr)
  --also-remove-shim  delete easytier-gui when ET is present and units are migrated
  --dir DIR           extra install directory to scan
  -h, --help          show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --apply) APPLY=1; shift ;;
    --also-remove-shim) ALSO_REMOVE_SHIM=1; shift ;;
    --dir)
      [[ $# -ge 2 ]] || { echo "missing value for --dir" >&2; exit 2; }
      EXTRA_DIRS+=("$2"); shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown arg: $1" >&2; usage; exit 2 ;;
  esac
done

prefix() { if [[ "$APPLY" -eq 1 ]]; then echo "[apply]"; else echo "[dry-run]"; fi; }
log() { printf '%s %s\n' "$(prefix)" "$*"; }
step() { printf '\n==> %s\n' "$*"; }

run() {
  if [[ "$APPLY" -eq 1 ]]; then
    "$@"
  else
    log "would run: $*"
  fi
}

is_root() { [[ "$(id -u)" -eq 0 ]]; }

candidate_dirs() {
  local d
  for d in \
    /opt/ET \
    /opt/easytier \
    /opt/easytier-gui \
    /usr/lib/ET \
    /usr/lib/easytier-gui \
    /usr/local/lib/ET \
    "$HOME/.local/share/ET" \
    "$HOME/ET"
  do
    [[ -d "$d" ]] && printf '%s\n' "$d"
  done
  local extra
  for extra in "${EXTRA_DIRS[@]+"${EXTRA_DIRS[@]}"}"; do
    [[ -d "$extra" ]] && printf '%s\n' "$extra"
  done
}

step "Stop legacy GUI processes (easytier-gui)"
if pgrep -x easytier-gui >/dev/null 2>&1; then
  log "killall easytier-gui"
  run killall -TERM easytier-gui 2>/dev/null || true
  sleep 1
  run killall -KILL easytier-gui 2>/dev/null || true
else
  log "no easytier-gui process"
fi

step "Inspect / migrate systemd units mentioning easytier-gui"
UNIT_MIG_OK=1
mapfile -t UNIT_FILES < <(
  {
    systemctl list-unit-files --type=service --no-legend 2>/dev/null | awk '{print $1}'
    ls /etc/systemd/system/*.service /usr/lib/systemd/system/*.service \
      /lib/systemd/system/*.service 2>/dev/null || true
  } | sed 's#.*/##' | sort -u | grep -Ei 'et-gui|easytier-gui|^et\.service$' || true
)

# Also grep any unit whose ExecStart still points at easytier-gui
mapfile -t GREPPED < <(
  grep -Rsl --include='*.service' -E 'easytier-gui' \
    /etc/systemd/system /usr/lib/systemd/system /lib/systemd/system \
    2>/dev/null || true
)

declare -A SEEN=()
for u in "${UNIT_FILES[@]+"${UNIT_FILES[@]}"}" "${GREPPED[@]+"${GREPPED[@]}"}"; do
  [[ -n "${u:-}" ]] || continue
  base="$(basename "$u")"
  [[ -n "${SEEN[$base]:-}" ]] && continue
  SEEN[$base]=1

  unit_path=""
  for cand in "/etc/systemd/system/$base" "/usr/lib/systemd/system/$base" "/lib/systemd/system/$base" "$u"; do
    if [[ -f "$cand" ]]; then unit_path="$cand"; break; fi
  done
  if [[ -z "$unit_path" ]]; then
    log "unit $base: file not found (skip)"
    continue
  fi

  if ! grep -Eq 'easytier-gui' "$unit_path"; then
    log "unit $base: no easytier-gui reference"
    continue
  fi

  # Legacy-named unit: remove entirely if current ET-Gui style unit exists or binary is ET
  if [[ "$base" =~ [Ee]asytier-gui ]]; then
    log "disable/remove legacy unit $base ($unit_path)"
    if [[ "$APPLY" -eq 1 ]]; then
      systemctl stop "$base" 2>/dev/null || true
      systemctl disable "$base" 2>/dev/null || true
      if [[ "$unit_path" == /etc/systemd/system/* ]]; then
        rm -f "$unit_path"
      else
        log "packaged unit $unit_path — masked instead of deleting"
        systemctl mask "$base" 2>/dev/null || true
      fi
      systemctl daemon-reload 2>/dev/null || true
    fi
    continue
  fi

  # Current unit still ExecStart=.../easytier-gui → rewrite to ET
  if grep -Eq 'ExecStart=.*easytier-gui' "$unit_path"; then
    log "rewrite ExecStart in $unit_path (easytier-gui -> ET)"
    if [[ "$APPLY" -eq 1 ]]; then
      if [[ ! -w "$unit_path" ]] && ! is_root; then
        log "ERROR: cannot write $unit_path (re-run as root)"
        UNIT_MIG_OK=0
        continue
      fi
      # Prefer drop-in override under /etc so packaged units survive package upgrades
      drop_dir="/etc/systemd/system/${base}.d"
      mkdir -p "$drop_dir"
      # Extract original ExecStart and rewrite
      old_exec="$(grep -E '^ExecStart=' "$unit_path" | head -n1 | sed 's/^ExecStart=//')"
      new_exec="$(printf '%s\n' "$old_exec" | sed -E 's/(^|[[:space:]/])easytier-gui([.exe]*)(\s|$)/\1ET\3/g')"
      cat >"$drop_dir/legacy-bin-rename.conf" <<EOF
[Service]
ExecStart=
ExecStart=${new_exec}
EOF
      systemctl daemon-reload
      log "wrote $drop_dir/legacy-bin-rename.conf"
    else
      UNIT_MIG_OK=0
    fi
  fi
done

step "Remove legacy binaries"
PATH_CANDIDATES=()
while IFS= read -r d; do PATH_CANDIDATES+=("$d"); done < <(candidate_dirs)
# Common bindir locations
for d in /usr/bin /usr/local/bin /usr/sbin /usr/local/sbin; do
  [[ -e "$d/easytier-gui" || -e "$d/ET" ]] && PATH_CANDIDATES+=("$d")
done

declare -A DIR_SEEN=()
for dir in "${PATH_CANDIDATES[@]+"${PATH_CANDIDATES[@]}"}"; do
  [[ -n "${DIR_SEEN[$dir]:-}" ]] && continue
  DIR_SEEN[$dir]=1
  printf '  scan: %s\n' "$dir"

  legacy="$dir/easytier-gui"
  et="$dir/ET"
  bak="$dir/easytier-gui.bak"

  if [[ -e "$bak" ]]; then
    log "delete $bak"
    run rm -f "$bak"
  fi

  if [[ ! -e "$legacy" ]]; then
    continue
  fi

  if [[ ! -e "$et" ]]; then
    log "keep $legacy (ET missing beside it — not a post-rename install)"
    continue
  fi

  if [[ "$ALSO_REMOVE_SHIM" -ne 1 ]]; then
    log "found shim $legacy (pass --also-remove-shim to delete when safe)"
    continue
  fi

  if [[ "$UNIT_MIG_OK" -ne 1 && "$APPLY" -eq 1 ]]; then
    log "ERROR: skip deleting $legacy — unit migration incomplete"
    continue
  fi

  log "delete legacy shim $legacy"
  run rm -f "$legacy"
done

step "Desktop entries targeting easytier-gui"
mapfile -t DESKTOPS < <(
  grep -Rsl --include='*.desktop' -E 'easytier-gui' \
    /usr/share/applications /usr/local/share/applications \
    "$HOME/.local/share/applications" 2>/dev/null || true
)
for desk in "${DESKTOPS[@]+"${DESKTOPS[@]}"}"; do
  [[ -f "$desk" ]] || continue
  if grep -Eq '^Exec=.*easytier-gui' "$desk"; then
    log "rewrite/remove desktop entry $desk"
    if [[ "$APPLY" -eq 1 ]]; then
      if grep -Eq 'Name=.*ET|easytier-gui' "$desk"; then
        # Prefer rewrite Exec to ET when Name looks like our app
        sed -i -E 's#(^Exec=.*[[:space:]/])easytier-gui([.exe]*)#\1ET#g' "$desk" || true
        if grep -Eq 'Exec=.*easytier-gui' "$desk"; then
          rm -f "$desk"
        fi
      else
        rm -f "$desk"
      fi
    fi
  fi
done

step "Done"
if [[ "$APPLY" -ne 1 ]]; then
  echo "No changes were made. Example:"
  echo "  sudo bash script/cleanup-legacy-gui.sh --apply"
  echo "  sudo bash script/cleanup-legacy-gui.sh --apply --also-remove-shim"
else
  echo "Finished. If the GUI service fails, open ET once and re-enable Service mode."
fi
