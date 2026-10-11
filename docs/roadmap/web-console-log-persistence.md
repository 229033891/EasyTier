# Web 控制台：日志本地留存与按条件查看

## Status

- Status: **In progress**（A/B/C 代码已落地；待本地/CI 编译与联调验收）
- 最近审阅：2026-10-11
- 归属：[`web-evolution.md`](./web-evolution.md) 阶段 B「可运维性」；承接 [`web-console-runtime-diagnostics.md`](./web-console-runtime-diagnostics.md) 的「运行日志」菜单
- 范围：`easytier-web`（REST + 前端）+ `easytier/src/common/log` 少量只读访问器；**不改** `easytier-core` 节点协议
- 索引：[`../README.md`](../README.md)

---

## 1. 背景与必要性

控制台（`easytier-web`）是常驻服务，当前**看不到 warn / error 的历史**：

| 现象 | 代码锚点（易漂移） |
|------|----------|
| 日志只在进程内 1000 条环形缓冲，重启即失 | `easytier-web/src/main.rs` `enable_memory_buffer(1000)`；`easytier/src/common/log/mod.rs` `MemoryLogBuffer::push` / `snapshot` |
| 缓冲阈值 `Info`，info 洪流会把 warn/error 挤出环 | 同上 `accepts()`；`enable_memory_buffer` → `LevelFilter::Info` |
| 文件日志**默认关闭**：`--file-log-dir` / `--file-log-level` 均未配置时级别解析为 `Off`，直接返回 `disabled()` | `easytier/src/common/log/file.rs` `FileSink::from_config` |
| 容器里 stdout 归宿主机 log driver 管，重启即丢、保留期不可控 | `docs/ops/docker-compose-deploy.md` |
| GUI 已有本地日志读/清/打开目录能力，唯独 Web 缺 | `easytier-gui/src-tauri/src/lib.rs`（`is_easytier_log_file` / `read_file_tail` / `list_log_files` / `read_log_file`） |

**结论：有必要。** 出故障时要能回看事发前的 warn/error，唯一可靠载体是**控制台本地的按天滚动文件**；内存环继续承担「刚刚发生了什么」的实时视图。

---

## 2. 已定设计决策（三项）

| # | 决策 | 理由 |
|---|------|------|
| D1 | 文件日志**默认开启**：目录 `./logs`、级别 `warn`、启动前探测可写性，不可写则**降级为仅内存环**并告警，不阻断启动 | 只读根文件系统 / 无权限容器不能因为写日志而启动失败（`main.rs` 现在对日志 init 是 `.unwrap()`） |
| D2 | **内存环保留**，与文件视图并存 | 文件可能未开启（降级 / `--file-log-level off`），此时页面仍有内容；实时轮询读盘要 seek+read+flush，多标签页放大 IO |
| D3 | **改造现有** `RuntimeLogs.vue`，不新增页面 | 「实时日志」与「历史日志」是同一件事的两种来源；入口 `MainPage.vue` `runtimeLogs` 已存在，组件仅约 220 行 |

---

## 3. 磁盘占用口径（务必按此对外说明）

滚动条件来自仓库内 vendored 实现 `easytier/src/common/tracing_rolling_appender/`：

- `condition_daily()` + `condition_max_file_size()` 是**两个并列条件，任一满足即滚**（`base.rs` `RollingFileAppenderBaseBuilder`：`condition.frequency_opt` 与 `condition.max_size_opt` 同时被设置）。
- 单文件上限：`size_mb.unwrap_or(100) * 1024 * 1024`（`easytier/src/common/log/file.rs`）。
- 归档数：`count.unwrap_or(3)`；命名规则 `easytier.log`（当前）+ `easytier.log.1/2/3`，轮转时先删 `filename_for(max_filecount)` 再逐个后移（`tracing_rolling_appender/mod.rs` `filename_for` / `rotate_files`）。

⇒ **共 4 个文件（1 当前 + 3 归档），单文件 ≤ 100MB，目录最坏约 400MB。** warn 级实际写量远小于此。

`easytier-web` CLI 目前**只暴露** `--file-log-dir` / `--file-log-level`（`ET_WEB_FILE_LOG_DIR` / `ET_WEB_FILE_LOG_LEVEL`），`size_mb` / `count` 走默认值；彻底关闭用 `--file-log-level off`（`from_config` 已有该语义）。想收紧体积需加 flag，见 §8 可选增强。

