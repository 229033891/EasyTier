# OpenWrt / ImmortalWrt 节点部署必检项（流量卸载 / 同网段主机路由）

- Status: **Ops**
- 最近审阅：2026-11-14（§4.3 补入 G1/G2 分工、F2 实现约束与升级迁移）
- 索引：[`../README.md`](../README.md)
- 关联：[`et-kernel-forward-check.md`](./et-kernel-forward-check.md)（转发口径巡检）、[`../roadmap/default-route-and-underlay-excludes.md`](../roadmap/default-route-and-underlay-excludes.md)（underlay 排除路由设计）

路由器节点做**子网代理**（`--proxy-networks`）时，以下两项若配错，会出现「网页打得开一半、大文件正常、某些接口卡十几秒」这类极难定位的故障。**新节点上线前按第 1 节跑一遍。**

---

## 1. 上线必检（两条命令）

```bash
# ① 流量卸载必须关闭（默认可能是开的）
uci get firewall.@defaults[0].flow_offloading      # 必须为 0
uci get firewall.@defaults[0].flow_offloading_hw   # 必须为 0

# ② 检查是否被装了「同网段对端」的主机路由（见第 4 节）
#    只看 host route（/32、/128）：exit-node 的 /0 默认路由同样是「via 网关 + metric 1」，
#    不加 /32|/128 过滤会把 exit 默认路由误报成残留。
ip route show | grep -E '/(32|128) via' | grep " via $(ip route show default | awk '{print $3; exit}')"
```

第 ① 项不为 0 时立即：

```bash
uci set firewall.@defaults[0].flow_offloading='0'
uci set firewall.@defaults[0].flow_offloading_hw='0'
uci commit firewall
/etc/init.d/firewall restart
```

LuCI 路径：**网络 → 防火墙 → 路由/NAT 卸载 → 流量卸载类型 = 无**
（部分版本在「常规设置」区显示为「启用 Flow Offloading」复选框，文案随版本变化，以 `uci get` 为准。）

---

## 2. 故障现象（开启流量卸载时）

| 现象 | 说明 |
|---|---|
| `Transfer-Encoding: chunked` 的响应必卡 **10~15s** | 最终仍返回 200，但用户侧表现为「页面转圈十几秒」 |
| `Content-Length` 的响应完全正常（<1s） | 与小文件/大文件无关，只与**响应是否依赖「突发之后的小包」**有关 |
| 抓包：尾段被服务端重传 6 次后 RST | `et` TUN 上能看到内核已注入，但对端从未 ACK 该 seq |
| 关闭流量卸载后瞬间恢复 | 0.73s 稳定，A/B 可逆复现 |

**根因**：`flow_offloading=1` 启用 nftables flowtable，已建立的 TCP 连接被卸载进快路径；后续报文经快路径交给 `et` TUN 后，在 EasyTier 用户态转发环节被丢弃（无日志）。

**触发条件**：服务端响应最后一个 chunk（`0\r\n\r\n`，通常只有 5 字节、滞后数据突发约 100~200ms 才发送）无法到达客户端 → 客户端等 TCP 重传退避（≈3.7s）→ 服务端 RST → 客户端重连并重做认证 → 累计 10~15s。

**实测对照**（ImmortalWrt 25.12.1 / x86_64 / EasyTier 2.7.11~2.7.12）：

| 流量卸载 | chunked 页面 | Content-Length 页面 |
|---|---|---|
| 软件流量卸载（开） | ❌ 14.98s ×5 | ✅ 0.34s |
| 无（关） | ✅ 0.72s ×9 | ✅ 0.34s |

> 同一客户端改走 Windows / Debian 节点代理时完全正常 —— 说明问题出在 OpenWrt 的 flowtable，与 EasyTier 版本、平台、客户端无关。

---

## 3. 为什么流量卸载会破坏子网代理

flowtable 是「内核快路径」：命中的报文跳过 netfilter 常规转发链，直接投递到出接口。`et` 是**用户态 TUN**，其收包路径依赖常规的 `forward` 链行为（含 conntrack/NAT 后的形态）。被卸载后交给 TUN 的报文形态与常规路径不一致，EasyTier 读取后无法正常转成隧道包，于是表现为**静默丢包**。

排查时容易踩的坑：

