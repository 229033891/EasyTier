# EasyTier 项目长期约定

## 0. 硬性禁止（先看）
- **不打包/不编译**（2026-09-30）：不主动 `cargo build`，exe 由老大自己出；只改代码别顺手触发全量编译。
- **不用本地 Rust 编译/check 当验证**（2026-10-07「本地编译会报错，请跳过」）：本机 `cargo check` 必挂 C 依赖（`windivert-sys`/`zstd-sys`/`ring` 的 `cl.exe` 退出码 2）。Rust 改动一律**静态核对**（读代码/对签名/查调用点），编译交用户或 CI；别去修环境。`#[cfg(mobile)]` 本地覆盖不到。
- **能跑的验证只有前端**：`easytier-gui` 用 `./node_modules/.bin/{vue-tsc --noEmit|vitest run <f>|eslint . --ignore-pattern src-tauri}`；`easytier-web/frontend-lib` 用 `./node_modules/.bin/{vue-tsc --noEmit|vitest run}`（**无 eslint 配置**）。
- **改动一律在 `dev` 分支**（2026-10-08），别在 `releases/*` 改代码；改完先 `git branch --show-current`。
- 核对签名读 cargo 缓存：`~/.cargo/git/checkouts/rust-tun-*/...`。

## 1. 前端验证 / 构建
- 现状：`easytier-gui` eslint 0 / vue-tsc 0 / vitest 28 passed；`frontend-lib` vitest 114 passed(15 文件) / vue-tsc 0 错（2026-10-09 复核）。
- **`pnpm` 在本机 Git Bash 不可用**（corepack 拼成 `D:\c\Program Files\...`→`MODULE_NOT_FOUND`）→ 用 `node_modules/.bin/<tool>`；必要时 `node "C:/Program Files/nodejs/node_modules/corepack/dist/pnpm.js" <script>`。
- 跑 `frontend-lib` 的 `vue-tsc` 前确保 `frontend-lib/dist` 最新（gitignore，易过期→报「`Ipv6Inet` 不存在」假错）。重建：`cd easytier-web/frontend-lib && node scripts/codegen-proto.mjs && ./node_modules/.bin/vue-tsc -b && ./node_modules/.bin/vite build`。
- **`vite build` 会被沙箱删除护栏挡**：`emptyDir(outDir)` 的 `rmSync`→`genie-trash ETIMEDOUT`，在 "N modules transformed" 之后才炸（不是编译失败）。绕法：先 `mv dist $TEMP/xxx` 再 build；**同一命令别带 `rm -rf`**（会连累整条命令 SIGTERM）。
- **`eslint --fix` 对依赖虚拟模块的 import 不可信**：`import/no-duplicates` 曾把 `vue-router/auto` 与 `vue-router/auto-routes` 合并（都解析到 `vue-router.mjs`）改坏 `main.ts`；已在 `eslint.config.js` 对 `src/main.ts` 关掉该规则。跑完 `--fix` 必须核对 import 说明符集合。
- `no-console` 放宽 `allow:['log','info','debug','warn','error']` —— WebView console 是安卓 logcat 唯一出口，别降级成 `warn`。
- **lint 已进门禁**：`easytier-gui` 的 `build` = `pnpm lint && pnpm --dir ../easytier-web/frontend-lib build && vue-tsc --noEmit && vite build`（覆盖本地 `pnpm build`/`tauri build` 与 CI `android.yml`/`windows.yml`）。`test.yml` **无**独立前端 lint job。
- `frontend-lib` 两个测试脚本：`test:config-ui`（`vitest run --config vitest.config.ts`，CI 只跑这个）与 `test:network-config`（`pnpm build && node scripts/test-network-config.mjs`）。改配置序列化**两个都跑**。
- **protobuf-ts 的 int64/uint64 是 BigInt、JSON 形状要字符串**：`toBackendNetworkConfig` 展开消息再 `fromJson` 会抛 `Cannot parse JSON bigint`→`networkCompat.ts::dropUnsupportedJsonValues()` 统一 `bigint→toString()`。加 int64/uint64（尤其嵌套）要确认覆盖。`allFieldFixture()` 用 `{...DEFAULT_NETWORK_CONFIG()}`，`NetworkConfigPb.create()` 只给**非 optional** 填默认→repeated 字段「看着覆盖」其实为空，加字段要显式给值。