---

## 4. 方案 A：文件日志默认开启 + 可写性预检

### 4.1 `easytier-web/src/main.rs`

在调 `log::init_with_default_console_targets` **之前**探测目录；探测结果反馈给已有的 `impl LoggingConfigLoader for &Cli`：

```rust
// 新增：解析文件日志配置（在 init 之前调用，结果供 get_file_logger_config 使用）
fn resolve_file_logger_config(cli: &Cli) -> FileLoggerConfig {
    let dir = cli
        .file_log_dir
        .clone()
        .unwrap_or_else(|| "logs".to_string());
    let level = cli
        .file_log_level
        .clone()
        .unwrap_or_else(|| "warn".to_string());
    if level.eq_ignore_ascii_case("off") {
        return FileLoggerConfig { dir: None, level: Some(level), ..Default::default() };
    }
    let writable = std::fs::create_dir_all(&dir)
        .and_then(|_| probe_writable(std::path::Path::new(&dir)))
        .is_ok();
    if !writable {
        // logger 尚未安装，先用 stderr；装好后补一条 warn。
        // 必须 level=off：`from_config` 在 dir=None 且 level=warn 时会落到 cwd(".")。
        eprintln!(
            "easytier-web: file log dir '{}' not writable, falling back to in-memory ring only",
            dir
        );
        return FileLoggerConfig {
            dir: None,
            level: Some("off".to_string()),
            ..Default::default()
        };
    }
    FileLoggerConfig { dir: Some(dir), level: Some(level), ..Default::default() }
}

/// 目录内 create + write + remove 一个探测文件（rolling appender 会 lazy create）
fn probe_writable(dir: &std::path::Path) -> std::io::Result<()> {
    let probe = dir.join(".easytier-log-probe");
    let mut f = std::fs::OpenOptions::new()
        .write(true).create(true).truncate(true).open(&probe)?;
    std::io::Write::write_all(&mut f, b"probe")?;
    drop(f);
    std::fs::remove_file(probe)
}
```

`impl LoggingConfigLoader for &Cli` 的 `get_file_logger_config()` 改为返回上面的结果（不再直接透传 `cli.file_log_dir`）。`--file-log-level off` 时 `from_config` 返回 disabled，即彻底关闭。

### 4.2 `easytier/src/common/log`：单一事实来源

前端不能猜「以为开了其实没开」，需要一个访问器：

```rust
/// `(dir, level)` of the installed file sink; `None` when file logging is off.
pub fn file_log_config() -> Option<(std::path::PathBuf, LevelFilter)>
```

- `FileSink` 在 `from_config` 里已持有 `dir` 与 `path`，补一个 `dir: Option<PathBuf>` 字段即可，
  经 `LOGGER.get()` 暴露（与 `snapshot_memory_logs` 同风格）。
- 该访问器只读、无锁竞争，供 `/admin/logs/files` 返回 `enabled` / `dir` / `level`。

### 4.3 单测

| 用例 | 期望 |
|------|------|
| 临时目录可写 | `get_file_logger_config()` 返回 `dir=Some`、`level=Some("warn")`；写入一条 warn 后文件出现该行 |
| 目录不可写（用「普通文件当目录」构造，root 下同样失败） | 返回 `dir=None` + `level=off`；进程**不** panic，stderr 有降级告警 |
| `--file-log-level off` | `dir=None`，`file_log_config()` 为 `None` |
| 显式 `--file-log-dir` | 覆盖默认 `logs` |

---

## 5. 方案 B：后端读文件 API

挂到现有 admin 路由（`easytier-web/src/restful/admin_diagnostics.rs` `router()`），复用现成 `require_admin`：

```rust
.route("/api/v1/admin/logs/files", get(handle_log_files))
.route("/api/v1/admin/logs/file",  get(handle_log_file_query))
```

### 5.1 `GET /api/v1/admin/logs/files`

```json
{
  "enabled": true,
  "dir": "logs",
  "level": "WARN",
  "files": [
    { "file_name": "easytier.log", "size_bytes": 2048, "modified_ms": 1760000000000, "active": true },
    { "file_name": "easytier.log.1", "size_bytes": 104857600, "modified_ms": 1759990000000, "active": false }
  ]
}
```

