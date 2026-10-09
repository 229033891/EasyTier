# MagicDNS 静态主机通配：代码审查与文档勘误（dev 工作区）

## Status

- Status: **Archive**（审查记录；**D2 / D3 / D4 / D6-2 已在第三轮修复**，D5 有意保留；D1 / D6-1 / D7 / D8 在复核轮已修 —— 见 §6、§7）
- 日期：2026-10-09（首轮审查 → 同日 22:30 复核 → 同日 修复轮）
- 分支：`dev`（工作区未提交改动，17 文件 / +1177 −189）
- 触发：用户要求「检查当前未提交的修改，看看是否有 bug、是否符合企业级标准」
- 审查对象：
  - `easytier-core/src/config/{dns.rs,mod.rs,toml.rs}`（新增 `classify_host_name` / `HostZoneTarget`）
  - `easytier/src/instance/dns_server/{server_instance.rs,server.rs,tests.rs}`
  - `docs/current/magic-dns.md`、`easytier/locales/app.yml`、`easytier-web/frontend-lib/src/locales/{cn,en}.yaml`
  - `tauri-plugin-vpnservice/android/**`（`setSession` 改资源字符串）
- 相关 Current：[`../current/magic-dns.md`](../current/magic-dns.md)
- 相关 Roadmap：[`../roadmap/dns-policy.md`](../roadmap/dns-policy.md)

> **本地未编译**（本机 Rust C 依赖 `cl.exe` 编不过，按约定跳过）。本文 Rust 侧结论全部为**读源码静态核对**，编译与单测结论以 CI 为准。

---

## 0. 结论摘要

改动方向正确，重构比原实现更稳（消除了一条既有隐患）。**P1 是文档硬错，必须改**；P2 为设计缺口（本次未修，仅记录）。

| # | 级别 | 一句话 | 锚点 |
|---|------|--------|------|
| D1 | **P1（文档错）** | `magic-dns.md` 写「`*.c.com` **不**匹配 `a.b.c.com`」。hickory 的通配查找会**逐级剥标签**重试，`*.corp.example` **会**命中 `a.b.corp.example`（符合 RFC 4592）。文档与实现不符 | `docs/current/magic-dns.md` §3；`hickory-server-0.25.2/src/store/in_memory/inner.rs::inner_lookup_wildcard` |
| D2 | **P2（无守卫）** | `classify_host_name` 接受单标签父域（如 `*.com`）→ 装成 `com.` 权威 zone，**劫持该节点全部 `.com` 解析**。守卫只覆盖 MagicDNS TLD / route zone / split zone | `easytier-core/src/config/dns.rs:25`；`server_instance.rs:250` |
| D3 | **P2（既有缺口）** | 精确静态条目的 zone 若与某个**非本机 TLD** 的 route zone 同名，会被 `MagicDnsRecordStore::update()` 用路由记录覆盖 → 静态条目静默失效 / 来回抖。本次新增的 TLD 守卫只收口了本机 TLD | `server_instance.rs:135 update()`；`:534`、`:708` 调用点 |
| D4 | P3（行为变化） | `--dns-host` 现在**硬失败**（`?` → 进程退出）；TOML / 控制台下发的条目不校验，运行时才 warn 跳过。且旧版本里含 `*` 的名字只是被静默忽略，升级后会让启动失败 | `easytier/src/core.rs:1249`；`toml.rs:53` |
| D5 | P3（品牌不一致） | 安卓会话名改 `R.string.vpn_session_name = "ET"`，但同目录 `vpn_tile_label = "EasyTier VPN"`、`easytier-contrib` 的 `setSession("EasyTier VPN")` 未同步 → 系统 VPN 列表显示 "ET" | `TauriVpnService.kt:387`、`res/values/strings.xml` |
| D6 | P3（测试质量） | `test_static_hosts_wildcard_skipped_for_route_tld` 末段只断言「答案里没有 10.8.8.8」，响应为空时**恒真**；`can_resolve_wildcard_a_record` 用 `drop(task)` 而非 `abort()`（同文件其他测试都 abort） | `tests.rs:389`；`server.rs:537` |
| D7 | P3（流程） | `magic-dns.md` 的「最近审阅」仍写 2026-10-08，而本次已改其内容 | `docs/current/magic-dns.md` |
| D8 | **P0（复核新增，已修）** | `en.yaml` 的 `editor_help` 新文案含 `(syntax: …`，`: ` 未加引号 → **YAML 解析失败**（`bad indentation of a mapping entry`）→ 前端 `vite build` / 任何 import `i18n.ts` 的测试直接挂。已给该值加双引号 | `frontend-lib/src/locales/en.yaml:551` |

