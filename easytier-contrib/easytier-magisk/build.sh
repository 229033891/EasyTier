#!/bin/sh
WEBUI_SOURCE="../../easytier-web/config-generator/dist"

if [ -f "${WEBUI_SOURCE}/index.html" ]; then
    rm -rf ./webroot
    mkdir -p ./webroot
    cp -R "${WEBUI_SOURCE}/." ./webroot/
elif [ ! -f "./webroot/index.html" ]; then
    echo "Error: WebUI 构建产物不存在，请先运行 pnpm --dir ../../easytier-web/config-generator build."
    exit 1
fi

version=$(grep '^version =' ../../easytier/Cargo.toml | cut -d '"' -f 2)

if [ -z "$version" ]; then
    echo "Error: 版本号不存在."
    exit 1
fi
version="v${version}"

filename="easytier_magisk_${version}.zip"
echo "${version}"

if [ ! -f "./ET-core" ] || [ ! -f "./ET-cli" ] || [ ! -f "./ET-web-embed" ]; then
    GITHUB_REPO="${GITHUB_REPO:-229033891/EasyTier}"
    wget -O "easytier_last.zip" "https://github.com/${GITHUB_REPO}/releases/download/${version}/ET-linux-aarch64-${version}.zip"
    unzip -o easytier_last.zip -d ./
    mv ./ET-linux-aarch64/* ./
    rm -rf ./easytier_last.zip
    rm -rf ./ET-linux-aarch64
fi

zip -r -o -X "${filename}" ./ -x '.git/*' -x '.github/*' -x 'folder/*' -x 'build.sh' -x 'magisk_update.json'