- 只列白名单文件：`Path::new(&name).file_name()` 归一化后命中 `name == "easytier.log" || name.starts_with("easytier.log.")`（与 GUI 同款，**杜绝目录穿越**），上限 32 个，按 active→modified 排序。
- `enabled=false` 时 `dir` 为空串、`files` 空数组，前端走引导态。

### 5.2 `GET /api/v1/admin/logs/file`

Query：

| 参数 | 类型 | 说明 |
|------|------|------|
| `file` | string | 必填；白名单校验 |
| `tail` | usize | 默认 500，**clamp 1..=2000** |
| `min_level` | string | `error`/`warn`/`info`/`debug`/`trace`；缺省不过滤 |
| `since` / `until` | string | RFC3339 或本地 ISO；缺省不限制 |
| `grep` | string | 大小写不敏感子串，匹配 `target` + `message` |

Response：

```json
{
  "file": "easytier.log",
  "dir": "logs",
  "level": "WARN",
  "total_scanned": 812,
  "matched": 37,
  "truncated_bytes": false,
  "lines": [ { "ts": "2026-10-11T02:03:04.567Z", "level": "WARN", "target": "easytier_web", "message": "…" } ]
}
```

实现要点（每条都有对应代码位置）：

1. **同步 IO 必须 `tokio::task::spawn_blocking`**：seek + read + 解析不能占 tokio worker（`RestfulServer` 只有 4 worker，一卡就是全局）。
2. `log::flush()` 先刷盘（GUI `read_log_file` 同款 best-effort）。
3. 读尾部 **1 MiB**（`MAX_LOG_READ_BYTES` 可对齐 GUI 的 8 MiB，Web 侧取 1 MiB 足够）；seek 造成的首残行丢弃（GUI `read_file_tail` 同款：`split_once('\n')` 取后半）。
4. 行格式即 `format_line` 的 colorless 分支（`easytier/src/common/log/mod.rs`）：
   `{ts} {level:<5} {target}: {message}`。解析时在 level 之后用 **`: `（冒号+空格）** 切
   target/message，避免 Rust target 中的 `::` 被拆错；时间戳为 RFC3339 UTC 毫秒带 `Z`。
5. **非标准前缀行**（多行 panic 堆栈、`eprintln!` 直写）解析为 `level="UNKNOWN"` 并**保留且不参与级别过滤**，避免丢上下文。
6. **文件行必须过 `redact_log_message()`**：脱敏目前只加在内存环出口（`admin_diagnostics.rs` `handle_logs`），文件里是明文；不补就等于把历史 `user_token` 明文吐给有 admin 权限的调用方。这是**安全必做项**。
7. **单行截断**：`MEMORY_LOG_MESSAGE_MAX` 只作用于内存环，文件行无上限 → 按字符截断（如 4 KiB + `…`），否则一行大 JSON 能撑爆响应。
8. 时间过滤用解析出的 `ts`，解析失败的行走 `UNKNOWN` 分支（不过滤）。
9. **与本文草案的偏差（有意）**：文件不存在时返回 **200 + 空列表**而非 404 —— 滚动 appender 是首次写入才创建文件，404 会让 UI 把"刚开还没日志"显示成错误。
10. **`RUST_LOG` 会覆盖文件级别**：`file_filter` 走 `TargetFilter::from_environment`，运维设了 `RUST_LOG=debug` 时文件同样按 debug 记录（磁盘上限仍是 4×100MB）。`/admin/logs/files` 的 `level` 字段反映的是**实际生效**的 sink 级别。
11. admin 读文件会写一条审计日志（actor / file / 过滤条件），日志可能含 token，便于追溯。

### 5.3 单测

| 用例 | 期望 |
|------|------|
| `file=../et.db` / `file=/etc/passwd` | 400（白名单拒绝） |
| warn/error 各若干行 + info 若干行，`min_level=warn` | 只回 warn/error，`matched` 正确 |
| 多行堆栈（无标准前缀） | 以 `UNKNOWN` 保留，`min_level=warn` 不吞掉 |
| 含 `udp://host:22020/<token>` 与 `"user_token":"x"` 的行 | 响应中均为 `***` |
| 超长单行（>4 KiB） | 截断并带 `…`，响应体不炸 |
| `tail=99999` | clamp 到 2000 |
| 文件不存在（日志已启用） | **200 + 空 `lines`**（非 404；见 §5.2.9） |
| 路径为 symlink / hardlink | 400 `invalid_log_file` |