## 2. MagicDNS fake IP = `10.255.255.254`（2026-10-08 起）
- 单一源头 `easytier/src/instance/dns_server/mod.rs::MAGIC_DNS_FAKE_IP`。历史 `100.100.100.53`→`10.10.10.10`→现值。`/32` 路由、Windows NameServer、Linux systemd-resolved drop-in、Android VpnService DNS 全由它派生（`server_instance.rs`：`!tun_inet.contains(fake_ip)`→`add_ipv4_route(...,32)`）。
- **改地址必须同步的硬编码副本**：`easytier-core/src/gateway/magic_dns/packet.rs`（单测）、`easytier/src/instance/dns_server/server.rs` 与 `system_config/linux.rs`（单测）、`easytier-gui/src/composables/mobile_vpn.ts`(+test)、`easytier-contrib/easytier-android-jni/kotlin/com/easytier/jni/{EasyTierVpnService.t.kt,EasyTierManager.kt,EasyTierJNI.kt,README.md}`、`docs/current/magic-dns{,-manual-wiring}.md`、`docs/roadmap/dns-policy.md`、`frontend-lib/src/locales/{cn,en}.yaml`。
- **别跟着改**：`gateway/proxy/wrapped_tcp_proxy.rs`、`frontend-lib/tests/status-display.spec.ts`（同名测试地址）。`tauri-plugin-vpnservice/android/.../TauriVpnService.kt` 无硬编码。切地址属项目级发布决策。

## 3. 配置页（高级设置）UI（2026-10-08）
- **Windows 控制台与移动端 App 共用 `frontend-lib/src/components/Config.vue`**（`easytier-gui` 只 `import ... from 'easytier-frontend-lib'`）→ 改一处两边生效。
- 断点：`≤760px` 高级分组 2 列→1 列；`≤640px` `.config-inline-label` 11rem→5.5rem、`.config-inline-expand` 的 `margin-left` 归零。样式两处：组件内 `<style scoped>`（`.advanced-*`/`.config-compact-*`）+ 全局 `src/style.css`（`.config-inline-*`，scoped 穿不进子组件）。PrimeVue **4.3.9**。
- **已修（方案 A）**：5 个 `ToggleButton`→`ToggleSwitch`；负逻辑字段扩到 **9 个**走 `inverted` 正向展示为 `allow_*`（字段名不变）：`disable_{p2p,kcp_input,quic_input,tcp_hole_punching,udp_hole_punching,sym_hole_punching,upnp,ipv6,encryption}`。**反转展示会改变冲突提示措辞→改文案务必核对 `configConflicts.ts` 的 `*_help` key**（`advancedFlagConflictHelpKey()` 返回值**优先于** `inverted.help`）。
- 相关文档：`docs/current/desktop-gui-and-config-server.md` §4、`docs/current/peer-connections.md`「Web 控件」行、`docs/roadmap/connection-stability-todo.md` P-UX.4。**改配置页 UI 先看这几处。**

## 4. 状态页「代理 CIDR 路由同步」显示规则（2026-10-08）
- 链路：`api_manage.proto::NetworkInstanceRunningInfo.proxy_cidr_route_sync`→`easytier-core/src/management/full/instance_info.rs` 每轮填（源 `easytier/src/instance/virtual_nic.rs` 桌面 L2 同步）→`Status.vue::myNodeInfoGroups`。GUI 本地态与 web-client 上报共用，**无 Windows/web 缺失**。
- **来源因平台而异**：桌面 = L2 路由同步；Android 由 `mobile_vpn.ts::annotateNetworkInfoWithMobileVpnRoutes` 覆盖写 VpnService 路由；OHOS 由 `runtime_api.rs::annotate_ohos_proxy_cidr_route_sync` 覆盖（那边 L2 ifcfg 是 no-op，core 只报空占位 `desired=[-] installed=[-]`）。
- **2026-10-08 起「字段有值就显示」**：`routeSync?.trim()` 判定，空占位也显示。`isMeaningfulProxyCidrRouteSync()` 已删。字段缺失/全空白不显示。
- 回归测试 `tests/status-vpn-portal.spec.ts`（**「节点详情」面板默认折叠，断言前先点 `button[data-label="node_info_details"]`**）。文档 `docs/current/traffic-steering.md`「可观测」。

