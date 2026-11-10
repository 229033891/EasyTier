# 隧道节点巡检：内核转发 / 设备健康 / 数据面线程

- Status: **Ops**
- 最近审阅：2026-11-10
- 索引：[`../README.md`](../README.md)
- 关联：[`../roadmap/tun-data-plane-throughput-2026-11.md`](../roadmap/tun-data-plane-throughput-2026-11.md)（吞吐瓶颈实测与改造候选）

Linux（Ubuntu / OpenWrt / Debian）节点上排查「隧道通但跨网段不通」「吞吐上不去」「延迟抖动」时的巡检口径。**先分清三种转发，否则会测错方向。**

---

## 0. 三种转发，只有一种用内核 `ip_forward`

| 场景 | 数据路径 | `et` 设备计数 | `IpForwDatagrams` | 内核 forward |
|---|---|---|---|---|
| **relay 中转**（A→本机→B） | 本机 socket 收，另一 socket 发 | **不涨** | 不涨 | 不参与 |
| **三层网关**（子网代理 / 出口节点） | `et` ↔ 物理网卡走内核路由表 | 涨 | **涨** | 参与 |
| 本机访问隧道对端 | 本机 socket ↔ `et` | 涨 | 不涨 | 不参与 |

「内核转发是否正常」这个命题**只对第二行成立**。节点只做 relay 时，`ip_forward` 写什么都不影响结果。

---

## 1. 一键巡检脚本

```bash
#!/bin/bash
# et-fwd-check.sh [隧道网卡名]
DEV=${1:-et}
PHY=$(ip route show default | awk '{print $5; exit}')
echo "== 0. identity =="; hostname; uname -r; nproc; echo "dev=$DEV phy=$PHY"

echo; echo "== 1. links =="
ip -br addr show "$DEV"; ip -br addr show "$PHY"
ip -d link show "$DEV" | sed -n '1,4p'

echo; echo "== 2. 转发开关 + rp_filter =="
sysctl net.ipv4.ip_forward net.ipv6.conf.all.forwarding 2>/dev/null
for f in /proc/sys/net/ipv4/conf/*/rp_filter; do printf '%-42s=%s\n' "$f" "$(cat "$f")"; done

echo; echo "== 3. 路由 / 策略规则 =="
ip route show table all | grep -E "$DEV|10\.10\.10\." | head -20
ip rule | head

echo; echo "== 4. 计数器 5s 增量（不依赖 nstat） =="
s(){ awk '/^Ip:/{if(f++)print $4,$7,$10,$11,$13}' /proc/net/snmp; }
read -r a b c d e <<<"$(s)"; sleep 5; read -r A B C D E <<<"$(s)"
echo "InReceives +$((A-a))  ForwDatagrams +$((B-b))  InDelivers +$((C-c))  OutRequests +$((D-d))  OutNoRoutes +$((E-e))"
grep -E 'Inter-|'"$DEV"':' /proc/net/dev

echo; echo "== 5. 防火墙规则（改名后易残留） =="
iptables -S FORWARD 2>/dev/null | head -20 || echo "(no iptables)"
nft list ruleset 2>/dev/null | grep -c . || echo "(no nft)"
ufw status verbose 2>/dev/null | head -12 || echo "(no ufw)"
conntrack -S 2>/dev/null | head -3 || echo "(no conntrack-tools)"

echo; echo "== 6. 丢包 / 队列 =="
awk '{d+=$2; q+=$3} END{print "softnet dropped="d"  time_squeeze="q}' /proc/net/softnet_stat
ip -s link show "$DEV"
tc -s qdisc show dev "$DEV" | head -6

echo; echo "== 7. offload =="
ethtool -k "$DEV" 2>/dev/null | grep -E 'tcp-segmentation|generic-segmentation|generic-receive|tx-checksumming|scatter-gather'

echo; echo "== 8. 数据面线程（需有流量时才有意义） =="
P=$(pgrep -x ET-core || pgrep -f '/ET-core' ); echo "core pid=$P"
if [ -n "$P" ]; then
  top -H -b -d 1 -n 5 -p "$P" | awk 'NR>7 && $9+0>0.5'
else
  echo "未找到 ET-core，按实际进程名调整（见 §3）"
fi
```