---

## 6. 方案 C：前端改造（复用 `RuntimeLogs.vue`）

### 6.1 API 客户端（`frontend/src/modules/api.ts`）

```ts
list_log_files(): Promise<LogFilesResponse>
read_log_file(params: {
  file: string; tail?: number; min_level?: string;
  since?: string; until?: string; grep?: string;
}): Promise<LogFileLinesResponse>
```

### 6.2 控件与行为

| 区域 | 内容 |
|------|------|
| 来源切换 | 「实时内存 ｜ 文件」；文件模式追加文件下拉（`easytier.log` 标 active） |
| 筛选条 | 级别下拉、起止时间（`datetime-local` → 转 UTC ISO）、关键字、tail 滑杆（100–2000） |
| 自动刷新 | **仅实时模式生效**（带时间范围刷文件没有意义）；轮询间隔 **3s** |
| 降级/引导 | `enabled=false` 时默认停在实时模式，显示引导条（说明 `--file-log-dir` / `--file-log-level`），**不留空白页** |
| 空态 | 筛选无命中时显示「无匹配」而非空白 |

### 6.3 i18n

`easytier-web/frontend-lib/src/locales/cn.yaml` 与 `en.yaml` **对称新增**：
`web.runtime_logs.source_realtime` / `source_file` / `file` / `level` / `time_from` / `time_to` / `keyword` / `tail` / `files_not_enabled` / `files_not_enabled_hint` / `no_match` / `matched_summary` / `truncated_bytes`。

### 6.4 文档与发布说明

- `docs/ops/deploy-install.md`、`docs/ops/docker-compose-deploy.md`：默认写 `./logs`（容器需挂卷）；只读根文件系统自动降级为仅内存环；关闭方式 `--file-log-level off`。
- CLI help（`cli.file_log_dir` / `cli.file_log_level` 的 i18n 文案）注明默认目录 `logs`、默认级别 `warn`、关闭用 `off`。
  （日志留存归属控制台运维，不写入 `client-web-console-interaction.md`。）

---

## 7. 风险与缓解

| 风险 | 缓解 |
|------|------|
| 默认写盘改变部署习惯、磁盘增长（最坏 ~400MB） | warn + 按天/100MB 双条件滚动 + 3 归档；`--file-log-level off` 可关；文档口径统一 |
| 只读容器启动失败 | 启动前 `probe_writable` 预检，失败降级内存环 + stderr 告警（**不** `unwrap`） |
| 路径穿越读任意文件 | 文件名白名单 + `Path::file_name()` 归一化 |
| 阻塞 tokio worker | 文件读与解析走 `spawn_blocking` |
| 历史 `user_token` 明文泄露 | 文件出口强制 `redact_log_message` |
| 巨型单行 / 超多行响应 | 单行 4 KiB 截断、`tail` clamp 2000、读取字节上限 |
| 多行日志被过滤吞掉 | 非标准前缀行按 `UNKNOWN` 保留且不参与级别过滤 |
| 时区困惑 | 磁盘为 UTC（`Z`）；前端时间输入转 UTC 发送（`until` 扩到分钟末），展示沿用现有 `localizeLogTimestamps` |
| 多进程共享日志目录误判不可写 | 探测文件名带 pid+纳秒；删探测文件遇 NotFound 不算失败 |
| sink 首次打开失败仍然崩进程 | init 失败不再 `unwrap`，降级 console + 内存环 |
| 日志目录内软链读到目录外 | `symlink_metadata` + Unix `O_NOFOLLOW` / Windows file_index 校验；拒绝 `nlink>1` hardlink |
| 多 tab 打满 blocking 池 | 文件读并发信号量（最多 2） |
| 历史 `user_token` 明文泄露 | 文件出口强制 `redact_log_message` + admin 读取审计 |

---

## 8. 待决与可选增强