**后续状态**：D1 / D6-1 / D7 / D8 已在复核轮修掉（§6）；**D2 / D3 / D4 / D6-2 已在第三轮修掉**（§7）；**D5 有意保留**（品牌统一属产品决策，见 §7 末）。

---

## 1. 改动在做什么

静态主机（`DnsConfig.hosts`）原先只支持**精确主机名**：`app.internal.=10.1.2.3` → Catalog 里一个名为 `app.internal.` 的权威 zone，apex 一条 A。

本次加**通配**：

| 输入 | 分类结果（`HostZoneTarget`） | 装的 Catalog zone |
|------|------------------------------|-------------------|
| `app.internal.` | `zone=app.internal.`、`rr=app.internal.`、`wildcard=false` | `app.internal.`（apex A） |
| `*.corp.example` | `zone=corp.example.`、`rr=*.corp.example.`、`wildcard=true` | `corp.example.`（`*.corp.example.` A） |
| `a.*.b` / `*foo` / `*.` / 空 | `Err` | 不装 |

配套改动：

1. **分类下沉到 `easytier-core`**（`config/dns.rs::classify_host_name`），CLI 与运行时共用同一套规则。
2. **CLI 加校验**（`toml.rs::parse_dns_host_flag` 调 `classify_host_name`）。
3. **`reapply_static_hosts` 重构**：`zone → (rr_name → StaticHostRr)` 两层 map，同一父 zone 可同时放精确 apex 与通配 RR。
4. **`applied_static_zones` 语义收紧**：只记「成功 upsert 的 zone」（旧实现记「所有候选 zone」，空 IP 集合会挡住清理）。
5. **新增 `magic_tld_zone` 字段 + `static_host_skip_reason()`**：apply 路径与 `get_dns_records` RPC 状态**共用**同一判定，避免「状态页报了但实际没装」。
6. **`reload_dns_policy` 顺序调整**：`retain_zones` → `reload_split_forwarders` → `apply_static_hosts` → `reload_root_forwarder`。
7. 文案与文档同步；安卓 `setSession` 改资源字符串。

---

## 2. 静态核对通过项（逐条读源码）

| 检查项 | 结论 |
|--------|------|
| `Server::remove` / `Server::split_zones` 存在 | ✅ `server.rs:351`、`:355` |
| `magic_tld_zone` 与 route zone key 同形 | ✅ 由 `normalize_host_zone(flags.tld_dns_zone)` 派生；默认 `DEFAULT_ET_DNS_ZONE = "et.net."`（带尾点），与 `update_dns_records` 直接用 `zone` 作 key 一致 |
| `target.zone` 与 split zone 可直接比较 | ✅ `normalize_forward_zone` 输出带尾点小写（`server.rs:201`），与 `classify_host_name` 的 `zone` 同形 |
| 无锁跨 await / 无死锁 | ✅ `applied_static_zones` / `static_hosts_by_client` 的 guard 都在块内释放；`update_dns_record` 与 `on_client_disconnected` 持 `record_apply`，`reapply_static_hosts` **不**取该锁 |
| `MagicDnsServerInstance.data` 在 `tests.rs` 可见 | ✅ 字段 `pub(super)`，`tests` 是 `dns_server` 子模块 |
| mock ctx 的 `set_dns_config` 不是 no-op | ✅ `get_mock_global_ctx()` 用 `TomlConfigLoader`（`toml.rs:1202` 有真实实现） |
| `com.plugin.vpnservice.R` 无需 import | ✅ 同包 |
| 新增测试的 import 齐全 | ✅ `BaseController` / `UpdateDnsRecordRequest` / `Route` / `DEFAULT_ET_DNS_ZONE` / `Client` / `UdpClientStream` 等均在 `tests.rs` 顶部已导入；`server.rs` 测试模块有 `maplit::hashmap` |