## 5. 版本号 bump（10 文件 / 17 处）
**权威清单 `docs/ops/release-version-bump.md`**。索引：1) `Cargo.toml` 3 处（`[workspace.dependencies]` 的 `easytier`/`easytier-core`/`easytier-proto`，只改 `version` 别动同行 `path`）；2) `Cargo.lock` **6 处**（`easytier`/`easytier-core`/`easytier-gui`/`easytier-mini`/`easytier-proto`/`easytier-web`，全仓只此一个）；3–6) `easytier{,-core,-proto}`/`easytier-web` 的 `Cargo.toml`；7–9) `easytier-gui/package.json`、`easytier-gui/src-tauri/{Cargo.toml,tauri.conf.json}`；10) `easytier-contrib/easytier-mini/Cargo.toml`（**易漏**：2.7.41/42/43 三次都漏）。
**不动**：`easytier-ffi`/`-android-jni`/`-ios`/`-uptime`/`-ohrs*`（`0.1.0`）、`tauri-plugin-vpnservice`（`0.0.0`）、`easytier-web/frontend{,-lib}/package.json`（`0.0.0`）、`easytier-js/package.json`（`0.1.0`）。
**校验**：`cargo metadata --no-deps --offline --format-version 1` 应报 6 包同版本；残留 `grep -rnE 'X\.Y\.Z([^0-9]|$)'`（`2.7.4` 是 `2.7.44` 前缀，必须带 `([^0-9]|$)`；`easytier-gui/package.json` 的 `@types/node: ^22.7.4` 无关）。**别用 `cargo build` 验证**。不硬编码版本：`openwrt.yml` 从 `easytier/Cargo.toml` 正则读；`docker.yml` L19 的 `image_tag.default:'v2.7.2'` 是陈旧手动默认值（未改）。

## 6. OpenWrt 打包（`.github/workflows/openwrt.yml`）
- 仓库只此一个 OpenWrt 文件；LuCI 界面与 feed Makefile 来自外部仓库 `229033891/luci-app-easytier`（checkout 到 `feed/`，本地无副本）。流水线 `build-bins`（musl x86_64 编 core/cli/web-embed，UPX 后传 `ET-openwrt-bins`）→`build-openwrt`（矩阵）→`openwrt-result`。
- **2026-10-08 起只出 apk**（仅 OpenWrt 25.12+）：矩阵单行 `pkgtype: apk` + `sdk:"25.12.5"`（原 `SNAPSHOT` 会漂）。产出 `easytier`+`luci-app-easytier`+自动带出的 `luci-i18n-easytier-zh-cn`。**artifact 名保留 `ET-openwrt-x86_64-apk`**，矩阵结构故意留着。
- ghcr `openwrt/sdk` 实测 `x86_64-25.12.0`~`25.12.5` 全在（无补丁号 404）。探测 `curl --ssl-no-revoke -H "Accept: application/vnd.oci.image.index.v1+json" https://ghcr.io/v2/openwrt/sdk/manifests/<tag>`（token 取 `https://ghcr.io/token?scope=repository%3Aopenwrt%2Fsdk%3Apull&service=ghcr.io`）。
- 本 workflow 无签名密钥→包未签名→须 `apk add --allow-untrusted`。25.12 起 apk 取代 opkg，≤24.10 仍 ipk；`release.yml` 按目录名通用处理，删 ipk 轨不破坏它。**bump 版本只需改 `easytier/Cargo.toml`**（workflow 正则读它生成 `feed/version.mk`）。

