#!/usr/bin/env bash
# Install a pinned protoc release without arduino/setup-protoc (Node 20).
set -euo pipefail

PROTOC_VERSION="${PROTOC_VERSION:-35.1}"
INSTALL_DIR="${PROTOC_INSTALL_DIR:-${RUNNER_TEMP:-/tmp}/protoc-${PROTOC_VERSION}}"

protoc_bin_name="protoc"
uname_s="$(uname -s)"
uname_m="$(uname -m)"
case "${uname_s}" in
  Linux)
    case "${uname_m}" in
      x86_64|amd64) platform="linux-x86_64" ;;
      aarch64|arm64) platform="linux-aarch_64" ;;
      *)
        echo "unsupported Linux arch: ${uname_m}" >&2
        exit 1
        ;;
    esac
    ;;
  Darwin)
    case "${uname_m}" in
      x86_64) platform="osx-x86_64" ;;
      arm64) platform="osx-aarch_64" ;;
      *)
        echo "unsupported macOS arch: ${uname_m}" >&2
        exit 1
        ;;
    esac
    ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    protoc_bin_name="protoc.exe"
    case "${uname_m}" in
      x86_64|amd64|AMD64) platform="win64" ;;
      *)
        echo "unsupported Windows arch: ${uname_m}" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    echo "unsupported OS: ${uname_s}" >&2
    exit 1
    ;;
esac

bin_dir="${INSTALL_DIR}/bin"
protoc_bin="${bin_dir}/${protoc_bin_name}"

already_ok() {
  local candidate="$1"
  if [[ ! -x "${candidate}" ]] && [[ ! -f "${candidate}" ]]; then
    return 1
  fi
  local version
  version="$("${candidate}" --version | tr -d '\r')"
  [[ "${version}" = "libprotoc ${PROTOC_VERSION}" ]]
}

# Same job may call this twice (prepare-build + prepare-pnpm). Prefer PATH hit.
if command -v protoc >/dev/null 2>&1 && already_ok "$(command -v protoc)"; then
  echo "protoc ${PROTOC_VERSION} already available on PATH"
  exit 0
fi

# Cache hit: restore from INSTALL_DIR without re-downloading.
if already_ok "${protoc_bin}"; then
  echo "${bin_dir}" >> "${GITHUB_PATH:?}"
  export PATH="${bin_dir}:${PATH}"
  echo "Reused cached protoc ${PROTOC_VERSION} from ${INSTALL_DIR}"
  exit 0
fi

archive="protoc-${PROTOC_VERSION}-${platform}.zip"
url="https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/${archive}"

mkdir -p "${INSTALL_DIR}"
tmp_zip="$(mktemp)"
curl -fsSL --retry 3 -o "${tmp_zip}" "${url}"
# -o: overwrite without prompting (needed on Windows runners / busy temp dirs)
unzip -oq "${tmp_zip}" -d "${INSTALL_DIR}"
rm -f "${tmp_zip}"

echo "${bin_dir}" >> "${GITHUB_PATH:?}"
export PATH="${bin_dir}:${PATH}"

version="$(protoc --version | tr -d '\r')"
test "${version}" = "libprotoc ${PROTOC_VERSION}"
echo "Installed ${version} from ${url}"
