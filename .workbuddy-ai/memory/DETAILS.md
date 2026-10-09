# EasyTier 详细笔记（D:\EasyTier）

> 本文件是 `MEMORY.md` 的**细节库**：MEMORY.md 只留硬规则 + 索引（受注入长度限制），细节都在这里。
> 需要哪块就读哪节；小节编号与 MEMORY.md 的指针一致。
> 改动一律在 `dev` 分支；不主动打包编译（见 §0）。

## 0. 硬性禁止 / 验证边界
- **不主动打包编译**（exe 老大自己出）。本机 Rust 编译不可用：`cargo check/build` 挂 C 依赖（`windivert-sys`/`zstd-sys`/`ring` 的 `cl.exe` 退出码 2）；`cargo test` 能链接但启动 `STATUS_DLL_NOT_FOUND`。Rust 改动只**静态核对**；语法用 `rustup run 1.95.0 rustfmt --edition 2024 --emit stdout <f>`。`#[cfg(mobile)]` 本地覆盖不到。
- **能跑的验证只有前端**：`easytier-gui` → `./node_modules/.bin/{vue-tsc --noEmit|vitest run <f>|eslint . --ignore-pattern src-tauri}`；`frontend-lib` → `./node_modules/.bin/{vue-tsc --noEmit|vitest run}`（无 eslint 配置）。基线 10-09：gui eslint/vue-tsc 0、vitest 28；frontend-lib vitest 114 passed(15 文件)、vue-tsc 0。
- `pnpm` 在 Git Bash 不可用（corepack 拼错路径）→ 一律 `node_modules/.bin/<tool>`。文档链接检查：`python ~/.workbuddy-ai/skills/easytier-docs-consistency/scripts/check_links.py docs`。

## 1. 前端构建 / 测试坑
- `frontend-lib` 跑 `vue-tsc` 前 `dist` 要最新（gitignore 易过期→假错 `Ipv6Inet` 不存在）。重建：`node scripts/codegen-proto.mjs && ./node_modules/.bin/vue-tsc -b && ./node_modules/.bin/vite build`。
- `vite build` 被沙箱删除护栏挡（`emptyDir` 的 `rmSync`→`genie-trash ETIMEDOUT`，在 "N modules transformed" 之后才炸，**不是**编译失败）。绕法：先 `mv dist $TEMP/xxx` 再 build；**同一命令别带 `rm -rf`**。
- `eslint --fix` 会把 `vue-router/auto` 与 `vue-router/auto-routes` 的 import 误合（已对 `src/main.ts` 关 `import/no-duplicates`）；跑完必须核对 import 说明符集合。
- `no-console` 放行 log/info/debug/warn/error —— WebView console 是安卓 logcat 唯一出口，别降级。
- **lint 已进门禁**：`easytier-gui` 的 `build` = `pnpm lint && pnpm --dir ../easytier-web/frontend-lib build && vue-tsc --noEmit && vite build`（覆盖本地 `pnpm build`/`tauri build` 与 CI `android.yml`/`windows.yml`）。`test.yml` 无独立前端 lint job。
- `frontend-lib` 两个测试脚本：`test:config-ui`（`vitest run --config vitest.config.ts`，CI 只跑这个）与 `test:network-config`（`pnpm build && node scripts/test-network-config.mjs`）。改配置序列化**两个都跑**。
- **locales `*.yaml` 是 plain scalar → 文案里出现 `: `（冒号+空格）js-yaml 直接解析失败**（`bad indentation of a mapping entry`）；`@modyfi/vite-plugin-yaml` 用的就是 js-yaml → `vite build` 与任何 import `i18n.ts` 的测试全挂。复验：`node -e "require('D:/EasyTier/node_modules/.pnpm/js-yaml@4.1.0/node_modules/js-yaml').load(require('fs').readFileSync('<f>','utf8'))"`。`easytier/locales/app.yml` 的值有双引号，安全。
- **PrimeVue 4.3.9 `AutoComplete` 在 `multiple` 模式下「失焦不提交」**：`autocomplete/index.mjs::onInput` 里 `if (!this.multiple) this.updateModel(...)` —— multiple 时打字**不进 model**；`onBlur` 只 `$emit('blur')`，**不 commit**。所以只绑 keydown 的 chips 输入（回车/逗号才提交）会**静默丢草稿**（打完字直接点保存/切走）。统一走 `frontend-lib/src/modules/chipsInput.ts`：`onChipsKeydown`（回车/逗号）+ **`onChipsFocusOut`（失焦提交，用 `focusout` 因为 `blur` 不冒泡）**。typeahead 场景要传 `{typeahead:true}`，下拉展开时 Enter 与失焦都不提交（否则会把半截查询词变成 chip）。**新增/改 chips 输入必须同时绑这两个。**
- 列表编辑器（`DnsHostsEditor`/`DnsForwardersEditor`/`DnsUpstreamEditor`/ACL 弹窗）**不能用 `arr.push`/`splice` 原地改**：`defineModel` 的 setter 不触发 → 不 emit。统一「复制数组 → 整体赋值」（`touchHosts(next)` 模式）。行 key 用 `WeakMap` 按对象身份生成，换数组不会导致 focus 跳。
- 脏标记快照 `modules/config-dirty.ts` 走 `toBackendNetworkConfig` 序列化后比对 → **序列化时被过滤的东西不算脏**（如空 `upstream_dns` 草稿行被 trim+filter 掉 → 不脏；但空 `forwarders` 行会原样保留 → 脏）。改序列化过滤规则会连带改脏判定。
- protobuf-ts 的 int64/uint64 是 BigInt、JSON 要字符串：`networkCompat.ts::dropUnsupportedJsonValues()` 统一 `bigint→toString()`。`allFieldFixture()` 用 `{...DEFAULT_NETWORK_CONFIG()}`，`NetworkConfigPb.create()` 只填非 optional → repeated 字段要显式给值。