## 7. Android / 移动端易踩事实（2026-10-08）
- **Android 无 `IfConfiger` 物理默认路由实现**：`netlink.rs` 门禁 `#[cfg(all(target_os="linux", feature="linux-netlink"))]`→Android 走默认实现，`find_ipv4/6_physical_default` 恒 `Ok(None)`。**已修**：新增 `ifcfg::physical_default_lookup_supported()`（`cfg!` 镜像同一批门禁，改门禁要同步），`dns.rs::pin_physical` 改为 `is_process_default() && physical_default_lookup_supported()`（行为等价，每查询一次的 `TUN is active but no physical default iface...` 警告消失）。`set_native_socket_protector` **只有 OHOS 注册**。
- **`None`/`Some("")`/`BindDev::Auto`/`Disabled` 等价关系（改 socket 绑定前必看）**：`BindDev::from("")`=`Disabled`；`bind_device=None` 时 TCP 因 `local_addr_was_defaulted`→`Disabled`、UDP 因 purpose `DirectConnect`→`Auto`；`Auto`=`tunnel/common.rs:230 get_interface_name_by_ip(&addr.ip())`，DNS 的 UDP 绑定地址是 `0.0.0.0:0`/`[::]:0`，该函数对 `is_unspecified()` **直接 return None**→**DNS 路径上 `None` 与 `Some("")` 等价**。
- **`web_client_routine` 退避（已修）**：原来 `connect_config_server` 成功就立刻重置 `backoff`，使「dial 成功但会话协商失败」永远 1s 重试；改为**只在会话真正建立时重置**。
- **Android TUN 路由由 GUI 汇总**：`mobile_vpn.ts::getRoutesForVpn`（`proxy_cidrs + node_config.routes + MagicDNS fake IP /32`，exit 可达时加 `/0`）经 `TauriVpnService.addRoute` 装进 VpnService——核心侧 L2 ifcfg 在 Android 是 no-op。
- **Android 日志只有 WARN/ERROR 是正常的**：console 级别设 `warn` 则 `info!` 不出现；降噪时把 `warn` 降成 `info` = 直接看不到。

## 8. Tauri 移动插件命令名：JS 自动转 camelCase，Rust 不转
- **JS→Kotlin/Swift** 被 Tauri 自动 `AsLowerCamelCase`（`tauri/src/webview/mod.rs` `#[cfg(mobile)]`）→`guest-js`/`dist-js` 保持 snake_case **是对的**；**Rust→Kotlin/Swift** 的 `PluginHandle::run_mobile_plugin("...")` **原样透传**，必须 camelCase。
- 原生侧命令表只按**方法名**建键（Android `PluginHandle.kt` `commands[method.name]`；iOS 是 `@objc` selector）→**Kotlin/Swift 方法名都必须 camelCase**。症状 `InvalidCommandException: No command <名字> found for plugin <类名>`（错误里是 raw 名字，带 snake_case = Rust 路径）。
- 权限：JS 路径过 Tauri 门禁（`permissions/autogenerated/commands/*.toml`+`capabilities/migrated.json`）；**Rust 路径不过门禁**。
- 已修：`tauri-plugin-vpnservice/src/mobile.rs` 5 个命令名改 camelCase；`StartVpnRequest` 补 `ipv6_addr`、`Status` 补 `granted`。核对脚本 `~/.workbuddy-ai/skills/easytier-mobile-plugin-contract/scripts/check_contract.py`。**CI 不测这个插件**。