**`reload_dns_policy` 的顺序调整是真修复**（不是等价重排）：旧顺序先装静态 hosts、再 `reload_split_forwarders`，而 split reload 会 `upsert` 同名 zone → **精确静态 zone 若与 split forwarder 同域会被 split 覆盖**；新顺序让 hosts 在 splits 之后重放，hosts 赢（B2 语义）。同理 `retain_zones` 现在把 `applied_static_zones` 与 `magic_tld_zone` 一并纳入，防止 split 清理阶段误删。

---

## 3. 缺陷详述

### D1（P1，文档硬错）通配是**多级**匹配，不是单级

`docs/current/magic-dns.md` §3 原文：

> 静态通配规则：仅 `*.parent`（单标签，如 `*.c.com` 匹配 `x.c.com`，**不**匹配 `a.b.c.com`）

**实现并非如此。** `InMemoryAuthority` 的查找在精确名未命中后进入 `inner_lookup_wildcard`（`hickory-server-0.25.2/src/store/in_memory/inner.rs:227`）：

```rust
let mut wildcard = name.clone().into_wildcard();   // 把「第一个标签」换成 *
loop {
    let Some(rrset) = self.inner_lookup(&wildcard, record_type, lookup_options) else {
        let parent = wildcard.base_name();          // 剥掉第一级，继续往上找
        if parent.is_root() { return None; }
        wildcard = parent.into_wildcard();
        continue;
    };
    // …把答案的 owner name 改回查询名
}
```

`LowerName::into_wildcard()` 的文档就是 *"Replaces the first label with the wildcard character"*（`hickory-proto-0.25.2/src/rr/lower_name.rs:164`）。因此查询 `a.b.corp.example` 会依次试 `*.b.corp.example`（未命中）→ `*.corp.example`（**命中**）。

这符合 **RFC 4592**（通配匹配「最近包围者」以下的任意层级），hickory 没有错。**要改的是文档**：

- 「语法」确实只允许最左单标签 `*.parent`（这是 `classify_host_name` 的输入约束）；
- 「匹配」是 RFC 4592 多级 —— `*.corp.example` 同样吃掉 `a.b.corp.example`。

> 影响：把 `*.corp.example` 当「只覆盖一级」用的用户会意外劫持更深层的名字（含 `foo.bar.corp.example`）。文案、CLI help、控制台 placeholder 都在暗示「单级」，需一并澄清。

### D2（P2，无守卫）单标签父域 `*.com` —— **已修，见 §7.2**

`classify_host_name` 只校验「非空 / `*` 只在最左 / 父域不空」，**不看父域层级**。于是 `--dns-host '*.com=10.1.2.3'` 会装成 `com.` 权威 zone —— 该节点上**所有 `.com` 解析**都被劫持到 `10.1.2.3`（含 `www.google.com`）。

现有守卫只覆盖：MagicDNS 路由 TLD（`et.net.`）、route zone、split zone。建议照 RFC 4592 的 `min_wildcard_depth` 思路，要求父域至少 2 个标签（或至少对单标签父域发 warning）。

### D3（P2，既有缺口，本次只收口了一半）—— **已修，见 §7.3**

`MagicDnsRecordStore::update()`（`server_instance.rs:135`）会遍历 route_store 的所有 zone 并 `update_dns_records` —— 即**用路由记录重建该 zone 的权威**。调用点：`update_dns_record`（`:534`）与 `on_client_disconnected`（`:708`），后者顺序是「先重放静态 hosts，再 `update()`」。

于是：**精确静态条目的 zone 若与某个 route zone 同名**（例：节点 join 了两个网络，本机 `tld_dns_zone=et.net.`，另一个网络的 route zone 是 `custom.local.`，用户又配了 `custom.local=1.2.3.4`），静态权威会被随后的 `update()` 覆盖 → 静态条目静默失效；下次静态重放又装回来 → 抖动。

