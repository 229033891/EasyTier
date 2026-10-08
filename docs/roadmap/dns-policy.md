# DNS 策略：剩余缺口与决策留档

## Status

- Status: **Roadmap**（R1–R3 / B6 / Linux drop-in / JNI 对齐 **已落地**；现状 SoT → [`../current/magic-dns.md`](../current/magic-dns.md)）
- 最近审阅：2026-10-06
- 本文只保留**未完成项**与拍板依据摘要；完整实施日志已收敛进 Current，勿再当「待实现全表」读。
- 手工接线：[`../current/magic-dns-manual-wiring.md`](../current/magic-dns-manual-wiring.md)
- 相关：[`domain-proxy.md`](./domain-proxy.md)（域名→CIDR 仍未做）、[`traffic-steering.md`](../current/traffic-steering.md)
- 索引：[`../README.md`](../README.md)

---

## 1. 已落地（勿重复排期）

见 Current：[`magic-dns.md`](../current/magic-dns.md)。摘要：

- `DnsConfig` hosts / forwarders / upstream_dns + managed + MagicDNS `static_hosts` 通道  
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

域名代理是否依赖 MagicDNS 覆盖：见 [`domain-proxy.md`](./domain-proxy.md)，不在本文扩 scope。