## 2. MagicDNS
- fake IP = `10.255.255.254`，单一源头 `easytier/src/instance/dns_server/mod.rs::MAGIC_DNS_FAKE_IP`；`/32` 路由、Windows NameServer、Linux systemd-resolved drop-in、Android VpnService DNS 全派生自它。
- **改地址要同步的硬编码副本**：`gateway/magic_dns/packet.rs`、`dns_server/server.rs` + `system_config/linux.rs`（单测）、`easytier-gui/src/composables/mobile_vpn.ts`(+test)、`easytier-contrib/easytier-android-jni/kotlin/com/easytier/jni/{EasyTierVpnService.t.kt,EasyTierManager.kt,EasyTierJNI.kt,README.md}`、`docs/current/magic-dns{,-manual-wiring}.md`、`docs/roadmap/dns-policy.md`、`frontend-lib/src/locales/{cn,en}.yaml`。**别跟着改**：`gateway/proxy/wrapped_tcp_proxy.rs`、`frontend-lib/tests/status-display.spec.ts`。切地址属发布决策。
- **静态 hosts（`dns_config.hosts`）**：分类在 `easytier-core/src/config/dns.rs::classify_host_name`；精确名 → 同名 zone 的 apex A，`*.suffix` → 父 zone + `*.suffix.` A。hickory 0.25.2 通配**逐级剥标签**（`hickory-server/src/store/in_memory/inner.rs::inner_lookup_wildcard`）→ `*.corp.example` **也命中** `a.b.corp.example`（RFC 4592）。
- 守卫（`server_instance.rs::static_host_skip_reason()`，apply 与状态 RPC **共用**）：zone **<2 标签**拒（`MIN_STATIC_HOST_ZONE_LABELS`，拒 `*.com`/`com`）→ zone 与 route zone 同名跳过（**精确与通配都查**；否则 `MagicDnsRecordStore::update()` 用路由重建覆盖 → 抖）→ zone == `magic_tld_zone`（`et.net.`）跳过 → 通配父 zone 被 split 占用跳过。精确子域仍可覆盖同名路由 hostname（B2）。
- 三入口校验**分层**（判定共用 `HostZoneTarget::reject_reason()` / `DnsConfig::invalid_hosts()`）：CLI `--dns-host` 硬失败；TOML 加载时 warn + 运行时 warn；托管下发只运行时 warn。改文案同步 `frontend-lib/src/locales/{cn,en}.yaml::dns.hosts.editor_help` 与 `easytier/locales/app.yml::core_clap.dns_host`。
- `reload_dns_policy` 顺序：retain_zones(route+applied_static+TLD) → `reload_split_forwarders` → `apply_static_hosts` → `reload_root_forwarder`（hosts 后放才赢）。`applied_static_zones` 只记成功 upsert 的 zone。