- **ping / 小流量正常** —— ICMP 与隧道的 pingpong 心跳不走被卸载的转发路径，不能作为「链路正常」的判据；
- **`ethtool -K et gro/gso/tso off` 无效** —— 那是 TUN 设备自身的 offload，与本项无关；
- **换 EasyTier 版本无效** —— 2.7.11 / 2.7.12 表现一致；
- **Windows 侧抓包看不到入站包** —— wintun 走共享内存 ring，`pktmon` 只记录 Tx，勿据此判断「包没到」。

---

## 4. 已知遗留：同网段对端被装 `via 网关` 主机路由

现场观察到路由器上出现：

```
192.168.10.101/32 via 192.168.10.1 dev eth0    # 对端与本地同属 192.168.10.0/24
```

### 4.1 代码链（已逐行确认）

| 环节 | 位置 | 行为 |
|---|---|---|
| 采集 | `easytier-core/src/gateway/proxy/underlay_exclude.rs:16` `collect_underlay_exclude_ips()` | 收 peer 隧道 `resolved_remote_addr` + 对端 STUN 公网 IP + config-server IP；`should_exclude_ip()`（`:69`）只排除 overlay VIP / managed IPv6，**不排除 on-link 同网段地址** |
| 门闩 | `easytier-core/src/instance/packet_plane.rs:244` `needs_underlay_excludes()` | 返回**布尔**：`desired` 含 `/0`，或「某个 underlay IP 被 desired 覆盖 **且** 不在本机物理网段内」。**这里考虑了 on-link，但只用来决定「装不装」，不决定「给谁装」** |
| 门闩调用 | `easytier/src/instance/virtual_nic.rs:1281-1285`（install 路径）、`:1966-1969`（tick 路径） | `needs_excludes` 为真 → 调用安装器，传入的是**全量** `exclude_desired` |
| 安装 | `easytier/src/instance/virtual_nic.rs:1558` `reconcile_underlay_exclude_routes()`，`:1667` `to_add = desired.difference(installed)` | 直接 `add_ipv4_host_route(ip, via=物理默认网关, metric=1)`，**全程不判 on-link** |
| 清理 | `easytier/src/instance/runtime_host/tun_common.rs:211` `cleanup_underlay_exclude_routes()` | 同样按「当前」`underlay_exclude_ips()` 推导；集合为空时直接 `return`，因此对端已断开 / 实例停止后**可能残留**（现场实测残留） |

### 4.2 触发条件

只要**存在一个**「被 proxy CIDR 覆盖、且不在本机物理网段内」的对端 underlay IP，门闩就打开，于是**整份 exclude 集合**（含同网段的 192.168.10.101）都被钉到物理默认网关。

现场即如此：`shdx` 代理 `172.16.0.0/16`，同时存在 `172.18.10.10/32`、`172.18.10.200/32 via 192.168.10.1`（同一批 exclude 路由，其地址落在 172.16/16 内）→ 门闩打开 → 连 `192.168.10.101/32`（DEV，同属 192.168.10.0/24）也被钉到网关。

`/32` 比 `/24` 直连路由更具体，**metric 1** 使其无条件优先 → 回给同网段对端的包全部绕行公司网关。

**G2（清理残留）的触发条件比上面宽得多**：只要装过任意一条 exclude 路由、且停止时采集集合为空，就会残留 —— 不需要同网段对端，exit-node `/0` 更是必装。§6 第 2 例的 8 条里有一半是公网 IP，即由此而来。

### 4.3 规避与修复

**临时止损**（同网段本就不需要该路由）：

```bash
ip route del 192.168.10.101/32
# 开机纠正（写入 /etc/rc.local，exit 0 之前）
# ip route del 192.168.10.101/32 2>/dev/null
```

**只删同网段那几条不够。** 公网 `/32` 同样是 ET 残留（§6 第 2 例的 `43.154.13.211` / `101.43.96.62` / `124.222.29.94` / `172.25.0.1`）。完整判定、留档与逐条删除命令见 [`docker-compose-deploy.md`](./docker-compose-deploy.md) §2.7 第 3 步；**不得动 `ip rule` / `table 10|11`**（飞牛自身双网卡策略路由，非 ET 产物）。

**代码级**（**F1 + F2 已落地（2026-11-14），待自测与现网验收**；设计与代码锚点唯一来源：[`../roadmap/underlay-exclude-onlink-filter.md`](../roadmap/underlay-exclude-onlink-filter.md) §4 / §8.5）：