---

## 2. 判据表

| 检查项 | 期望 | 不正常时的含义 |
|---|---|---|
| `ip -br addr show et` | `UP` + `10.10.10.x/24`，`POINTOPOINT` | 设备名未生效 / 配置仍写旧名 |
| `ip_forward` | `1`（仅当要当网关） | `0` → 三层转发一律不通；relay 不受影响 |
| `rp_filter` | `0` 或 `2`，尤其 `all` 与 `et` | `1` → 非对称路由下静默丢包，「隧道通但跨网段不通」首要嫌疑 |
| `IpForwDatagrams` 增量 | 跨网段打流时 ≈ 包速率 | 恒 0 → 流量根本没进内核 forward（是 relay 或本机发起） |
| `InReceives` 与被加数 | `InReceives = ForwDatagrams + InDelivers` | 不相等 → 有 `IpInAddrErrors`/`InHdrErrors` 被吞 |
| `OutNoRoutes` | 恒 0 | 增长 → 路由黑洞 |
| `et` 的 errors / dropped / missed | 全 0 | 非 0 → 设备侧或内存问题 |
| `softnet_stat` 第 2 列 | 恒 0 | 增长 → 软中断队列丢包，调 `net.core.netdev_max_backlog` / RPS |
| `tc -s qdisc` 的 dropped | 0 | 增长 → 本机限速/队列过载 |
| `conntrack -S` 的 `insert_failed` | 0 | 增长 → `nf_conntrack: table full` |
| `ethtool -k et` 的 gso/gro | 通常 `on` | `tcp-segmentation: off [requested on]` = 无 virtio 头，TSO 被拒（见 §4） |
| 最高线程 `%CPU` | 视负载 | 接近 100% 单核 → 单线程流水线瓶颈，加核无用 |

---

## 3. 采数据面线程：两个必踩的坑

**坑 1：进程名不是 `easytier`。** 实际二进制为 `ET-core`（数据面）与 `ET-web-embed`（控制面）。`pgrep -f easytier` 会漏掉或抓错：

```bash
pgrep -af 'ET-core'          # 先看清
P=$(pgrep -x ET-core); echo "$P"
```

`ET-web-embed` 与 `ET-core` 常挤在同一台机器上，单核机（如 1 vCPU VPS）上控制面的 `sqlx` 轮询会直接抢数据面 CPU —— 排查吞吐时先看这俩是否共存。

**坑 2：`top -p` 不能为空**，`P` 为空时直接报 `-p argument missing`；且 `top -n1` 是单次快照，**空闲时必然全 0**，必须在打流量的同时采样：

```bash
pidstat -t -p "$P" 1 5 | sort -k8 -nr | head -10   # 需 sysstat
top -H -b -d 1 -n 5 -p "$P" | awk '$9+0>0.5'       # 无 sysstat
```

---

## 4. `tx-tcp-segmentation: off [requested on]` 怎么读

这行不是故障，是**能力缺失**的运行时证据：TUN 未开 `IFF_VNET_HDR`，内核拒绝 `TUNSETOFFLOAD`，因此 TSO/GSO 在 TUN 上不可用 → 每包一次 syscall。

推论：**不要把读缓冲从 2500 字节调大了事**。PI off 且无 virtio 头时，一次 `read` 只返回一个包，缓冲再大也拿不到批量。根因分析见 [`../roadmap/tun-data-plane-throughput-2026-11.md`](../roadmap/tun-data-plane-throughput-2026-11.md) §3。

---

## 5. 测量陷阱：SMB Multichannel 会绕过隧道

Windows 客户端访问 `\\10.10.10.201` 时，SMB3 Multichannel 会探测服务器其它地址并从**物理网卡**另开通道。实测：默认配置下 642 MB 复制**0 字节**经过 `et`，全部走物理局域网。

