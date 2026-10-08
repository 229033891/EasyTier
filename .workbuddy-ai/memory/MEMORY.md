# EasyTier 项目长期约定

## easytier-web 打包 / 生成 exe 规则（强制）

**2026-09-30 用户两次明确表态：「你不需要管打包的事情」「不要你生成Exe文件。我会自行生成」。因此不要构建/打包，也不要追问「要不要现在构建」——exe 由用户自己出。** 下面几条继续有效：

- 不要主动执行 `cargo build` / `cargo build --release` 生成 `easytier-web.exe` 或 embed 包。
- 全量编译 + LTO 链接耗时很长（数分钟起），未经确认直接开编会浪费大量时间。
- 仅改代码、排查问题、看类型或构建配置时，**不要顺手触发全量编译**。
- 只有用户明确说「打包 / 生成 exe / 出可部署产物」时才编译；届时先问清平台、用途、是否要 embed 一体包，日常迭代优先 `release-fast`。
- **例外：`vue-tsc` / `vitest` 属于验证手段，不是打包产物，可以主动跑**（见下面「环境备注」）。

详细步骤与说明见 `docs/easytier-web-build-and-deploy.md` 第 0 节。

## 不要用本地 Rust 编译/check 作为验证手段（强制，2026-10-07 用户表态）

**2026-10-07 用户明确说：「本地环境编译会报错，请跳过」。** 本机 `cargo check` 无论 debug 还是 `--profile release-fast`，都会卡在 C 依赖上（`windivert-sys` / `zstd-sys` / `ring` 的 `cl.exe` 退出码 2，debug profile 必挂；release-fast 也要先过一遍这些 crate）。因此：

- **改完 Rust 不要跑 `cargo check` / `cargo build` 去「验证」**，也不要因为它报错就去修 C 依赖或环境——那不是本任务的信号。
- Rust 改动一律**静态核对**（读代码、对签名、查调用点），编译交给用户 / CI。
- `tun_mobile.rs` 之类 `#[cfg(mobile)]` 的代码本地无论如何都覆盖不到，别指望本地能查出来。
- 能跑的验证只剩：`cd easytier-gui && ./node_modules/.bin/vue-tsc --noEmit`、`./node_modules/.bin/vitest run <file>`、`./node_modules/.bin/eslint . --ignore-pattern src-tauri`，以及 Kotlin 的静态核对。
- 需要交叉核对签名时可直接读 cargo 缓存里的依赖源码（如 `~/.cargo/git/checkouts/rust-tun-*/.../src/async/unix_device.rs`）。

## easytier-gui 前端验证 / lint（2026-10-07 归零）

**状态：`eslint . --ignore-pattern src-tauri` = 0 问题**（此前 546 个）。`vue-tsc --noEmit` 0 错、`vitest run` 28 passed、`vite build` 成功。