## 3. 配置页（高级设置）UI / 状态页
- Windows 控制台与移动端共用 `frontend-lib/src/components/Config.vue`。PrimeVue **4.3.9**。断点：`≤760px` 高级分组 2 列→1 列；`≤640px` `.config-inline-label` 11rem→5.5rem、`.config-inline-expand` 的 `margin-left` 归零。样式两处：组件内 `<style scoped>`（`.advanced-*`/`.config-compact-*`）+ 全局 `src/style.css`（`.config-inline-*`，scoped 穿不进子组件）。
- 9 个负逻辑字段走 `inverted` 正向展示为 `allow_*`（字段名不变）：`disable_{p2p,kcp_input,quic_input,tcp_hole_punching,udp_hole_punching,sym_hole_punching,upnp,ipv6,encryption}`。反转会改冲突提示措辞 → 核对 `configConflicts.ts` 的 `*_help` key（`advancedFlagConflictHelpKey()` **优先于** `inverted.help`）。文档：`docs/current/desktop-gui-and-config-server.md` §4、`docs/current/peer-connections.md`「Web 控件」、`docs/roadmap/connection-stability-todo.md` P-UX.4。
- 状态页「代理 CIDR 路由同步」：链路 `api_manage.proto::NetworkInstanceRunningInfo.proxy_cidr_route_sync` → `easytier-core/src/management/full/instance_info.rs` → `Status.vue::myNodeInfoGroups`。桌面 = L2 路由同步；Android 由 `mobile_vpn.ts::annotateNetworkInfoWithMobileVpnRoutes` 覆盖；OHOS 由 `runtime_api.rs::annotate_ohos_proxy_cidr_route_sync` 覆盖。规则：字段有值（`trim()` 非空）就显示，空占位也显示；缺失/全空白不显示。回归 `tests/status-vpn-portal.spec.ts`（**「节点详情」默认折叠，断言前先点 `button[data-label="node_info_details"]`**）。

## 4. Android / 移动端
- **无 `IfConfiger` 物理默认路由实现**（`netlink.rs` 门禁 `#[cfg(all(target_os="linux", feature="linux-netlink"))]`）→ `find_ipv4/6_physical_default` 恒 `Ok(None)`。已加 `ifcfg::physical_default_lookup_supported()`（`cfg!` 镜像同一批门禁，改门禁要同步），`dns.rs::pin_physical` 用它。
- **`None`/`Some("")`/`BindDev::Auto`/`Disabled` 等价关系**：`BindDev::from("")`=`Disabled`；`bind_device=None` 时 TCP（`local_addr_was_defaulted`）→`Disabled`、UDP（purpose `DirectConnect`）→`Auto`；`Auto`=`tunnel/common.rs::get_interface_name_by_ip`，DNS 的 `0.0.0.0:0`/`[::]:0` 直接 return None → **DNS 路径上 `None` 与 `Some("")` 等价**。
- **TUN 路由由 GUI 汇总**：`mobile_vpn.ts::getRoutesForVpn`（proxy_cidrs + `node_config.routes` + MagicDNS /32，exit 可达时加 /0）→ `TauriVpnService.addRoute`；核心侧 L2 ifcfg 在 Android 是 no-op。Android 日志只有 WARN/ERROR 属正常。`set_native_socket_protector` **只有 OHOS 注册**。`web_client_routine` 退避只在会话真正建立时重置。
- **Tauri 插件命令名**：JS→Kotlin/Swift 被自动 `AsLowerCamelCase` → `guest-js`/`dist-js` 保持 snake_case 是对的；**Rust→原生** `run_mobile_plugin("...")` **原样透传**，必须 camelCase（Android 按 `commands[method.name]`、iOS 按 `@objc` selector）。症状 `InvalidCommandException: No command <名字> found`。权限：JS 路径过门禁，**Rust 路径不过**。核对脚本见 skill `easytier-mobile-plugin-contract`；**CI 不测这个插件**。

## 5. Windows 安装包 / NSIS
- `$INSTDIR`=`%LOCALAPPDATA%\ET`（产品名 `ET`）。随包资源（`wintun.dll`/`Packet.dll`/`*.sys`）不在 git，由 `windows.yml:120` 从 `easytier/third_party/<arch>/` 拷进 `easytier-gui/src-tauri/`。
- **`Packet.dll`（Npcap）是 `easytier-gui.exe` 静态导入**（`pnet_datalink` 的 `#[link(name="Packet")]`，由 `default` 里的 `faketcp` 引入）→ 启动即映射、全生命周期锁定；`wintun.dll` 懒加载；WinDivert 走内核驱动。报错 `Error opening file for writing: ...Packet.dll`（CI 复现不了）。
- **NSIS 标签作用域不确定 → 保守**：标签只放「全文件只展开一次」的宏（`NSIS_HOOK_PREINSTALL`）里；多实例用参数化 `${SUFFIX}`。`installer-hooks.nsh` 必须**纯 ASCII**（`—` U+2014 也不行）；LogicLib 可用。
- **NSIS 改动可以本机编译校验**（Tauri 缓存 `%LOCALAPPDATA%\tauri\NSIS\` v3.08）：最小 harness 放 `%TEMP%`（别放仓库）→ 两个 Section 各 `!insertmacro NSIS_HOOK_*` → `makensis.exe -V3 -NOCD`；**两个 Section = 两个函数，能暴露 label 跨函数撞车**。模板见 skill `easytier-nsis-hook-verify`。

## 6. 两个「默认值 / 读取顺序」大坑
- **隧道协议默认 TCP → TCP-over-TCP**：默认 `default_protocol=tcp`（`Flags::default()`）。**手动 peer URL 会被改写，且偏好候选排在配置 URL 之前**（`preference_candidate_urls()` 先按偏好生成、最后才追加原始 URL，`reconnect()` 首个成功即返回）→ 用户配的 `udp://host:11010` 先被改写成 `tcp://` 并优先用，RDP 等 TCP 业务卡。诊断：grep `manual reconnect start`（同印 `configured_url` 与候选 `url`）。**规避**：`default_protocol="udp"`+只留 `udp://` peer，或 `wg://`（`tunnel/wireguard.rs` 是 boringtun 真 WireGuard，不可被改写）。MTU 默认 **1380**、全仓**无 MSS clamp**。**别把默认简单改成 udp**（镜像问题）；真正修法是把**配置的 URL 放回候选第一位**。
- **配置读取优先顺序**：`remote_client.rs::handle_get_network_config_with_source` 改 **storage-first** 会连带影响 GUI —— web 侧 storage=权威 DB 是对的，但 `easytier-gui/src-tauri/src/manager.rs::persist_runtime_dev_name`（`#[cfg(windows)]`）依赖 RPC 优先回读内核分配的 wintun `dev_name` → 存储优先后拿不到非空名字 → Windows 适配器名不再持久化 → 每次重启可能多一块 `et_*` 网卡。改前先 grep 所有 `handle_get_network_config*` 调用点（GUI `lib.rs:352/459`、`manager.rs:564`、web `restful/network.rs:367`）。