测隧道吞吐前必须关掉：

```powershell
Set-SmbClientConfiguration -EnableMultiChannel $false -Confirm:$false
Restart-Service LanmanWorkstation -Force     # 断开已建立的多通道会话
```

测完记得恢复 `$true`。判定是否走隧道：比对 `et` 适配器增量与文件大小，若 `et` 增量接近 0 则测的是物理链路。

**相关坑**：`robocopy` 会**预分配**目标文件，`文件长度` 无法作为进度指标；必须用 `et` 适配器字节计数或逐秒采样，否则会误判为「1 秒传完」。

---

## 6. 给转发路径加压（不需要额外硬件）

用 netns 造一个「LAN 客户端」，让流量真的穿过本机 forward 路径：

```bash
sudo ip netns add t1
sudo ip link add veth0 type veth peer name veth1
sudo ip link set veth1 netns t1
sudo ip addr add 172.31.255.1/30 dev veth0 && sudo ip link set veth0 up
sudo ip netns exec t1 ip addr add 172.31.255.2/30 dev veth1
sudo ip netns exec t1 ip link set veth1 up
sudo ip netns exec t1 ip route add 10.10.10.0/24 via 172.31.255.1

awk '/^Ip:/{print "before Forw="$7}' /proc/net/snmp
sudo ip netns exec t1 ping -f -c 20000 -s 1200 10.10.10.101 >/dev/null 2>&1
awk '/^Ip:/{print "after  Forw="$7}' /proc/net/snmp

sudo ip netns del t1; sudo ip link del veth0
```

`ForwDatagrams` 增量 ≈ 20000 → 内核转发满速无问题，可把该项从怀疑列表划掉。

更硬的证据是规则计数（比 `nstat` 更直观）：

```bash
sudo iptables -I FORWARD -i et   -o eth0 -j ACCEPT
sudo iptables -I FORWARD -i eth0 -o et   -j ACCEPT
watch -n1 'iptables -L FORWARD -v -n | head -5'
# 测完删除
sudo iptables -D FORWARD -i et -o eth0 -j ACCEPT
sudo iptables -D FORWARD -i eth0 -o et -j ACCEPT
```

---

## 7. 改名后必查两项

设备名从 `tun` 改为 `et` 之后：

1. **防火墙规则按名字绑定的全部失效**。`nft`/`ufw` 里写 `iifname "tun"` 的规则不再匹配，表现是「隧道内互通正常、跨网段全断」；`ufw` 默认 `FORWARD DROP` 时尤其明显。用 §1 脚本第 5 段核对。
2. **持久化缺项**：policy routing（`ip rule`）、`tc` 脚本、`netplan` 里指向旧名的配置会失配，重启后行为与手工调试时不同。

另外确认两端 MTU 一致：`ip -d link show et | head -1`。历史上出现过「控制台显示 1380、实际节点仍为 1360/1280」的不同步。

---

## 8. 参考基线（2026-11-10 实测，Ubuntu 6.8.0-90，1 vCPU）

| 项 | 值 |
|---|---|
| `ip_forward` / IPv6 forwarding | `1` / `1` |
| `rp_filter`（all/eth0/et） | 全 `2` |
| `InReceives` / `ForwDatagrams` / `InDelivers`（5 s） | `+671` / `+172` / `+499`（自洽） |
| `OutNoRoutes` | `+0` |
| `et` 设备 | 30644 RX / 33247 TX 包，errors/dropped/missed 全 0 |
| `fq` 队列 | dropped 0，overlimits 0，backlog 0 |
| `softnet_stat` | dropped 0，time_squeeze 3（累计值） |
| iptables / nft / ufw | 均无规则 |
| offload | `generic-segmentation: on`、`tx-tcp-segmentation: off [requested on]`、`tx-checksumming: off` |

该节点为 1 vCPU，`nproc=1` 且 `/proc/cpuinfo` 仅 1 个 processor —— **不能作为吞吐对照样本**，只用于验证内核转发与设备健康。