| 项 | 说明 |
|----|------|
| 滚动参数 flag | `--file-log-size-mb` / `--file-log-count`（当前只有默认 100MB / 3）。需要就加，默认不改 |
| SSE / tail -f | 替代实时模式轮询；内存环足够便宜，暂不做 |
| 设备侧日志聚合 | 把节点日志也汇到控制台（涉及 `LoggerRpcService` 与体量，**明确不做**） |
| 存储层留存 | 落 SQLite/对象存储；与「本地文件 + 宿主机采集」重复，**不做** |

---

## 9. 分期与验收

### 阶段 1：默认开启 + 降级（A）

- [x] `resolve_file_logger_config` / `probe_writable`（探测文件名带 pid+纳秒，容忍并发清理）+ `WebLoggingConfig`
- [x] `log::file_log_config()` 访问器
- [x] 单测：可写开、`off` 关闭、显式 `dir` 覆盖、不可写路径降级（「文件当目录」，root 下同样失败）、并发残留探测文件不误判
- [x] 复核修复：撤销 `from_config` 里 `dir=None → disabled` 的共享 guard（那会让节点 CLI 的 `--file-log-level` 无 dir 时静默不写文件；web 侧用 `level=off` 表达关闭），并补 cwd 回退的单测（`CwdGuard` 切到临时目录）

验收：只读目录启动成功且 stderr 有告警；`/admin/logs/files` 的 `enabled` 与真实状态一致。
额外验收：`FileSink` 首开失败（磁盘满等）不再 `unwrap` 崩进程，而是降级为 console + 内存环并告警。

### 阶段 2：读文件 API（B）

- [x] 两个 endpoint + 白名单 / 脱敏 / 过滤 / 截断 / `spawn_blocking`
- [x] 单测：穿越拒绝、`::` target 解析、UNKNOWN 保留、脱敏、min_level
- [x] 复核加固：`symlink_metadata` + `O_NOFOLLOW`/file_index；拒绝 hardlink；admin 读审计；读并发信号量（2）

验收：`curl` 带 `min_level=warn&since=…` 只回 warn+；`file=../et.db` 被拒。

### 阶段 3：前端（C）

- [x] `api.ts` 两个方法 + `RuntimeLogs.vue` 改造 + i18n 双语
- [x] `vue-tsc --noEmit` 已通过（完整 frontend build / Rust CI 待跑）
- [x] 复核修复：`until` 由 `datetime-local`（分钟精度）扩到该分钟 `:59.999`，避免「到 02:03」漏掉 02:03:59

验收：页面能在「实时 / 文件」间切换并正确筛选；文件关闭时显示引导态。

### 阶段 4：文档（D）

- [x] `docs/ops/web-upgrade.md`、`docker-compose-deploy.md`、`deploy-install.md`、CLI help、`docker-compose.yml` 挂 `logs`
- [ ] 联调通过后把本文 Status 改为 Superseded，缺口移入 Current

---

## 10. 修订记录

| 日期 | 内容 |
|------|------|
| 2026-10-11 | 立项：三项决策（默认开启 `./logs`+warn+可写性预检 / 内存环保留 / 复用现有页）、磁盘口径 4 文件 ≈400MB、后端两 endpoint 草案、前端筛选形态、风险表 |
| 2026-10-11 | 落地修正：降级必须 `level=off`；行解析用 `: ` 切开 target；实现 A/B/C + compose 持久化 `/app/data/logs` |
| 2026-10-11 | 复核修复：① 撤销 `FileSink::from_config` 的 `dir=None → disabled` 共享 guard（会让节点 CLI `--file-log-level` 无 dir 时静默不写文件，改由 web 传 `level=off` 表达关闭）；② `probe_writable` 探测文件名带 pid+纳秒并容忍并发清理；③ 日志 init 失败不再 `unwrap`，降级 console+内存环；④ 不可写测试改用「文件当目录」（root 下也失败）；⑤ `symlink_metadata` 拒绝日志目录内软链；⑥ admin 读文件加审计日志；⑦ `until` 扩到分钟末 |

---

## 相关文档

- [`web-console-runtime-diagnostics.md`](./web-console-runtime-diagnostics.md)：系统诊断 + 运行日志（ring buffer 一期已落地）
- [`client-web-console-interaction.md`](./client-web-console-interaction.md)：客户端 ↔ 控制台交互（超时分级 / Start 覆盖 / 会话接管）
- [`web-evolution.md`](./web-evolution.md)：`easytier-web` 演进约束与阶段
