#!/usr/bin/env bash
# Install a pinned UPX release into a cacheable directory.
set -euo pipefail

UPX_VERSION="${UPX_VERSION:-4.2.4}"
INSTALL_DIR="${UPX_INSTALL_DIR:-${RUNNER_TEMP:-/tmp}/upx-${UPX_VERSION}}"

uname_s="$(uname -s)"
uname_m="$(uname -m)"
case "${uname_s}" in
  Linux)
    case "${uname_m}" in
      x86_64|amd64) platform="amd64_linux" ;;
      aarch64|arm64) platform="arm64_linux" ;;
      *)
        echo "unsupported Linux arch: ${uname_m}" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    echo "unsupported OS for UPX helper: ${uname_s}" >&2
    exit 1
    ;;
esac

upx_bin="${INSTALL_DIR}/upx"
if [[ -x "${upx_bin}" ]]; then
  echo "UPX ${UPX_VERSION} already present at ${upx_bin}"
else
  pkg="upx-${UPX_VERSION}-${platform}"
  url="https://github.com/upx/upx/releases/download/v${UPX_VERSION}/${pkg}.tar.xz"
  mkdir -p "${INSTALL_DIR}"
  tmp_dir="$(mktemp -d)"
  curl -fsSL --retry 3 "${url}" | tar -xJ -C "${tmp_dir}"
  cp "${tmp_dir}/${pkg}/upx" "${upx_bin}"
  chmod +x "${upx_bin}"
  rm -rf "${tmp_dir}"
  echo "Installed UPX ${UPX_VERSION} from ${url}"
fi

echo "${INSTALL_DIR}" >> "${GITHUB_PATH:?}"
export PATH="${INSTALL_DIR}:${PATH}"
upx --version | head -n 1