## 9. Windows 安装包文件锁 + NSIS
- 安装位置 Tauri per-user，`$INSTDIR`=`%LOCALAPPDATA%\ET`（产品名 `ET`）。随包资源（`wintun.dll`/`Packet.dll`/`*.sys`）不在 git，由 `windows.yml:120` 从 `easytier/third_party/<arch>/` 拷进 `easytier-gui/src-tauri/`。
- **`Packet.dll`（Npcap）是 `easytier-gui.exe` 静态导入**（`pnet_datalink` 的 `#[link(name="Packet")]`，由 **`default` 里的 `faketcp`** 引入）→**进程一启动就映射、整个生命周期锁定**。`wintun.dll` 懒加载，`WinDivert*.sys` 走内核驱动。
- 报错机理：原钩子只对主 exe rename-aside，DLL/SYS 原地覆盖→`Error opening file for writing: ...\Packet.dll`。per-user 不提权、停不掉 SYSTEM 的 `ET-Gui` 服务→必然复现；**Server 上没跑该服务就"基本不报"**（`windows-latest`=Server 的 CI 永远复现不了）。
- **已修**：`ET_UnlockThirdPartyBinaries`（4 文件逐个 rename-aside + `cmd move` 兜底 + `Delete /REBOOTOK`，PREINSTALL/PREUNINSTALL 都用）；`ET_DeleteGuiService`/`ET_DeleteOneService`（query→delete→失败 `ExecShellWait "runas"`→复核→MessageBox；**删服务前必须先 stop**；`sc.exe` 写 `"$SYSDIR\sc.exe"`）；`ET_ElevateServiceStopForInstall`（PREINSTALL 最前，先 `sc query|findstr RUNNING`，没服务在跑就不弹 UAC）。
- **NSIS 标签作用域不确定→保守做法**：标签只放「全文件只展开一次」的宏（`NSIS_HOOK_PREINSTALL`）里；多实例用参数化 `${SUFFIX}` 拼标签。改完静态核对 `!macro`/`!macroend` 配平 + 推演宏插入次数。

## 10. NSIS 改动**可以**本机编译校验
- **不需要装 NSIS**：Tauri 打包会下载一份到 `C:\Users\Administrator\AppData\Local\tauri\NSIS\`（v3.08，含 `Include/`、`Contrib/Modern UI 2/`、`makensis.exe`）。
- 校验：最小 harness（`%TEMP%\et-nsis-check\check.nsi`，**别放仓库**）→`!include` 真实 hooks→`Section Install`/`Section Uninstall` 里 `!insertmacro NSIS_HOOK_*`→`%LOCALAPPDATA%\tauri\NSIS\makensis.exe -V3 -NOCD check.nsi`。**两个 Section=两个函数，能真实暴露 label 跨函数撞车。** 模板见 skill `easytier-nsis-hook-verify`。
- **`installer-hooks.nsh` 必须纯 ASCII**（NSIS 用系统 ANSI 代码页，非 ASCII 直接 `Bad text encoding` 中止；`—` U+2014 也不行）。改完 python 扫 `ord(c)>127`。**LogicLib 可用**（`MUI2.nsh:27` 无条件 include）。

## 11. 隧道协议默认优先 TCP → TCP-over-TCP（2026-10-08 分析）
- **默认 `default_protocol` 就是 `tcp`**（`Flags::default()`，`easytier-core/src/config/toml.rs`）；`parse_protocol_preference("")` 也回落 `["tcp"]`。
- **手动 peer URL 会被改写，且"偏好候选"排在配置 URL 之前**：`preference_candidate_urls()` 先按偏好生成候选、最后才追加原始 URL，`reconnect()` 首个成功即返回（`connectivity/manual/mod.rs`）→用户配的 `udp://host:11010` 会先被改写成 `tcp://host:11010` 并优先使用。直连也按同一偏好排序（`connectivity/direct/mod.rs`）。
- **后果**：RDP 这类 TCP 业务变 TCP-over-TCP，轻微丢包就卡。Tailscale 只有 WireGuard/UDP 无此叠加——**「同网络下 Tailscale 好、EasyTier 差」的首选解释**。诊断：grep 日志 `manual reconnect start`（同时打印 `configured_url` 与候选 `url`，不同=发生改写）。
- **规避**：`default_protocol="udp"`+只留 `udp://` peer；或改 **`wg://`**（`easytier/src/tunnel/wireguard.rs` 是 boringtun 真 WireGuard，且 `wg` 不可被改写）。MTU 默认 **1380** 且全仓**无 MSS clamp**，可降到 1280。
- **别把默认协议简单改成 udp**（那会让 `tcp://` 被改写成 udp，同一问题镜像）；真正修法是把**配置的 URL 放回候选第一位**。`select_conn` 丢包权重 **4**（`score=1*RTT+4*loss+1*jitter`），探测包走 TCP 会被重传掩盖→TCP conn 反而显得干净。

