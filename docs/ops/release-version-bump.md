# EasyTier 版本号 bump 清单

## Status

- Status: **Ops**
- 最近审阅：2026-10-08
- 适用范围：发版前把项目版本号从 `X.Y.Z` 改到 `X.Y.(Z+1)` 时，**要改哪些文件的哪些位置**
- 目的：**照单执行即可，不用再翻代码找版本号**；也用于回答"这次 bump 漏了什么"
- 索引：[`../README.md`](../README.md)
- 配套：[`web-build-deploy.md`](./web-build-deploy.md)（构建 / 打包）、[`../roadmap/github-release-install.md`](../roadmap/github-release-install.md)（发版与安装）

## TL;DR

一次 bump = **改 10 个文件**：根 `Cargo.toml` **3 处** + 根 `Cargo.lock` **6 处** + 其余 8 个文件各 **1 处** = **17 处**。

## 必改清单

| # | 文件 | 位置 | 处数 |
|---|------|------|------|
| 1 | `Cargo.toml` | `[workspace.dependencies]` 里 `easytier` / `easytier-core` / `easytier-proto` 三条（约 L41–46） | 3 |
| 2 | `Cargo.lock` | 6 个 workspace 包的 `version = "…"`：`easytier` / `easytier-core` / `easytier-gui` / `easytier-mini` / `easytier-proto` / `easytier-web` | 6 |
| 3 | `easytier/Cargo.toml` | `[package] version`（约 L6） | 1 |
| 4 | `easytier-core/Cargo.toml` | `[package] version`（约 L6） | 1 |
| 5 | `easytier-proto/Cargo.toml` | `[package] version`（约 L6） | 1 |
| 6 | `easytier-web/Cargo.toml` | `[package] version`（约 L3） | 1 |
| 7 | `easytier-contrib/easytier-mini/Cargo.toml` | `[package] version`（约 L4） | 1 |
| 8 | `easytier-gui/package.json` | 顶层 `"version"`（约 L4） | 1 |
| 9 | `easytier-gui/src-tauri/Cargo.toml` | `[package] version`（约 L3） | 1 |
| 10 | `easytier-gui/src-tauri/tauri.conf.json` | 顶层 `"version"`（约 L26） | 1 |

行号会随代码漂移，**以 `grep` 为准**（见「校验」一节）。

几个要点：

- **根 `Cargo.toml` 只改 `version`，不要动同一行里的 `path` / `default-features`。**
- **全仓只有根目录一个 `Cargo.lock`**（`git ls-files | grep Cargo.lock` 只有一条），子目录没有各自的锁文件。
- **第 7 项（`easytier-mini`）是最容易漏的一项**：`2.7.41` / `2.7.42` / `2.7.43` 连续三次 bump 都漏了它，导致它一直停在 `2.7.4`、`easytier-mini --version` 打印错误版本（`2.7.44` 那次已补上）。它是 workspace 成员，但**没有任何 CI workflow 引用它的版本**——漏了不会让 CI 变红，只会让版本号自相矛盾。
- `easytier/Cargo.toml`（第 3 项）**必须改**：OpenWrt 打包 workflow 从这里读版本（见下「不要改的地方」）。

## 不要改的地方

| 位置 | 当前值 | 为什么不动 |
|------|--------|-----------|
| `easytier-contrib/easytier-ffi`、`easytier-android-jni`、`easytier-ios`、`easytier-uptime`、`easytier-ohrs*` 的 `Cargo.toml` | `0.1.0` | 独立版本线，历史上从不跟主版本走 |
| `tauri-plugin-vpnservice/Cargo.toml` | `0.0.0` | 同上 |
| `easytier-web/frontend/package.json`、`easytier-web/frontend-lib/package.json` | `0.0.0` | `private: true` 的私有包，不参与发版 |
| `easytier-js/package.json` | `0.1.0` | 独立版本线 |
| `easytier-gui/package.json` 里的 `"@types/node": "^22.7.4"` | — | 第三方依赖版本，**不是项目版本**。用 `grep 2.7.4` 搜残留会命中它，别手抖改掉 |
| `.github/workflows/docker.yml` 的 `image_tag.default: 'v2.7.2'` | `v2.7.2` | 手动 dispatch 的输入默认值，历史 bump 从未同步（**已知陈旧**，要修另开一单） |
| `.github/workflows/openwrt.yml` 注释里的 `2.7.4` / `2.6.4` | — | 只是注释示例。该 workflow **动态**从 `easytier-src/easytier/Cargo.toml` 读版本（L178、L271），所以改第 3 项就够了 |

