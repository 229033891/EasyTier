# EasyTier 项目长期约定

## easytier-web 打包 / 生成 exe 规则（强制）

**生成可部署产物前必须先询问用户，得到明确确认后才允许编译。**

- 不要主动执行 `cargo build` / `cargo build --release` 生成 `easytier-web.exe` 或 embed 包。
- 全量编译 + LTO 链接耗时很长（数分钟起），未经确认直接开编会浪费大量时间。
- 正确顺序：
  1. 先问用户**是否需要现在生成** exe / 可部署产物；
  2. 问清**平台**（Windows / Linux）、**用途**（本地试跑 / 正式发版）、**是否要 embed 一体包**；
  3. 用户明确同意后，再按 `docs/easytier-web-build-and-deploy.md` 构建；日常迭代优先 `release-fast`，不要默认 `--release`。
- 仅改代码、排查问题、看类型或构建配置时，**不要顺手触发全量编译**。
- 只有用户明确说「打包 / 生成 exe / 出可部署产物」时才编译。

详细步骤与说明见 `docs/easytier-web-build-and-deploy.md` 第 0 节。

## 环境备注

- 本机沙箱默认拒绝写 `D:\EasyTier\target`，`cargo check/build` 会因无法写入 `target/debug/build/aws-lc-sys-*` 等而失败（aws-lc-sys 需 NASM/MSVC 实编）。这是沙箱限制，不是代码错误；需要真正编译时须放开该目录写权限。
- cargo 路径：`C:\Users\Administrator\.cargo\bin\cargo.exe`（不在默认 PATH，需显式加入）。

## 本机 Windows 构建前置依赖（已全部装好缓存，构建前加这些环境变量）

构建 `easytier-web` / `easytier` / `easytier-gui` 会走 `thunk-rs`（VC-LTL5 + YY-Thunks，Win7 兼容）与 `protoc`，三者都要联网抓二进制，本机默认环境会失败。已缓存到：

- `C:\Users\Administrator\.cache\thunk-deps\VC-LTL-5.2.2`
- `C:\Users\Administrator\.cache\thunk-deps\YY-Thunks-1.1.7`
- `C:\Users\Administrator\.cache\protoc\bin\protoc.exe`（libprotoc 26.0-rc1）

构建命令（Windows / Git Bash）：

```bash
cd /d/EasyTier
export PATH="$PATH:/c/Users/Administrator/.cargo/bin:/c/Program Files/7-Zip"
export VC_LTL="C:\\Users\\Administrator\\.cache\\thunk-deps\\VC-LTL-5.2.2"
export YY_THUNKS="C:\\Users\\Administrator\\.cache\\thunk-deps\\YY-Thunks-1.1.7"
export PROTOC="C:\\Users\\Administrator\\.cache\\protoc\\bin\\protoc.exe"
cargo build --profile release-fast -p easytier-web
```

三个坑：
1. **7z 不在 PATH**（装在 `C:\Program Files\7-Zip`）→ thunk-rs 调 `Command::new("7z")` 报 `program not found` 并 panic。
2. **Schannel 吊销检查失败**：`curl: (35) ... 0x80092013 由于吊销服务器已脱机`，不是网络不通 → 手动下载加 `--ssl-no-revoke` 即可。
3. **`--profile release-fast` 会打断 `easytier-proto/build/main.rs`**：自定义 profile 下 cargo 把 `PROFILE` 设为 `release`，但 OUT_DIR 在 `target/release-fast/...`，`get_cargo_target_dir()` 找不到以 `release` 结尾的父目录 → `unwrap()` panic「not found」。用 `PROTOC` 环境变量指到已有 protoc 即可绕开下载分支（推荐，也省下每次联网）。

## Win7 兼容定位（2026-09-29 用户确认：保持不变）

- `thunk-rs` features 固定 `["win7"]`，四个包一致：`easytier`、`easytier-contrib/easytier-ffi`、`easytier-gui/src-tauri`、`easytier-web`。
- 作用是双份：VC-LTL5 把 VC 运行库换成系统自带 msvcrt/ucrt（**exe 更小 + 目标机免装 VC++ 运行库**），YY-Thunks 补齐 Win7 缺失的系统 API。
- **取消兼容不只是「少支持一个系统」**，会连带丢掉上面两点收益，且影响全部四个包的发布。README / docs 未对外声明支持 Win7，所以没有违约风险，但改动属项目级发布策略，须用户明确决定后再动。
- 若只想让构建不再联网：用上面的 `VC_LTL` / `YY_THUNKS` 环境变量即可，代码零改动。