## 12. 改「配置读取」优先顺序的坑（2026-10-09 复核）
- `easytier-core/src/management/full/remote_client.rs::handle_get_network_config_with_source` 改 **storage-first** 会连带影响 GUI：web 侧 storage=权威 DB，存储优先是对的；**但 `easytier-gui/src-tauri/src/manager.rs::persist_runtime_dev_name`（`#[cfg(windows)]`）依赖 RPC 优先**——它要在实例启动后回读内核分配的 wintun `dev_name`，而库里那份 dev_name 为空→存储优先后该函数永远拿不到非空名字→Windows 上适配器名不再持久化→每次重启可能多一块 `et_*` 网卡。
- → **改这个函数或 storage/RPC 顺序前，先 grep 所有 `handle_get_network_config*` 调用点**（GUI `lib.rs:352/459`、`manager.rs:564`、web `restful/network.rs:367`），逐个确认要「库里那份」还是「运行时那份」。现状：trait 默认 RPC-first，web 覆写为 SQLite-first。

## 13. 本机构建前置依赖（只在明确要求编译时用）
`thunk-rs`（VC-LTL5+YY-Thunks，Win7 兼容）与 `protoc` 要联网抓二进制，已缓存 `C:\Users\Administrator\.cache\thunk-deps\{VC-LTL-5.2.2,YY-Thunks-1.1.7}`、`C:\Users\Administrator\.cache\protoc\bin\protoc.exe`。
```bash
cd /d/EasyTier
export PATH="$PATH:/c/Users/Administrator/.cargo/bin:/c/Program Files/7-Zip"
export VC_LTL="C:\\Users\\Administrator\\.cache\\thunk-deps\\VC-LTL-5.2.2"
export YY_THUNKS="C:\\Users\\Administrator\\.cache\\thunk-deps\\YY-Thunks-1.1.7"
export PROTOC="C:\\Users\\Administrator\\.cache\\protoc\\bin\\protoc.exe"
cargo build --profile release-fast -p easytier-web
```
三坑：①**7z 不在 PATH**→thunk-rs panic；②**Schannel 吊销检查失败**（`0x80092013`，非断网）→加 `--ssl-no-revoke`；③**`--profile release-fast` 会打断 `easytier-proto/build/main.rs`**（cargo 设 `PROFILE=release` 但 OUT_DIR 在 `target/release-fast/...`→`unwrap()` panic），用 `PROTOC` 绕开下载分支。

## 14. Win7 兼容（2026-09-29 确认保持）
`thunk-rs` features 固定 `["win7"]`，四包一致：`easytier`、`easytier-contrib/easytier-ffi`、`easytier-gui/src-tauri`、`easytier-web`。收益：VC-LTL5 换掉 VC 运行库（exe 更小+目标机免装 VC++）+YY-Thunks 补 Win7 缺失 API。取消会连带丢这两点且影响四包发布；README/docs 未声明支持 Win7，属发布策略，须用户明确决定。

