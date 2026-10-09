# EasyTier 长期约定（D:\EasyTier，改动一律在 dev 分支）

> **详细笔记见同目录 `DETAILS.md`**（按主题分节，编号与本文件指针一致）。本文件受注入长度限制，只留「每次都要遵守的硬规则」+ 高频坑一句话版；需要细节时读 DETAILS.md 对应小节。

## 硬性禁止（先看）
- **不主动打包编译**（exe 老大自己出）。本机 Rust 编译不可用：`cargo check/build` 挂 C 依赖（`windivert-sys`/`zstd-sys`/`ring` 的 `cl.exe` 退出码 2）；`cargo test` 能链接但启动 `STATUS_DLL_NOT_FOUND`。Rust 改动只**静态核对**（读代码/对签名/查调用点）；语法用 `rustup run 1.95.0 rustfmt --edition 2024 --emit stdout <f>`。`#[cfg(mobile)]` 本地覆盖不到。
- **能跑的验证只有前端**：`easytier-gui` → `./node_modules/.bin/{vue-tsc --noEmit|vitest run <f>|eslint . --ignore-pattern src-tauri}`；`easytier-web/frontend-lib` → `./node_modules/.bin/{vue-tsc --noEmit|vitest run}`（无 eslint 配置）。基线 10-09：gui eslint/vue-tsc 0、vitest 28；frontend-lib vitest 114 passed(15 文件)、vue-tsc 0。`pnpm` 在 Git Bash 不可用（corepack 拼错路径）→ 一律 `node_modules/.bin/<tool>`。
- 文档链接检查：`python ~/.workbuddy-ai/skills/easytier-docs-consistency/scripts/check_links.py docs`。
- 改完先 `git branch --show-current`；**不主动 commit / 不主动跑 cargo**，等老大确认。

## 高频坑（一句话版 → 细节见 DETAILS.md）
- **locales `*.yaml` 的值里出现 `: `（冒号+空格）→ js-yaml 解析失败 → `vite build` 与所有 import `i18n.ts` 的测试全挂**。改文案必复验。→ §1
- `vite build` 会被沙箱删除护栏挡（`emptyDir` 的 `rmSync`→`genie-trash ETIMEDOUT`，在 "N modules transformed" 之后才炸，**不是**编译失败）：先 `mv dist $TEMP/xxx` 再 build，**同一命令别带 `rm -rf`**。→ §1
- `eslint --fix` 会把 `vue-router/auto` 与 `vue-router/auto-routes` 的 import 误合（已对 `src/main.ts` 关 `import/no-duplicates`）→ 跑完必须核对 import 说明符集合。→ §1
- `frontend-lib` 跑 `vue-tsc` 前 `dist` 要最新（gitignore 易过期 → 假错「`Ipv6Inet` 不存在」）。→ §1
- `easytier-gui` 的 `build` 已含 `lint` → 本地 `pnpm build`/`tauri build` 与 CI `android.yml`/`windows.yml` 都会跑 lint。→ §1
- MagicDNS fake IP `10.255.255.254` 有**一长串硬编码副本**；静态 hosts 通配是 **RFC 4592 多级**，zone 必须 ≥2 标签、不得占用 route zone / MagicDNS TLD。→ §2
- 默认 `default_protocol=tcp` 会把手动配的 `udp://` peer **改写成 `tcp://` 并优先用**（→ TCP-over-TCP 卡）；把配置读取顺序改成 storage-first 会让 Windows 适配器名不再持久化。→ §6
- 沙箱下 `git` 改状态可能 exit 0 但静默不生效 → **凡改 git 状态必须回读**。→ §7

## 权威清单（别在此重复）
- 版本号 bump：`docs/ops/release-version-bump.md`（10 文件 / 17 处；`easytier-contrib/easytier-mini/Cargo.toml` 易漏）
- OpenWrt 打包：`.github/workflows/openwrt.yml`（只出 apk / 25.12+；bump 版本只改 `easytier/Cargo.toml`）
- 现状 SoT 在 `docs/current/`，决策留档在 `docs/roadmap/`