本次新增的 `magic_tld_zone` 守卫**只覆盖本机 TLD**，非本机 TLD 的 route zone 仍无保护。修法建议：把 `static_host_skip_reason` 的 TLD 判定从「只比 `magic_tld_zone`」扩成「比 `route_zones` 全集」（或至少对精确条目也检查），或者在 `update()` 前剔除已被静态 hosts 占用的 zone。

> 注：这条**不是本次引入**（旧实现同样会覆盖），但本次既然加了守卫，就该一次做全，否则守卫给人的「已防住」错觉更危险。

### D4（P3）三条配置入口校验不一致 + 向后兼容 —— **已修（落成分层策略），见 §7.4**

- `--dns-host`：`core.rs:1249` 用 `?` 传播 → **进程退出**。
- TOML `[dns_config].hosts`：直接反序列化进 `DnsHostEntry`，**不校验**；运行时 `reapply_static_hosts` 里 `classify_host_name` 失败只 `tracing::warn!` 跳过。
- 控制台下发（managed config / `static_hosts` 通道）：同上，不校验。

且这是**行为变化**：旧版本里 `a.*.b` 这类名字会被静默忽略（且实际装出的 `a.*.b.` zone 根本不可能被查询命中，等于无效配置），现在 CLI 会直接让实例起不来。建议要么三处统一 fail-fast，要么统一 warn-skip，并在发版说明里提一句。

### D5（P3）安卓会话名与其它品牌位不一致

| 位置 | 值 |
|------|-----|
| `res/values/strings.xml` → `vpn_session_name`（本次新增，用于 `setSession`） | `ET` |
| 同文件 → `vpn_tile_label`（快捷开关瓦片） | `EasyTier VPN` |
| `easytier-contrib/easytier-android-jni/.../EasyTierVpnService.t.kt:65` | `EasyTier VPN` |

`setSession` 的值会出现在 **系统设置 → VPN 列表**和常驻通知里，用户看到的是 "ET"。若刻意缩短（对齐 Windows 安装包产品名 `ET`），建议同时把瓦片与 contrib 那份也统一，否则同一产品在系统里两个名字。

### D6（P3）测试质量（D6-1 复核轮已修；D6-2 **已修，见 §7.5**）

1. `tests.rs:389 test_static_hosts_wildcard_skipped_for_route_tld` 末段：

```rust
for ans in response.answers() {
    if let Ok(a) = ans.clone().into_parts().rdata.into_a() {
        assert_ne!(a.0, Ipv4Addr::from_str("10.8.8.8").unwrap());
    }
}
```

`random.et.net` 若返回空答案（NXDOMAIN）则循环体不执行 → **恒真**。要证明「通配被跳过」，应断言「该名字未解析到 10.8.8.8 **且** 未安装 `et.net.` 的通配 RR」（例如直接查 `get_dns_records` 的返回，或断言 `route_store`/catalog 状态）。

2. `server.rs:537 can_resolve_wildcard_a_record` 用 `drop(background_task)` 而**不是** `abort()`；同文件其它用例（以及 `tests.rs::check_dns_record`）都是 `abort(); let _ = task.await;`。detached 的后台任务在测试结束前仍持有 UDP socket。

### D7（P3）文档流程

`docs/current/magic-dns.md` 的「最近审阅」仍是 `2026-10-08（fake IP …）`，而本次已修改其 §3 内容 → 需 bump 并写明变更点（已在本次勘误中同步）。

---

## 4. 企业级评价

**做得好**

- **分层干净**：命名分类放 `easytier-core`（可被 CLI / 运行时 / 未来 Web 校验复用），安装逻辑留在 `dns_server`。
- **单一判定源**：`static_host_skip_reason()` 被 apply 与 RPC 状态共用 → 「状态页报了但实际没装」这类不一致被结构性排除。这是本次改动里最有价值的一点。
- **状态记账收紧**：`applied_static_zones` 只记成功项，修掉「空 IP 集合挡住 prune」的坑。
- **修了一条真隐患**：`reload_dns_policy` 顺序调整（见 §2 末）。
- **测试覆盖三态**：正例（精确 + 通配同 zone）、负例（非法名字）、冲突（通配撞 route TLD）。
- **文档/i18n 同步意识**：改行为同时改了 CLI help、控制台文案、Current 文档。