## 15. 环境备注（杂项）
- **`cargo test` 跑不起来**：能链接，启动 `STATUS_DLL_NOT_FOUND (0xc0000135)`，环境问题。需 SQL 层验证时用 **Python `sqlite3` 从源码正则抽建表/查询 SQL 直接跑**。
- **沙箱下 `git` 改状态要回读确认**：`branch -f`/`update-ref`/`fetch` 出现过 **exit 0 但静默不生效**（`branch -f` 还把分支搞没了），`packed-refs` 的 `refs/remotes/origin/*` 尤其。绕法：**直接写 loose ref**（`mkdir -p .git/refs/heads/releases && git rev-parse <rev> > .git/refs/heads/releases/vX.Y.Z`）。**凡改 git 状态必须 `git branch --list`/`rev-parse`/`show-ref`/`status -sb` 回读**，远端用 `git ls-remote`。
- **本机 `git` 联网需绕证书吊销**：`env GIT_SSL_NO_VERIFY=true git -c http.schannelCheckRevoke=false -c http.sslBackend=openssl <cmd>`（单独 `-c http.schannelCheckRevoke=false` 无效）；或 `curl --ssl-no-revoke`。网络本身通。
- git 2.55 的 `git rev-parse --short` 不接受多 rev。cargo 在 `C:\Users\Administrator\.cargo\bin\cargo.exe`（不在默认 PATH）。`rustfmt` 只在 1.95.0 工具链：`rustup run 1.95.0 rustfmt --edition 2024 <file>`（只查语法）。
- **Rust 易踩**：①`i64::div_ceil` 1.95 仍 unstable，用 `(a+b-1)/b`；②`DeleteMany::filter` 来自 `QueryFilter` trait（不 `use QueryFilter as _` 会解析到 `Iterator::filter`），`DatabaseConnection::query_all` 是 inherent；③`Box::<dyn Any+Send>::downcast` 按值消费 self→不能 `if let ... else if let ...`（E0382），用 `match`+`Err(boxed)`；④`let it = temp_guard().iter_like();` 是 E0716，要么绑名要么 `.collect()`。

## 16. Web 轮询节奏与隧道争用（2026-10-09）
- **瓶颈**：状态页 `CollectNetworkInfo`、列表 `list_machines`、历史采样、heartbeat reconcile 共用同一 BidirectRpc 隧道；1Hz 轮询最伤配置加载。
- 现状节奏：`RemoteManagement.vue` 的 `STATUS_POLL_MS=2000`；**配置模式**跳过 info 轮询、列表每 `CONFIG_MODE_LIST_POLL_EVERY=3` tick（≈6s）；`frontend` 的 `Dashboard.vue`、`usePollingList` 默认、`DeviceList`/`NetworkList` 均 2s。
- **历史采样定向化**：`easytier-web/src/peer_history.rs` 先用 `ClientManager::get_heartbeat_requests` 取该设备最近心跳的 `running_network_instances`→`Some(非空)` 定向 collect、`Some(空)` 跳过、`None`（无心跳）回退全量。依据：`handle_collect_network_info(identify, None)` 送空 `inst_ids`，设备侧 `process_rpc.rs` 把 `included.is_empty()` 当「全量」。`instance_ids()` 是 InstanceManager 全部运行实例（非仅托管），不会漏本地实例；心跳默认 3.5s，比 60s 采样新。
- **不要随手把轮询调回 1s**，也别给单点加更密的循环——争用直接体现为「配置页加载慢」。

## 17. Chart.js 图表在 v-if 分支里重挂载的坑（2026-10-09 实测）
- `PeerConnHistoryChart.vue` 的 canvas 在 `<template v-else>` 里：`data.value` 一清空（切实例/切范围）就卸载，数据回来再挂载。但 `initCharts()` 守卫是 `if (canvas.value && !chart)`，**模块内 `chart` 变量还指着已卸载的 canvas**→跳过创建→图表全空白。
- 实测：初次加载建 4 个 Chart；`setProps({instanceId:'B'})` 后 DOM 里 4 个新 canvas 都在，但 Chart 实例没重建（created 仍 4、destroyed 0）。HEAD 同样复现（2→2），属既有 bug。
- 修法：在 `watch([instanceId, hours])` 与错误分支里调 `destroyCharts()`（清 data 前/时）；别放 `syncCharts()` 之后（每次刷新都重建会闪）。回归测试 `tests/peer-conn-history-chart.spec.ts`。
- 通用教训：**v-if 卸载 + 模块级图表实例**必须配对 `destroy`，否则重挂载后是空画布。
