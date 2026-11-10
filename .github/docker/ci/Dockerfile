# syntax=docker/dockerfile:1
#
# EasyTier CI 构建环境（Linux）。
#
# 目的：把 `.github/actions/prepare-build` 在**每个 job 每次运行**里都要做的事
# 提前烘进镜像——
#   1. apt 装 mold / musl-tools / build-essential
#   2. apt 装 webkit2gtk / gtk3 / librsvg / xdotool 等 GUI 依赖
#   3. 下载并安装 Rust 1.95 + musl/wasm target
#   4. 下载并安装 protoc 35.1
#
# 这四件事在 Ubuntu runner 上是 3–5 分钟，其中前两件要碰 apt 索引——而
# `apt-get update` 正是 ET Linux #169 僵死 4h41m 的那条路径（详见
# docs/ops/ci-failure-log.md）。进镜像后这些全部消失，环境准备降到秒级。
#
# 构建/推送由 `.github/workflows/ci-image.yml` 负责；本文件变更会自动触发重建。
# 使用方式见 `.github/workflows/linux.yml` 各 job 的 `container:` 与 docs 说明。

FROM ubuntu:24.04

ARG RUST_VERSION=1.95
ARG PROTOC_VERSION=35.1

# 镜像构建期间同样可能撞上半开连接，沿用与 CI 一致的四项防护。
ARG APT_OPTS="-o Acquire::ForceIPv4=true -o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30"

ENV DEBIAN_FRONTEND=noninteractive \
    LANG=C.UTF-8 \
    LC_ALL=C.UTF-8 \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    CARGO_TERM_COLOR=always \
    # /root/.cargo/bin、/root/.local/bin：部分 action（sccache 等）把二进制装这里
    PATH=/usr/local/cargo/bin:/root/.cargo/bin:/root/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# 分组说明（对应原 workflow 里的三处安装）：
#   build-essential mold musl-tools        ← prepare-build「Install dependencies」
#   xdg-utils … libxdo-dev                 ← prepare-build「Install GUI dependencies」
#   pkg-config libssl-dev clang libclang-dev  ← openssl-sys / bindgen 类 -sys crate
#   cmake perl nasm autoconf automake libtool ← aws-lc-sys（构建脚本用 cmake）、
#                                             jemalloc / ring 等原生依赖；runner
#                                             镜像自带这些，镜像里必须补齐
#   libsqlite3-dev zlib1g-dev              ← libsqlite3-sys / libz-sys 的非 bundled 路径
#   dpkg-dev fakeroot binutils             ← tauri 打 .deb
#   git curl wget xz-utils unzip jq file   ← checkout / cache / install-protoc / install-upx
#   sudo                                   ← ubuntu 镜像不自带；装上后 prepare-build 的
#                                            回退分支（sudo apt-get …）可原样工作
RUN apt-get update ${APT_OPTS} \
 && apt-get install -y --no-install-recommends ${APT_OPTS} \
        build-essential mold musl-tools \
        pkg-config libssl-dev clang libclang-dev \
        cmake perl nasm autoconf automake libtool \
        libsqlite3-dev zlib1g-dev \
        xdg-utils libappindicator3-dev libgtk-3-dev librsvg2-dev \
        libwebkit2gtk-4.1-dev libxdo-dev \
        ca-certificates curl wget xz-utils unzip git sudo jq file \
        dpkg-dev fakeroot binutils \
 && rm -rf /var/lib/apt/lists/*

# Rust：与 rust-toolchain.toml 对齐（channel = 1.95）。
# target 覆盖 linux.yml 需要的 musl（无头节点 / web-embed）、prepare-pnpm 需要的
# wasm32-unknown-unknown，以及 test.yml 的 check-wasi（wasm32-wasip1）。
# components 是 `--profile minimal` 不自带的：check-fmt 要 rustfmt，check-clippy
# 要 clippy。装进镜像后 setup-rust-toolchain 变成空操作，不用再连 static.rust-lang.org。
RUN set -eux; \
    curl -fsSL --retry 3 https://sh.rustup.rs -o /tmp/rustup-init.sh; \
    sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal \
        --default-toolchain "${RUST_VERSION}"; \
    rm -f /tmp/rustup-init.sh; \
    rustup target add --toolchain "${RUST_VERSION}" \
        x86_64-unknown-linux-musl \
        aarch64-unknown-linux-musl \
        wasm32-unknown-unknown \
        wasm32-wasip1; \
    rustup component add --toolchain "${RUST_VERSION}" rustfmt clippy; \
    rustc -V; cargo -V; cargo fmt --version; cargo clippy --version

# protoc：版本与 prepare-build / prepare-pnpm 的 PROTOC_VERSION 保持一致，
# 安装到 /usr/local/bin，使 script/install-protoc.sh 的「PATH 命中即复用」分支生效。
RUN set -eux; \
    curl -fsSL --retry 3 -o /tmp/protoc.zip \
        "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-x86_64.zip"; \
    unzip -oq /tmp/protoc.zip -d /usr/local; \
    chmod +x /usr/local/bin/protoc; \
    rm -f /tmp/protoc.zip; \
    protoc --version

# 哨兵文件：prepare-build 靠它判断「依赖已预装，跳过一切 apt」。
# 没有这个文件时，prepare-build 走原来的加固 apt 路径（普通 runner 场景）。
RUN printf '{"image":"easytier-ci","rust":"%s","protoc":"%s","created":"%s"}\n' \
        "${RUST_VERSION}" "${PROTOC_VERSION}" "$(date -u +%FT%TZ)" \
        > /etc/easytier-ci-image

# 自检：任何一个关键工具缺失都让镜像构建直接失败，而不是等到 CI 里才暴露。
RUN set -eux; \
    for b in gcc g++ make cmake mold musl-gcc pkg-config protoc git sudo curl unzip; do \
        command -v "$b" >/dev/null || { echo "missing: $b" >&2; exit 1; }; \
    done; \
    test -f /usr/local/cargo/bin/rustc; \
    rustup target list --installed | grep -qx 'x86_64-unknown-linux-musl'; \
    echo "ci image ok: $(cat /etc/easytier-ci-image)"