**待补**（首轮时点；D1–D4 后续均已修，见 §6 / §7）

- 文档与实现不符（D1，硬错）。
- 缺少「父域最小深度」这类输入护栏（D2）。
- 守卫范围与文档承诺不匹配（D3）。
- 三条配置入口校验策略不统一（D4）。
- Rust 单测本机跑不了 → **必须确认 CI 真跑 `easytier` 的 `dns_server` 单测**，否则新测试等于没验证。**这一条至今仍未闭环。**

---

## 5. 本次已随审查同步的文档

| 文件 | 改动 |
|------|------|
| `docs/current/magic-dns.md` | §3 修正通配匹配语义（RFC 4592 多级）+ 补语法/守卫/未守卫说明；§9 补两条局限；bump 最近审阅到 2026-10-09 |
| `docs/roadmap/dns-policy.md` | §2「仍开放」新增 D2 / D3；§3 拍板摘要新增「通配 = RFC 4592 多级，文档不得写成单级」 |
| `docs/README.md` | Archive 索引登记本文 |

**首轮未改代码**：D2 / D3 的守卫、D4 的校验统一、D5 的品牌统一、D6 的测试加固均留待后续（见 Roadmap §2）。复核期作者已修掉其中两项，另新增 D8（已修），见 §6；**修复轮又落地 D2 / D3 / D4 / D6-2，仅 D5 有意保留，见 §7。**

---

## 6. 复核（同日 22:30，作者改过代码后）

复核对象：工作区最新改动（较首轮 +29 行 `server.rs`、三个语言文件文案）。

| 项 | 状态 | 说明 |
|----|------|------|
| D1 文案 | **已修** | `cn.yaml` / `en.yaml` 的 `dns.hosts.editor_help` 改为「语法仅最左单标签 `*`；匹配按 RFC 4592 多级，如 `*.corp.example → foo.corp.example 与 a.b.corp.example`」；`easytier/locales/app.yml` 的 `core_clap.dns_host` 同步。与 §3 的 Current 表述一致 |
| D6-1 测试卫生 | **已修** | `can_resolve_wildcard_a_record` 由 `drop(background_task)` 改为 `background_task.abort(); let _ = background_task.await;`，与同文件其它用例一致 |
| **D6 附加** | 已加强 | 该测试新增 `a.b.corp.example` 断言（1 条 answer 且为 `10.9.9.9`），**把 D1 的 RFC 4592 多级语义锁进回归**，以后想收窄成单级会立刻红 |
| D8 YAML 语法错 | **新发现 → 已修** | 见 §0 表。`en.yaml` 的 `(syntax: ` 触发 js-yaml `bad indentation of a mapping entry (551:131)`；`@modyfi/vite-plugin-yaml` 正是用 `js-yaml` 的 `load`，故 `vite build` 与任何 import `i18n.ts` 的测试都会挂。已给该值加双引号（值内无 `"` / `\`，安全） |
| D2 父域最小深度 | **第三轮已修** | `MIN_STATIC_HOST_ZONE_LABELS = 2` + `HostZoneTarget::reject_reason()`，见 §7 |
| D3 静态 zone 与 route zone 同名 | **第三轮已修** | `static_host_skip_reason` 改为对 `route_zones` 全集判定，**精确条目也查**，见 §7 |
| D4 三入口校验统一 | **第三轮已修**（落成「分层」而非「三处 fail-fast」） | CLI 硬失败 / TOML 加载时 + 运行时 warn / 托管仅运行时 warn，共用同一判定，见 §7 |
| D5 安卓会话名品牌统一 | 未修（**有意保留**） | `vpn_session_name` 仍为 `ET`；与瓦片/contrib 统一属产品决策，非缺陷 |
| D6-2 弱断言 | **第三轮已修** | 改为 `assert!(answers.is_empty())` + 状态 RPC 交叉断言，并新增 route-zone 抢占回归，见 §7 |
| D7 审阅日期 | **已修** | Current / Roadmap 均已 bump 到 2026-10-09 |

### 6.1 复核验证记录

- **YAML 解析**：用插件同款解析器（`js-yaml@4.1.0` 的 `load`）跑 `en.yaml` / `cn.yaml` / `easytier/locales/app.yml` → 三个文件全部 OK，`editor_help` / `dns_host` 取值符合预期（修 D8 前 `en.yaml` 必失败）。
- **前端套件**：`easytier-web/frontend-lib` → `vitest run --config vitest.config.ts` → **15 文件 / 114 passed**，与基线一致（locales 经 `src/modules/i18n.ts` 被各组件测试加载，可覆盖 D8 的回归）。
- **Rust**：仍按约定不本地编译；`server.rs` 新增断言的正确性依据是 §3-D1 的 hickory 源码结论（`inner_lookup_wildcard` 逐级 `base_name()`），需 CI 跑 `easytier` 的 `dns_server` 单测坐实。
- 未跑 `vue-tsc`（本次改动只有 YAML 值，无 TS 变更）。

### 6.2 复核结论

D8 是**会直接打断前端构建**的一类错（文案里写 `: ` 忘了加引号），已修并验证；D1 的语义澄清与 D6-1 的测试卫生作者已按建议处理，且额外把多级匹配锁进了回归，这部分质量到位。当时 **D2 / D3 仍开放**（`*.com` 可配、非本机 TLD 的静态条目会被 `update()` 覆盖）—— 已在第三轮修复，见 §7。

---

## 7. 修复轮（D2 / D3 / D4 / D6-2 已落地）

改动 5 个 Rust 文件：`easytier-core/src/config/{dns.rs,mod.rs,toml.rs}`、`easytier/src/instance/dns_server/{server_instance.rs,tests.rs}`（合计约 730 行改动）。

### 7.1 策略收口到 `easytier-core`（D2 与 D4 的共同基础）

`easytier-core/src/config/dns.rs` 新增：

```rust
pub const MIN_STATIC_HOST_ZONE_LABELS: usize = 2;
pub const SINGLE_LABEL_ZONE_REASON: &str =
    "single-label zone: static hosts must own at least two labels (e.g. corp.example)";

