## Status

- Status: **Archive**
- Index: [../README.md](../README.md)
# TCP 代理半关闭与 KCP 短连接修复验证（2026-09-14�?
本次修复上一轮验证发现的半关闭失败、无丢包 KCP 突发连接交接丢失�?以及跨平台验证暴露的握手前心跳竞态。这些是转发与连接生命周期的局�?逻辑问题；没有改�?KCP 报文格式，也没有增加重试或放宽业务超时�?丢包网络下另有握手最�?ACK 丢失的问题，本次未修复，见下文�?
## 版本与根�?
- EasyTier 基线：`24572b49`；生产修复：`03fa375e`、`fd6e2a3e`�?- kcp-sys 基线：`d7427c22d764deb1860a7d37acc446ed5033464c`�?- 最终依赖：`b37ee660fb70bb6d816fb8bbc08b140e55e7218b`�?  `Cargo.toml` �?`Cargo.lock` 均锁定此 Git revision�?  已发布到 kcp-sys �?`fix/accept-half-close-20260914` 专用分支�?  便于其他机器获取锁定的依赖；未合并主分支�?- 旧版真实流量对照：`eb836559`，已包含 flow-key 修复，尚无本次修复�?- 最终流量二进制：`delivery-easytier-core`，SHA-256�?  `4635810ac9a536258f9b7ff606e5e1a5eb0243cb0f187825ba6f721d92dfc4fe`�?  构建内容与最终生产代码一致；构建发生在依�?pin 提交前，内嵌版本�?  不用于区分本次产物，�?SHA-256 和依�?revision 为准�?
修复内容�?
1. 原转发函数在任一方向 EOF 后退出，取消另一方向，服务端在请�?EOF
   后返回的响应因此丢失。改�?Tokio 双向拷贝传播写端关闭，继续转�?   反向数据，直到双向完成或发生错误�?2. KCP 容量�?4 �?accept 通知队列满时，已建立连接失去交接机会�?   在连接状态中保留�?accept 标记，队列为空时领取待交接连接；保留
   原有有界通知队列和串行领取，防止重复交接�?3. KCP 将正常关闭当作读错误，并可能�?FIN 后丢弃已经确认接收、尚�?   交给应用的数据。现在先排空接收数据再返�?EOF，RST 和端点销毁仍
   返回错误；双向关闭清理等待接收任务完成。EOF 判断和过期清理的
   状态读取顺序也一并修正，避免并发数据到达或心跳更新被旧判断覆盖�?4. FIN 可以先于 accept �?connect 返回到达。创建流时继承该关闭状态，
   使纯空请求、纯空响应也能得�?EOF。半关闭连接继续响应心跳�?5. Windows 原生测试捕获�?PING 先于 SYN 到达，对端因连接未知返回
   RST；旧依赖也复现相同时序。周期心跳现在只覆盖已建立和半关闭状态�?
## 回归覆盖与自动化结果

| 验证 | 结果 |
| --- | --- |
| Linux、macOS arm64、Windows x64 原生网关测试 | �?200/200 |
| 最�?kcp-sys 三平台原生测�?| �?10/10，两�?example target 通过 |
| 最终完整三节点矩阵及补充集成测�?| 276/276�?48.628 �?|
| EasyTier 格式检查、Linux 严格 Clippy | 通过 |
| kcp-sys Linux/macOS 格式检查、严�?Clippy | 通过 |

网关新增两个测试，分别从两端先关闭写方向；使用空请求�?2 KiB 请求
�?64 KiB 响应，接收方等到 EOF 才返回响应。旧转发函数两项均在 5 �?超时，修复后通过。原生网关测试不依赖应用�?KCP，心跳修复没有改�?这些测试的生产代码�?
新增三节点半关闭测试覆盖 TCP/KCP/QUIC × 内核/smoltcp 六种模式�?各检查两个关闭方向、空请求�?64 KiB 请求�?28 KiB 响应。完整集�?集合�?276 项：原有三节点矩�?256 项、新�?6 项、ACL/配置更新/
端口转发/断连 14 项，均在已有 `rust` 容器中串行运行�?
依赖回归覆盖�?2 次握手全部完成后�?accept�?00 KiB 缓冲数据后的
EOF/响应；双向关闭后跨越清理周期再读数据；半关闭心跳；RST/端点销�?读错误；FIN 先于 accept、FIN 先于 connect 返回；各握手阶段的心�?筛选。通知饱和测试在旧实现第五�?accept 超时；提�?FIN 测试在修�?前等�?EOF 超时。排空测试使用多工作线程运行时，并做过额外重复验证�?
主要命令（Linux 容器内，Rust 1.95）：