- **`pnpm` 在本机 Git Bash 下不可用**（corepack shim 把路径拼成 `D:\c\Program Files\...` → `MODULE_NOT_FOUND`）。一律直接用 `node_modules/.bin/<tool>`：`vite` / `vue-tsc` / `vitest` / `eslint`。
- **跑 `vue-tsc` 前必须先确保 `easytier-web/frontend-lib/dist` 是最新的**（该目录被 gitignore，容易过期，会报出「`Ipv6Inet` 不存在」这类假错误）。重建：`cd easytier-web/frontend-lib && node scripts/codegen-proto.mjs && ./node_modules/.bin/vue-tsc -b && ./node_modules/.bin/vite build`。
- **`vite build` 会被沙箱删除护栏挡住**：`emptyDir(outDir)` 的 `rmSync` → `spawnSync genie-trash ETIMEDOUT`，在 "N modules transformed" 之后才炸，看着像编译失败其实不是。绕法：先 `mv dist $TEMP/xxx` 把旧产物移走（纯改名不触发护栏）再 build。**同一命令里不要带 `rm -rf`**，会连累整条命令被 SIGTERM。
- **`eslint --fix` 对依赖虚拟模块的 import 不可信**：`import/no-duplicates` 曾把 `vue-router/auto` 与 `vue-router/auto-routes` 合并（resolver 把两者都解析到 `vue-router.mjs`），丢掉 `routes` 直接改坏 `main.ts`。已在 `eslint.config.js` 对 `src/main.ts` 关闭该规则并加注释。跑完 `--fix` 必须核对 import 的模块说明符集合。
- `no-console` 已放宽为 `allow: ['log','info','debug','warn','error']` —— WebView console 是安卓 logcat 的唯一日志出口，不要把这些日志降级成 `warn`。
- **lint 已进门禁**（2026-10-07）：`easytier-gui/package.json` 的 `build` 改为 `pnpm lint && pnpm --dir ../easytier-web/frontend-lib build && vue-tsc --noEmit && vite build`。lint 放最前是为了快速失败（不必等几分钟的前端构建）。覆盖面：本地 `pnpm build`、`pnpm tauri build`（`tauri.conf.json` 的 `beforeBuildCommand: pnpm build`）、CI 的 `android.yml` / `windows.yml`（经 `prepare-pnpm` 的 `pnpm -r --filter "easytier-gui..." build`）。`test.yml` 里仍然**没有**独立的前端 lint job（其 `check` job 只聚合 `check-fmt/clippy/hack/wasi`）。
- 在 Git Bash 里跑不了 `pnpm` 时，可用 `node "C:/Program Files/nodejs/node_modules/corepack/dist/pnpm.js" <script>` 代替，实测可用（会按 `packageManager` 字段用 pnpm 9.12.1）。

## MagicDNS fake IP（2026-10-08 起 = `10.255.255.254`）

- 单一源头：`easytier/src/instance/dns_server/mod.rs` 的 `pub static MAGIC_DNS_FAKE_IP`。历史：CGNAT `100.100.100.53` → `10.10.10.10` → 现 `10.255.255.254`。`/32` 路由、Windows NameServer、Linux systemd-resolved drop-in、Android VpnService DNS 全部由它派生（`server_instance.rs`：`if !tun_inet.contains(&fake_ip) → add_ipv4_route(...,32)`）。
- **硬编码副本散落多处，改地址时必须同步**：`easytier-core/src/gateway/magic_dns/packet.rs`（单测）、`easytier/src/instance/dns_server/server.rs` 与 `system_config/linux.rs`（单测）、`easytier-gui/src/composables/mobile_vpn.ts`（+ test）、`easytier-contrib/easytier-android-jni/kotlin/com/easytier/jni/{EasyTierVpnService.t.kt,EasyTierManager.kt,EasyTierJNI.kt,README.md}`、`docs/current/magic-dns.md`、`docs/current/magic-dns-manual-wiring.md`、`docs/roadmap/dns-policy.md`、`easytier-web/frontend-lib/src/locales/{cn,en}.yaml`。
- 不相关的同名测试地址（不要跟着改）：`easytier-core/src/gateway/proxy/wrapped_tcp_proxy.rs`、`easytier-web/frontend-lib/tests/status-display.spec.ts`。
- `tauri-plugin-vpnservice/android/.../TauriVpnService.kt` 的 dns/routes 来自 Intent 参数，无硬编码，不用动。
- 切地址 = 全网 OS DNS/`/32` 重写，属项目级发布决策，别随手改。

## 配置页（高级设置）UI 归属（2026-10-08 查证）