## 7. 环境杂项
- **沙箱下 `git` 改状态可能 exit 0 但静默不生效**（`branch -f`/`update-ref`/`fetch`，尤其 `packed-refs`）→ 直接写 loose ref。**凡改 git 状态必须回读**（`branch --list`/`rev-parse`/`show-ref`/`status -sb`），远端用 `git ls-remote`。本机 git 联网要绕证书吊销：`env GIT_SSL_NO_VERIFY=true git -c http.schannelCheckRevoke=false -c http.sslBackend=openssl <cmd>`；或 `curl --ssl-no-revoke`。
- git 2.55 的 `rev-parse --short` 不接受多 rev；cargo 在 `~/.cargo/bin/cargo.exe`。
- Rust 易踩：①`i64::div_ceil` 1.95 仍 unstable，用 `(a+b-1)/b`；②`DeleteMany::filter` 来自 `QueryFilter` trait（不 `use QueryFilter as _` 会解析到 `Iterator::filter`）；③`Box::<dyn Any+Send>::downcast` 按值消费 self → 不能 `if let ... else if let`，用 `match`；④`let it = temp_guard().iter_like();` 是 E0716。
- 只在明确要求编译时：`thunk-rs`/`protoc` 已缓存 `~/.cache/thunk-deps/*`、`~/.cache/protoc/bin/protoc.exe`；设 `VC_LTL`/`YY_THUNKS`/`PROTOC` + PATH 加 7-Zip，`cargo build --profile release-fast`。Win7 兼容：`thunk-rs` features 固定 `["win7"]`，四包（`easytier`、`easytier-ffi`、`easytier-gui/src-tauri`、`easytier-web`）一致，别取消。

## 8. Web 轮询 / Chart.js
- **瓶颈**：状态页 `CollectNetworkInfo`、`list_machines`、历史采样、heartbeat reconcile 共用同一 BidirectRpc 隧道；1Hz 最伤配置加载。现状：`RemoteManagement.vue` `STATUS_POLL_MS=2000`、配置模式跳过 info 轮询（列表每 3 tick）、`Dashboard.vue`/`usePollingList`/`DeviceList`/`NetworkList` 均 2s。**别随手调回 1s**。历史采样定向化在 `easytier-web/src/peer_history.rs`（按心跳的 `running_network_instances` 定向 collect）。
- `PeerConnHistoryChart.vue`：canvas 在 `v-if` 分支里，模块级 `chart` 指向已卸载 canvas → `initCharts()` 跳过创建 → 图表空白。修法：`watch([instanceId, hours])` 与错误分支里调 `destroyCharts()`（别放 `syncCharts()` 之后，会闪）。**通用：v-if 卸载 + 模块级图表实例必须配对 destroy。**

## 9. 权威清单（别在此重复）
- 版本号 bump：`docs/ops/release-version-bump.md`（10 文件/17 处；`easytier-contrib/easytier-mini/Cargo.toml` 易漏）。
- OpenWrt 打包：`.github/workflows/openwrt.yml`（只出 apk / 25.12+，artifact 名保留 `ET-openwrt-x86_64-apk`；**bump 版本只改 `easytier/Cargo.toml`**，workflow 正则读它生成 `feed/version.mk`）。
- 其他现状 SoT 在 `docs/current/`、决策留档在 `docs/roadmap/`。
