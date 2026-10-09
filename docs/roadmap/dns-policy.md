# DNS 策略：剩余缺口与决策留档

## Status

- Status: **Roadmap**（R1–R3 / B6 / Linux drop-in / JNI 对齐 **已落地**；现状 SoT → [`../current/magic-dns.md`](../current/magic-dns.md)）
- 最近审阅：2026-10-09（静态 hosts 通配审查；本轮已落地「父域 ≥2 标签」「静态 zone 不得占用 route/TLD zone（精确与通配一致）」「三入口校验分层」三项，见 §1）
- 本文只保留**未完成项**与拍板依据摘要；完整实施日志已收敛进 Current，勿再当「待实现全表」读。
- 手工接线：[`../current/magic-dns-manual-wiring.md`](../current/magic-dns-manual-wiring.md)
- 相关：[`domain-proxy.md`](./domain-proxy.md)（域名→CIDR 仍未做）、[`traffic-steering.md`](../current/traffic-steering.md)
- 索引：[`../README.md`](../README.md)

---

## 1. 已落地（勿重复排期）

见 Current：[`magic-dns.md`](../current/magic-dns.md)。摘要：

- `DnsConfig` hosts（含 `*.suffix` 通配；**匹配语义见 Current §3**）/ forwarders / upstream_dns + managed + MagicDNS `static_hosts` 通道  
- 静态 hosts 输入护栏（2026-10-09）：zone **≥2 标签**（拒 `*.com` / `com`）；zone 与 **route zone 同名**（精确与通配都查）或等于 MagicDNS 路由 TLD → 跳过；通配父 zone 被 Split 占用 → 跳过。守卫与状态 RPC 共用 `static_host_skip_reason()`  
- 静态 hosts 三入口校验分层：CLI 硬失败 / TOML 加载时 + 运行时 warn / 托管下发仅运行时 warn；判定共用 `HostZoneTarget::reject_reason()` 与 `DnsConfig::invalid_hosts()`  
- 串行 failover 上游、防环绑物理网卡、热重载  
- 平台接线矩阵 + 控制台覆盖徽章 + `magic_dns_os_wired`  
- fake IP `10.255.255.254`  

历史长文（需求 / 竞速论证 / 落地顺序 / B1–B6 勾选）已吸收进 Current；需要争议上下文时查 git 历史。

---

## 2. 仍开放

| 项 | 说明 | 建议优先级 |
|----|------|------------|
| DoT / DoH / 域名上游 | 首期仅明文 IP；另立项 | 低（按合规需求） |
| Fake IP 可配化 | 仍硬编码；切地址成本高 | 低（慎做） |
| 发布 / 解析开关拆分 | headless「只被解析、不接管本机 DNS」 | 中（有用户诉求再开） |
| iOS / macOS-NE 接线 | 明确不支持；需走 NE DNS 设置另案 | 中（平台产品驱动） |
| 与域名子网代理联动 | W2 / `domain-proxy.md`：答案同步保证打中 CIDR | 随 domain-proxy |
| Hedged query | 首包超时后再问第二上游；hickory 无内建 | 低（尾延迟） |
| 选举偏好新版本 server | 旧节点抢 `:49813` 丢 static_hosts；现靠控制台 warning | 低 |
| 双建模收敛 | `StaticDnsHost` vs `DnsHostEntry` | 低（重构） |

已知发版注明即可的局限（残留 drop-in、离线徽章偏 `manual_required` 等）写在 Current §9，不单独立项。

---

## 3. 拍板摘要（防止回潮）

仍有效、Current 已写死的决策：

1. 上游 **顺序 failover，不竞速**（NXDOMAIN / GeoDNS / 合规）。  
2. `upstream_dns` / split 未配 → **系统 DNS**，禁止空=NXDOMAIN。  
3. hosts **赢** 路由 hostname。  
4. Fake IP **固定** `10.255.255.254`；否决公网段。  
5. 首期 **一个** `enable_magic_dns`（发布+解析耦合）。  
6. 覆盖验收：**禁止**用「配置已下发 + 心跳存活」冒充成功。  
7. 静态 hosts 通配的**匹配语义 = RFC 4592 多级**（`*.corp.example` 也命中 `a.b.corp.example`）；「单标签」只描述**配置语法**，文档与 UI 文案不得写成「只匹配一级」。  
8. 静态 hosts 与 Split forwarder 同域时 **hosts 赢** —— `reload_dns_policy` 里 hosts 必须重放在 `reload_split_forwarders` **之后**，且 `retain_zones` 要含 `applied_static_zones` + `magic_tld_zone`。
9. 静态 hosts 拥有的 Catalog zone **至少 2 个标签**（`MIN_STATIC_HOST_ZONE_LABELS = 2`）。单标签 zone = 拥有整个 TLD（`*.com` → `com.` 权威），一律拒绝；精确与通配同规。
10. 静态条目**不得**占用 route zone（精确与通配都查）或 MagicDNS 路由 TLD：`MagicDnsRecordStore::update()` 会在路由变化时重建该 zone 的权威，静态条目会被覆盖 → 抖动。冲突时静态让路（warn），「hosts 赢」只适用于**精确子域**覆盖同名路由 hostname（B2）。

域名代理是否依赖 MagicDNS 覆盖：见 [`domain-proxy.md`](./domain-proxy.md)，不在本文扩 scope。