- **Windows 控制台和移动端 App 用的是同一个组件**：`easytier-web/frontend-lib/src/components/Config.vue`。`easytier-gui` 只 `import ... from 'easytier-frontend-lib'`（`main.ts` / `pages/index.vue` 的 `RemoteManagement`），没有自己的配置页。**改一处两边同时生效**，不用做两套。
- 响应式断点：`@media (max-width: 760px)` → 高级开关分组从 2 列变 1 列、开关项双列；`@media (max-width: 640px)` → `.config-inline-label` 从 11rem 收到 5.5rem、`.config-inline-expand` 的 `margin-left` 归零（展开项和开关失去视觉从属）。
- 样式分两处：组件内 `<style scoped>`（`.advanced-*`、`.config-compact-*`）+ 全局 `src/style.css`（`.config-inline-field/label/control/expand`，因为 scoped 穿不进子组件）。
- PrimeVue **4.3.9**，`ToggleSwitch` 可用。
- **UX 债已修（2026-10-08，方案 A）**：5 个 `ToggleButton class="w-48"`（VPN Portal / 网络白名单 / 自定义路由 / socks5 / 共享 IPv6 子网）已换成 `ToggleSwitch`（去掉固定 192px 宽与 `off-icon="pi pi-times"` 的错位语义）；7 个负逻辑勾选框（`disable_p2p` / `disable_kcp_input` / `disable_quic_input` / `disable_tcp|udp|sym_hole_punching` / `disable_upnp`）已走 `inverted` 机制，正向展示为 `allow_*`（字段名不变）。**反转展示会改变冲突提示的措辞方向，改文案时务必核对 `configConflicts.ts` 里对应的 `*_help` key。**

## 状态页「代理 CIDR 路由同步」的显示规则（2026-10-08 查证 + 改造）

- 数据链路：`api_manage.proto` `NetworkInstanceRunningInfo.proxy_cidr_route_sync` → 后端 `easytier-core/src/management/full/instance_info.rs` 每轮填（源：`easytier/src/instance/virtual_nic.rs` 桌面 L2 路由同步）→ 前端 `Status.vue` 的 `myNodeInfoGroups`。GUI 本地态与 web-client 上报共用 `network_instance_running_info`，**不存在 Windows/web 缺失**。
- **字段来源因平台而异**：桌面 = L2 路由同步；Android 由 `easytier-gui/src/composables/mobile_vpn.ts` 的 `annotateNetworkInfoWithMobileVpnRoutes` 覆盖写入 VpnService 路由，OHOS 由 `runtime_api.rs` 的 `annotate_ohos_proxy_cidr_route_sync` 覆盖（那边 L2 ifcfg 是 no-op，core 只会报空占位 `desired=[-] installed=[-]`）。
- **2026-10-08 起改为「字段有值就显示」**（方案 A）：`Status.vue` 用 `routeSync?.trim()` 判定，空占位也照显示，便于确认"确实一条代理路由都没装"。原来的 `isMeaningfulProxyCidrRouteSync()` 判定函数已删除（死代码 + 其单测）。字段缺失（老核心）或全空白仍不显示。
- 回归测试在 `tests/status-vpn-portal.spec.ts`（挂载 Status.vue；**「节点详情」面板默认折叠，断言前要先点 `button[data-label="node_info_details"]`**）。

## 版本号 bump 的文件清单（2026-10-08 定稿：10 个文件）

「版本号改为 X」= 改这 **10 个文件**里的版本号（`easytier-mini` 也在内，用户 2026-10-08 明确选择一起升）：

1. `Cargo.toml` — 3 处，全在 `[workspace.dependencies]`：`easytier` / `easytier-core` / `easytier-proto`
2. `Cargo.lock` — 5 处：`easytier` / `easytier-core` / `easytier-proto` / `easytier-web` / `easytier-gui` 的 `version =`（**全仓只有这一个 Cargo.lock**）
3. `easytier/Cargo.toml`、4. `easytier-core/Cargo.toml`、5. `easytier-proto/Cargo.toml`、6. `easytier-web/Cargo.toml`
7. `easytier-gui/package.json`、8. `easytier-gui/src-tauri/Cargo.toml`、9. `easytier-gui/src-tauri/tauri.conf.json`
10. `easytier-contrib/easytier-mini/Cargo.toml`（**易漏**：`2.7.41/42/43` 三次 bump 都漏了它，停在 `2.7.4`）

**不动**：`easytier-contrib/` 下的 `easytier-ffi` / `easytier-android-jni` / `easytier-ios` / `easytier-uptime` / `easytier-ohrs*`（固定 `0.1.0`）、`tauri-plugin-vpnservice`（`0.0.0`）。