## 校验

### 1) 残留检查（**注意正则边界**）

```bash
# 把 2.7.4 换成上一个版本号
grep -rnE '2\.7\.4([^0-9]|$)' \
  Cargo.toml Cargo.lock \
  easytier/Cargo.toml easytier-core/Cargo.toml easytier-proto/Cargo.toml \
  easytier-web/Cargo.toml easytier-contrib/easytier-mini/Cargo.toml \
  easytier-gui/package.json easytier-gui/src-tauri/Cargo.toml \
  easytier-gui/src-tauri/tauri.conf.json
```

- **`2.7.4` 是 `2.7.44` 的前缀**，必须写成 `2\.7\.4([^0-9]|$)`；否则新版本号自己也会被搜出来，旧新互相误报。
- 期望结果：**只剩** `easytier-gui/package.json` 的 `"@types/node": "^22.7.4"`（无关依赖）。

### 2) cargo 认锁文件 + 六个包版本一致

```bash
cargo metadata --no-deps --offline --format-version 1 | python -c "import sys,json; d=json.load(sys.stdin); [print(f\"{p['name']:<22} {p['version']}\") for p in sorted(d['packages'], key=lambda x: x['name'])]"
```

期望：`easytier` / `easytier-core` / `easytier-gui` / `easytier-mini` / `easytier-proto` / `easytier-web` **六个包同一版本**；其余为 `0.1.0`（`tauri-plugin-vpnservice` 为 `0.0.0`）。这一步同时兜住"`Cargo.lock` 只改了一半"。

### 3) JSON 仍可解析

```bash
node -e "for (const f of ['easytier-gui/package.json','easytier-gui/src-tauri/tauri.conf.json']) JSON.parse(require('fs').readFileSync(f,'utf8'))"
```

### 4) **不要**用本地编译"验证"版本号

版本号是纯文本替换，上面三步足够。**不要**为了验证跑 `cargo build` / `cargo check` —— 本机 C 依赖编不过，且耗时很长（见 [`web-build-deploy.md` 第 0 节](./web-build-deploy.md#0-️-重要不要默认直接生成-exe)）。真正的编译验证交给 CI。

## 发布分支惯例

- release 分支命名 `releases/vX.Y.Z`，**从 `dev` 上的 bump 提交切出**，即分支 tip == `chore(release): bump package version to X.Y.Z`。
  - 已核对：`origin/releases/v2.7.43` = `8b4838ef`（2.7.43 的 bump 提交）、`releases/v2.7.42` = `f428ca86`（2.7.42 的 bump 提交）。
- 切出后 `dev` 继续前进，与 release 分支自然分叉；后续把 `dev` 合回 `releases/vX.Y.Z`，用于把发布线上的修复带过去。
- 发版走 [`release.yml`](../../.github/workflows/release.yml)，输入 `source_branch: releases/vX.Y.Z`。

## 历史坑

1. **`easytier-mini` 漏改**（2.7.41–2.7.43 连续三次）：CI 不会红，只有 `easytier-mini --version` 会打印旧版本。
2. **用不带边界的 `grep 2.7.4` 搜残留**：会同时命中 `2.7.44` 自己、`@types/node: ^22.7.4`、workflow 注释 → 必须带 `([^0-9]|$)`。
3. **`Cargo.lock` 只改一半**（漏 `easytier-gui` / `easytier-mini`）→ 用上面第 2 步的 `cargo metadata` 兜底。