| 缺口 | 修复 | 能解决什么 |
|---|---|---|
| **G1** 门闩是集合级布尔、安装不判 on-link | **F1（已落地）**：`collect_underlay_exclude_ips()` 采集阶段就丢 on-link 地址（来源与 `set_local_ipv4_networks` 同源，`None` 未注入时保守放行） | 只防「同网段 `/32` 被装」 |
| 备选（未采用，留作记录） | 在 `reconcile_underlay_exclude_routes()` 的 `to_add` 上按 `local_ipv4_networks` 过滤：只改安装侧，清理侧仍按全量，两侧不自洽 | 同 G1，但清理不自洽 |
| **G2** 清理按「当前采集」推导 | **F2（已落地）**：安装器逐条发布「已安装集合 + 安装时 via 快照」到平面状态，stop 期清理按该状态卸载（失败条目保留记账待重试），不再按当前采集推导 | 清掉**所有**残留，含公网条目 |
| 防御（defer） | F3：装前二次校验 on-link | IPv6 on-link / 网段信息长期不可用 |

> **G1 与 G2 修的对象不同（易遗漏）**：§6 第 2 例的 8 条中，4 条公网 IP 本就该被排除、与 on-link 无关，**F1 对它们完全无效，只有 F2 能清掉**；4 条同网段才是 G1 的锅。所以「只做 F1 就完事」是错的，**F2 优先级不低于 F1**（它是「卸载不干净」的直接原因，且覆盖面更广）。

> **升级不会自动清掉存量残留**：F1 之后 on-link 地址连 `desired` 都进不去，F2 的 `installed` 也不会记录它们（新进程从空开始）；只有再次进入 `desired` 的 IP 才会因 EEXIST 被重新纳管。⇒ 修复上线包**必须附一次性清理**，按 §2.7 第 3 步执行（Roadmap §4.4）。不做开机全表扫描自动删，以免误删用户路由。

**F2 实现约束**（**SoT = [`../roadmap/underlay-exclude-onlink-filter.md`](../roadmap/underlay-exclude-onlink-filter.md) §4 / §8.5**，实现细节（via 快照 / 共享状态位置与跨 crate 约束 / 发布粒度）不在本文复制，防两处漂移。运维侧只需知道三点）：

1. **升级不会自愈**：F1/F2 都清不掉存量 `/32`，上线包**必须**附一次性清理，按 [`docker-compose-deploy.md`](./docker-compose-deploy.md) §2.7 第 3 步执行（Roadmap §4.4）；
2. **验收断言口径**：§1 第 ② 项（已收紧为 `/32|/128 via <物理网关>`）—— 不要用 `grep 'metric 1'`（exit-node `/0` 默认路由同为「via 网关 + metric 1」，会误报）；
3. **验收环境**：`shdx` 上做 stop 断言需要维护窗口（会中断现网代理），提前排期。

---

## 5. 排查顺序建议（同类问题复用）

1. 按**响应传输方式**分组（`Content-Length` vs `chunked`）—— 一步就能把「网络慢」收缩到「流尾部小包丢失」；
2. 在代理节点 `et` 接口抓包，确认尾包是否被注入 TUN，再用「对端是否 ACK 该 seq」判定丢在哪一跳；
3. 关掉路由器流量卸载做 A/B —— 最快的一次验证；
4. 排除版本/平台/客户端：换一个非 OpenWrt 节点做同样的代理，若正常则锁定本机环境。

---

## 6. 现场记录

- 2026-11-14：ImmortalWrt 25.12.1 路由器节点开启软件流量卸载 → `transfer-encoding: chunked` 页面卡 10~15s；关闭后 0.72s ×9 稳定。诊断过程与 A/B 明细见内部现场报告（未入库），本文 §1–§3 即为可执行结论。
- 2026-11-14（**第 2 例，Docker 形态**）：飞牛 NAS `FNNAS`（`6.18.18.c1186-trim`，双网卡同网段 `ens192`=192.168.10.105 / `ens256`=192.168.10.106）的 ET 容器 / 镜像 / 二进制**全部卸净**后，主表仍留 8 条 `/32 via 192.168.10.1 dev ens256 proto static metric 1` —— 4 条是同网段主机（本文 §4 的 G1 面），4 条是公网 peer 与控制台 `43.154.13.211`（**只有清理侧缺欠能解**，on-link 过滤无效）。来源已用「配置文件 grep 无命中 + `ip rule` / `table 10|11` 非 ET 产物」双重排除，确认属本节已知遗留。**Docker 卸载后的清理口径见** [`docker-compose-deploy.md`](./docker-compose-deploy.md) §2.7；**缺陷与排期见** [`../roadmap/underlay-exclude-onlink-filter.md`](../roadmap/underlay-exclude-onlink-filter.md) §2.2 / §8。