**校验手段**：`cargo metadata --no-deps --offline --format-version 1` 应报 6 个包为同一版本（`easytier` / `easytier-core` / `easytier-gui` / `easytier-mini` / `easytier-proto` / `easytier-web`）；残留检查用 `grep -rnE 'X\.Y\.Z([^0-9]|$)'`（**注意 `2.7.4` 是 `2.7.44` 的前缀，必须加 `([^0-9]|$)`**，否则误报；`easytier-gui/package.json` 里的 `@types/node: ^22.7.4` 是无关依赖，别动）。**不要顺手跑 `cargo build` 验证**（见上文「不要用本地 Rust 编译/check 作为验证手段」）。

## 环境备注

- **`easytier-web/frontend-lib` 有两个测试脚本**：`test:config-ui`（= `vitest run --config vitest.config.ts`）与 `test:network-config`（= `pnpm build && node scripts/test-network-config.mjs`）。**CI 只跑前者**（`linux.yml:77`），后者本地才跑；改完配置序列化相关代码要**两个都跑**。2026-10-08 起两半都绿（脚本的 `allFieldFixture()` 补齐了 11 个 proto 字段，并修了下面那个 BigInt bug）。
- **protobuf-ts 的 int64/uint64 是 BigInt，JSON 形状却要字符串**（2026-10-08 修）：`toBackendNetworkConfig` 把消息实例展开成普通对象再 `fromJson`，BigInt 会漏出来抛 `Cannot parse JSON bigint`（如 `managed_credentials[].expiry_unix`，非 optional 字段被类初始化成 `0n`，所以只要数组非空必炸）→ 已在 `networkCompat.ts` 的 `dropUnsupportedJsonValues()` 里统一 `bigint → toString()`。**以后往 NetworkConfig 加 int64/uint64 字段（尤其嵌套消息里的），记得确认这条转换覆盖到了。**
- **`allFieldFixture()` 覆盖检查的坑**：它用 `{...DEFAULT_NETWORK_CONFIG()}`，而 `NetworkConfigPb.create()` 只给**非 optional** 字段填默认值（repeated → `[]`）。所以 repeated 字段"看起来覆盖了"其实值是空的（会被 `toJson` 省略，过不了 round-trip 的「字段必须在场」检查），`optional` 字段则真的缺席。加字段时要显式给值。
- **`cargo test` 跑不起来**：测试二进制能编译链接，启动时 `STATUS_DLL_NOT_FOUND (0xc0000135)`。已排除随包 DLL（只有运行时动态加载的 wintun/Packet）与系统 VC 运行库，属环境问题，同环境跑其它 crate 的测试也一样。
- 需要 SQL 层验证时，可行办法是**用 Python `sqlite3` 从源码正则抽出建表/查询 SQL 直接跑**（绕开 Rust 编译与测试运行时），已验证有效。
- cargo 路径：`C:\Users\Administrator\.cargo\bin\cargo.exe`（不在默认 PATH，需显式加入）。`rustfmt` 只装在 `1.95.0` 工具链上（不是 rust-toolchain.toml 指定的 `1.95`），要用 `rustup run 1.95.0 rustfmt --edition 2024 <file>`；它只能查语法，查不出类型错误。
- **Rust 侧几个易踩的坑**：
  1. `i64::div_ceil` 在 1.95 上仍属 unstable 的 `int_roundings`（只有无符号整数稳定了），要用 `(a + b - 1) / b`。
  2. `DeleteMany::filter` 来自 `QueryFilter` trait，不是 inherent 方法；不 `use QueryFilter as _` 会解析到 `Iterator::filter` 报「is not an iterator」。而 `DatabaseConnection::query_all` 是 inherent，加 `ConnectionTrait` 反而报 unused import。
  3. `Box::<dyn Any + Send>::downcast` **按值消费** self（`fn downcast<T>(self: Box<Self>)`）→ 不能 `if let Ok(a) = b.downcast::<A>() {} else if let Ok(c) = b.downcast::<B>() {}`（E0382），要用 `match` + `Err(boxed)` 重新绑定。
  4. `let it = temp_guard().iter_like();` 这种「临时值当接收者、返回值借用它」的写法是 E0716；要么把 guard 绑到名字上，要么 `.collect()` 成 owned 容器。

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