impl HostZoneTarget {
    pub fn zone_label_count(&self) -> usize { /* zone 去尾点后按 '.' 切，过滤空标签 */ }
    pub fn reject_reason(&self) -> Option<&'static str> {
        if self.zone.is_empty() || self.zone == "." { return Some("empty zone"); }
        if self.zone_label_count() < MIN_STATIC_HOST_ZONE_LABELS { return Some(SINGLE_LABEL_ZONE_REASON); }
        None
    }
}

impl DnsConfig {
    /// 三个入口共用的「哪些条目该跳过、为什么」
    pub fn invalid_hosts(&self) -> Vec<(&str, String)> { /* classify → reject_reason → Err */ }
}
```

`config/mod.rs` 增补导出 `HostZoneTarget` / `MIN_STATIC_HOST_ZONE_LABELS` / `SINGLE_LABEL_ZONE_REASON` / `classify_host_name`。

### 7.2 D2：单标签 zone 一律拒绝

`toml.rs::parse_dns_host_flag` 在原有语法校验后多走一步策略判定：

```rust
let target = super::dns::classify_host_name(&name)
    .map_err(|e| anyhow::anyhow!("invalid dns-host name in `{raw}`: {e}"))?;
if let Some(reason) = target.reject_reason() {
    anyhow::bail!("invalid dns-host name in `{raw}`: {reason}");
}
```

**精确条目也挡**：`com` 这种精确单标签 zone 与 `*.com` 是同一失效模式（拥有整个 TLD 的权威），故 `reject_reason()` **不看** `is_wildcard`。

单测：`single_label_zones_are_rejected`（`*.com` / `com` 被拒，`*.corp.example` / `corp.example` 通过，`app.internal.`=2 标签、`a.b.c.d`=4 标签）、`invalid_hosts_reports_reasons`；`parse_dns_host_flag` 补 `*.com=1.1.1.1` / `com=1.1.1.1` 的 `is_err()` 断言。

### 7.3 D3：route zone 冲突对**精确条目**也生效

`server_instance.rs::static_host_skip_reason()` 重写，判定顺序：

1. `target.reject_reason()` —— 共用语法/策略结论（含 D2 的单标签判定）；
2. `route_zones.contains(&target.zone)` → `conflicts with MagicDNS route zone` —— **精确与通配都查**（这就是 D3 的修法）；
3. `target.zone == self.magic_tld_zone` → `conflicts with MagicDNS TLD zone`（该 zone 在 `new()` 里早于任何路由装好，可能不在 route 快照里，故单独判一次）；
4. `target.is_wildcard && split_zones.contains(&target.zone)` → `wildcard conflicts with split Catalog zone`（精确子域仍允许覆盖同名路由 hostname，B2）。

apply 路径与 `get_dns_records` RPC **共用**该函数，因此状态页不会再广告「实际已被跳过」的条目（原来两处判定各写一份，正是漂移源）。

回归测试 `test_static_host_exact_zone_cannot_steal_route_zone`（新增）：先让 peer 路由 `custom.local.`（hostname `peer1`），再 `apply_static_hosts(&["custom.local=10.1.1.1"])`，断言 `peer1.custom.local → 10.144.144.43`，且状态 RPC 的 A 记录里**没有** `custom.local.`。

### 7.4 D4：落成「分层策略」而非三处 fail-fast

三处入口的**判定**共用（`reject_reason` / `invalid_hosts`），**处置**按来源分层：

| 入口 | 处置 | 理由 |
|------|------|------|
| CLI `--dns-host` | **硬失败**（进程退出） | 本地命令行，用户当场就能改 |
| TOML `[dns_config].hosts` | 加载时 `warn!`（带 `source` 文件名）+ 运行时再 warn 跳过 | 配置文件常是从别处拷来的，不该让节点起不来 |
| 控制台 / 托管下发 | 仅运行时 warn 跳过 | 远端配置**永远不能**把节点弄死 |

新增 `TomlConfig::warn_invalid_dns_hosts(source_name, config)`，在 `new_from_str_with_source` 的 `normalize_config_source` 之后调用。比「三处都 fail-fast」更贴合企业级预期：**输入源可信度决定严格度**。

### 7.5 D6-2：弱断言换成真断言

`test_static_hosts_wildcard_skipped_for_route_tld` 末段原为「遍历 answers 断言没有 10.8.8.8」（空答案时恒真），改为：

```rust
assert!(response.answers().is_empty(), "skipped wildcard must not synthesize answers: {:?}", response.answers());
```

并补状态 RPC 交叉断言（无 `*` 记录被广播、`node1.et.net.` 仍在）。另新增 `test_static_hosts_wildcard_a_record`（精确 apex 与通配共用一个父 zone）。

### 7.6 验证与残留

- **Rust**：按项目约定**不本地编译**。5 个改动文件跑 `rustfmt --edition 2024 --emit stdout` → **PARSE OK**（只证明语法）。`dns.rs` 的 `#[cfg(test)]` 单测与 `tests.rs` 的新用例需 **CI 跑 `easytier` 的 `dns_server` 单测**坐实 —— 这是本轮唯一未闭环项。
- **前端**：本轮未再动 locales。D8 的双引号修复已在复核轮验证（三个语言文件用 `js-yaml@4.1.0` 的 `load` 全 OK；`frontend-lib` vitest 15 文件 / 114 passed）。
- **D5 有意保留**：安卓 `vpn_session_name = "ET"` 与瓦片 `EasyTier VPN` 不一致，属产品命名决策（是否全面对齐 Windows 安装包的 `ET`），不是缺陷，等产品侧拍板再动。

### 7.7 同步的文档

| 文件 | 改动 |
|------|------|
| `docs/current/magic-dns.md` | §3 重写「静态 hosts 通配规则」（语法 + ≥2 标签 / 四条守卫逐条 / 三入口分层）；§9 删掉已修的 3 条局限，换成一条「行为变化（发版需注明）」；bump 最近审阅 |
| `docs/roadmap/dns-policy.md` | §1 补两条已落地护栏；§2 删掉 3 条开放项；§3 新增拍板 9（≥2 标签）、10（不得占用 route/TLD zone） |
| 本文 | §0 状态说明 / §6 表 / §6.2 / 新增 §7 |