```sh
cargo +1.95 test --locked -p easytier-core \
  --features proxy-smoltcp-stack --lib gateway::
cargo +1.95 nextest run --locked -p easytier --features full --lib \
  -E 'test(subnet_proxy_three_node_test) | test(subnet_proxy_half_close_test) | test(acl_rule_test_inbound) | test(acl_rule_test_subnet_proxy) | test(proxy_three_node_disconnect_test) | test(config_patch_test) | test(port_forward_with_inbound_default_drop_acl_test)' \
  --test-threads 1 --no-fail-fast
cargo fmt --all -- --check
cargo +1.95 clippy --locked -p easytier --features full \
  --lib --tests -- -D warnings
```

网关严格 Clippy 另以 `proxy-smoltcp-stack,ring-crypto` 通过；原因见上一�?记录中的既有未使用警告说明。Windows 所�?stable 工具链缺少格式检�?组件，未�?Windows 格式�?Clippy 计为通过�?
## 最终二进制真实流量

普�?TCP、KCP、QUIC 与内�?smoltcp 六种组合，底�?UDP；每组合 300 �?短连接�?6 并发�? �?socket 超时，检�?greeting 和逐字节回显�?
- 六模式主矩阵�?,800/1,800 次短连接通过�?- 请求 EOF 后返回响应：�?大请求共 12/12，通过；大请求 256 KiB�?  响应 1 MiB + 4 字节�?- 服务端先关闭写端，客户端读到 EOF 后再�?256 KiB�?/6，通过�?  服务端实际校验接收数据�?- connect 后立即关闭写端，不发送模式字节、不等待 greeting：每模式
  32 次�?6 并发，共 192/192 次纯空请求通过�?- KCP 每栈额外三轮，与主矩阵合�?2,400/2,400 次短连接通过。追�?  测试的大小半关闭�?192 次纯空请求通过�? 次反向半关闭�?1 次超时，
  因此不能将所有追加半关闭计为通过，具体证据见下文�?- 最终版本在 `delay 10ms 3ms loss 1%` 下，六模式各两个连接持续
  15 秒逐字节回显�?2 次大小半关闭�?6 次反向半关闭通过。KCP 短连�?  仍有内核模式 5/300、smoltcp 模式 8/300 �?greeting 超时，其�?  四种模式零失败；没有为每次超时单独抓包证明原因相同�?- 新旧两方�?× 六模式，�?12 组合正常持续回显通过。旧源端→新目标�?  1,800 次短连接全通过；新源端→旧目标�?KCP 内核模式 12/300�?  smoltcp 模式 9/300 次超时，其他四种模式零失败�?
旧版同样的无丢包 KCP 2,400 次短连接�?78 �?greeting 超时；旧版六�?模式在请�?EOF 后均没有响应，且出现�?QUIC 请求数据被截断�?
最终版�?KCP 主矩阵后，两种栈源端�?335 �?Closed 代理记录均在
15 秒观察点清零；目标端记录为零。两�?FD 数保持内核模�?15、smoltcp
模式 14。RSS 较起始保留约 3.2�?.4 MiB 增长，单次有限负载不能证�?或排除长期内存泄漏，也不能将代理条目清零等同于所有内部对象回收�?
## 仍然存在的丢包握手问题与升级限制

在源端施�?`netem delay 10ms 3ms loss 1%` 后，中间修复版本
（SHA-256 `2e78ecf…`）KCP 仍有 3/600 �?greeting 超时。额外有�?TRACE 重跑捕获 3 个连接：源端收到 SYN|ACK，发出空 ACK|DATA 后即
返回已建立；目标端未收到最�?ACK，直�?5 秒后�?FIN 才返�?RST�?这与 accept 通知饱和不同，目标端尚未完成握手�?
该控制报文可靠性问题需要单独设计握手恢复、重传及过期清理的一致性，
不能仅延长超时或重试应用请求。本次没有声称消除了所�?KCP 超时�?具体连接号和双方日志行见 `remaining-loss-handshake.md`�?`loss-handshake-evidence.log`�?
混合版本正常回显通过并不意味着旧端获得修复。旧 KCP 目标端仍保留
accept 缺陷；KCP 的完整半关闭能力需要两端升级。没有修改报文格式�?
追加�?netem �?KCP/kernel 反向半关闭中，连�?`3969305677` 在等
服务端数据和 EOF 时超时；服务端没有收到客户端原定�?EOF 后发送的
256 KiB 数据。发送端同一连接出现 780 次数据输出队�?Full，随后在
08:36:47.247 UTC 发送缓冲排空，FIN 成功进入输出队列；接收端直到
5 秒业务超时都未报告对端关闭，08:37:01.030 才报告关闭或重置信号�?�?debug 日志行也可能�?RST 触发，不能据此认定迟到的�?FIN，更不能
将数据队�?Full 直接解释�?FIN 被该队列丢弃。现�?FIN 发送路径没�?确认重传；这一单次失败的具体丢包点尚未证实，保留为未解决的验证异常�?同场景有�?TRACE 重跑 3 轮，900 次短连接�? 次反向半关闭均通过�?抓包确认成功轮次收到 FIN 并完成反向数据，未复现原失败；不能以重跑
通过覆盖原失败。旧依赖与最终依赖的发送排空后入队 FIN 路径相同�?对照保存�?`fin-send-path-old-new.txt`�?
异常轮次�?15 秒资源观察点仍有 1 �?Closed 代理记录，无 Connecting/
Connected，FD 已回到起始值；因此仅主矩阵可以报告该观察点全部清零�?定向 TRACE 三轮�?1,005 �?Closed 记录�?15 秒观察点全部清零�?
## 验证中的异常与证�?
首轮未完成的集成运行中，端口转发 ACL case 3 曾在连接本地监听端口�?出现一�?ConnectionRefused；随后完整一�?276 项通过，同项也通过�?旧版单次及额外十次均通过，因此没有证据把这次失败断言为旧版必现问题，
也未修改无关的端口转发代码。新增半关闭测试的早期夹具曾只等待路�?公告而未等待实际 TUN 路由可用，已补足就绪检查，并用有界 try_join
及时报告连接错误；这不计入生产代码的 red/green 对照�?
Windows 测试早期表现为挂起，原因是测�?join �?connect 失败后仍等待
accept。限定握手等待后暴露出上�?PING/RST/SYN 竞态，旧依赖和修复�?分支均有 TRACE 证据；最终原生测试通过。完�?diff 和后续心跳增量均
经过独立子代理审查，没有 high-confidence 的新缺陷发现�?
原始脚本、日志、逐连�?JSON、二进制和哈希保存在本机�?
```text
/data/project/proxy-close-validation-20260914/
```

主要文件：`delivery-manifest.json`、`delivery-integration.log`、`delivery-clippy.log`�?`final-core-gateway.log`、`macos-core.log`、`windows-core.log`�?`mac-kcp-heartbeat-tests.log`、`final-tests.log`（Windows KCP）�?`delivery-summary.json`、`delivery-kcp-repeat-summary.json`�?`delivery-*-resources.json`、`traffic-summary.md`、`traffic-binaries.json`�?`delivery-commands.json`、`artifacts-manifest.json`、`delivery-reverse-timeout.md`�?`relay-red.log`、`windows-kcp-trace-fail.txt`�?`windows-kcp-baseline-startup.txt`。Windows 部分原始日志�?UTF-16LE�?Linux kcp-sys 测试、格式、Clippy 及依赖测�?red/green 对照为子代理
执行结果，未单独落盘原始命令日志；最�?10 项测试耗时 11.03 秒�?
未覆�?Windows/macOS 原生 TUN 端到端路径、生产规模长时间运行、吞�?基准及容量极限负载。流量实验仅使用并清理自己的 namespace 和进程�?