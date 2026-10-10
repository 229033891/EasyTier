use std::{
    collections::VecDeque,
    fmt::{self, Write as _},
    io::{self, IsTerminal, Write as _},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU8, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use log::{Level, LevelFilter, Metadata as LogMetadata, Record as LogRecord};
use paste::paste;
use tracing::{
    Event,
    field::{Field, Visit},
};

mod file;
#[cfg(feature = "management")]
mod management;
#[cfg(feature = "management")]
pub use management::{init, init_with_default_console_targets};
mod tracing_backend;

use file::FileSink;

macro_rules! __log__ {
    (const $var:ident = $target:expr) => {
        const $var: &'static str = $target;
        __log__!(@impl $target, $);
    };

    (@impl $target:expr, $_:tt) => {
        __log__!(@impl $_, $target, error, warn, info, debug, trace);
    };

    (@impl $_:tt, $target:expr, $($lvl:ident),+) => {
        paste! {
            $(
                macro_rules! [< __ $lvl __ >] {
                    (category: $cat:expr, $_ ($arg:tt)+) => {
                        tracing::$lvl!(target: concat!($target, "::", $cat), $_ ($arg)+)
                    };
                    ($_ ($arg:tt)+) => {
                        tracing::$lvl!(target: $target, $_ ($arg)+)
                    };
                }

                #[allow(unused_imports)]
                pub(crate) use [< __ $lvl __ >] as $lvl;
            )+
        }
    };
}

__log__!(const LOG_TARGET = "CORE");

static LOGGER: OnceLock<Logger> = OnceLock::new();
static MEMORY_BUFFER: OnceLock<MemoryLogBuffer> = OnceLock::new();

const MEMORY_LOG_MESSAGE_MAX: usize = 2048;

/// One line captured for in-process diagnostics (web console, etc.).
#[derive(Debug, Clone)]
pub struct MemoryLogLine {
    pub ts: String,
    pub level: String,
    pub target: String,
    pub message: String,
}

struct MemoryLogBuffer {
    capacity: usize,
    /// 内存缓冲**自己的**级别阈值，独立于 console/file。
    /// 否则 `--console-log-level off/warn` 会把 Web 控制台的日志页一起清空。
    level: LevelFilter,
    lines: Mutex<VecDeque<MemoryLogLine>>,
}

impl MemoryLogBuffer {
    fn new(capacity: usize, level: LevelFilter) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            level,
            lines: Mutex::new(VecDeque::with_capacity(capacity.min(1024))),
        }
    }

    /// 该级别是否应进入内存缓冲。
    fn accepts(&self, level: Level) -> bool {
        level_is_enabled(level_rank(self.level), level)
    }

    fn push(&self, ts: String, level: Level, target: &str, message: &str) {
        // 截断必须按**字符**计数：`message.len()` 是字节数，而中文 1 字 = 3 字节，
        // 用字节判断会让「截断后反而更长」。短消息（字节 <= 上限 ⇒ 字符必 <= 上限）走快路径。
        let message = if message.len() <= MEMORY_LOG_MESSAGE_MAX {
            message.to_string()
        } else {
            let mut chars = message.chars();
            let truncated: String = chars.by_ref().take(MEMORY_LOG_MESSAGE_MAX).collect();
            if chars.next().is_some() {
                format!("{truncated}…")
            } else {
                truncated
            }
        };
        let line = MemoryLogLine {
            ts,
            level: level.to_string().to_ascii_uppercase(),
            target: target.to_string(),
            message,
        };
        let Ok(mut guard) = self.lines.lock() else {
            return;
        };
        if guard.len() >= self.capacity {
            guard.pop_front();
        }
        guard.push_back(line);
    }

    fn snapshot(&self, tail: usize) -> (usize, Vec<MemoryLogLine>) {
        let Ok(guard) = self.lines.lock() else {
            return (0, Vec::new());
        };
        let capacity = self.capacity;
        if tail == 0 || guard.is_empty() {
            return (capacity, Vec::new());
        }
        let start = guard.len().saturating_sub(tail);
        (capacity, guard.iter().skip(start).cloned().collect())
    }
}

fn memory_buffer_level() -> LevelFilter {
    MEMORY_BUFFER
        .get()
        .map(|buf| buf.level)
        .unwrap_or(LevelFilter::Off)
}

/// Raise `log::max_level` / `active_max_level` so the `log` facade and tracing
/// `max_level_hint` do not filter out lines the memory buffer still wants.
fn refresh_dispatch_max_level() {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let max = logger.max_level();
    logger
        .active_max_level
        .store(level_rank(max), Ordering::Release);
    log::set_max_level(max);
}

/// Enable a process-local ring buffer of recent log lines (idempotent first-wins).
///
/// 默认以 `INFO` 作为缓冲**自己的**级别阈值 —— 刻意与 console/file 解耦，
/// 这样 `--console-log-level off`（或 `warn`）不会让 Web 控制台的日志页变成空白。
///
/// 启用后会抬升全局 `log::max_level` / tracing hint，否则 facade 在 console 为
/// `warn`/`off` 时根本不会把 INFO 送到 `enabled()`/`emit()`。
pub fn enable_memory_buffer(capacity: usize) -> anyhow::Result<()> {
    enable_memory_buffer_with_level(capacity, LevelFilter::Info)
}

/// 同 [`enable_memory_buffer`]，但显式指定缓冲的级别阈值。
pub fn enable_memory_buffer_with_level(
    capacity: usize,
    level: LevelFilter,
) -> anyhow::Result<()> {
    if MEMORY_BUFFER.get().is_some() {
        return Ok(()); // already enabled (first wins)
    }
    match MEMORY_BUFFER.set(MemoryLogBuffer::new(capacity, level)) {
        Ok(()) => {
            refresh_dispatch_max_level();
            Ok(())
        }
        Err(_) => Ok(()), // raced with another caller; first wins
    }
}

/// Return up to `tail` most recent lines from the memory buffer (empty if disabled).
pub fn snapshot_memory_logs(tail: usize) -> (usize, Vec<MemoryLogLine>) {
    MEMORY_BUFFER
        .get()
        .map(|buf| buf.snapshot(tail))
        .unwrap_or((0, Vec::new()))
}

pub fn init_console() -> anyhow::Result<()> {
    install(Logger::new(
        TargetFilter::console(Some(LevelFilter::Info))?,
        FileSink::disabled(),
    ))
}

pub fn set_file_level(level: &str) -> anyhow::Result<()> {
    let level = parse_level(level).context("invalid file log level")?;
    LOGGER
        .get()
        .context("logger is not initialized")?
        .set_file_level(level)
}

pub fn file_level() -> String {
    LOGGER
        .get()
        .map(Logger::file_level)
        .unwrap_or(LevelFilter::Info)
        .to_string()
        .to_ascii_lowercase()
}

pub fn flush() {
    if let Some(logger) = LOGGER.get() {
        logger.flush_file();
    }
}

fn install(logger: Logger) -> anyhow::Result<()> {
    LOGGER
        .set(logger)
        .map_err(|_| anyhow::anyhow!("logger is already initialized"))?;
    let logger = LOGGER.get().expect("logger was just initialized");

    log::set_logger(logger).map_err(|_| anyhow::anyhow!("a log logger is already installed"))?;
    // Include memory-buffer level when present (enable-before-init or same process).
    let max = logger.max_level();
    logger
        .active_max_level
        .store(level_rank(max), Ordering::Release);
    log::set_max_level(max);
    tracing_backend::install(logger).context("failed to install tracing subscriber")
}

fn parse_level(level: &str) -> anyhow::Result<LevelFilter> {
    level
        .parse()
        .map_err(|error| anyhow::anyhow!("{error}: {level:?}"))
}

#[derive(Clone, Debug)]
struct TargetFilter {
    default: LevelFilter,
    targets: Vec<(Box<str>, LevelFilter)>,
}

impl TargetFilter {
    fn console(level: Option<LevelFilter>) -> anyhow::Result<Self> {
        Self::console_with_default_targets(level, &[LOG_TARGET])
    }

    fn console_with_default_targets(
        level: Option<LevelFilter>,
        default_targets: &[&str],
    ) -> anyhow::Result<Self> {
        if level == Some(LevelFilter::Off) {
            return Ok(Self::off());
        }

        let fallback = match level {
            Some(level) => Self::with_default(level),
            None => Self {
                default: LevelFilter::Off,
                targets: default_targets
                    .iter()
                    .map(|target| ((*target).into(), LevelFilter::Info))
                    .collect(),
            },
        };
        Self::from_environment(fallback)
    }

    fn off() -> Self {
        Self::with_default(LevelFilter::Off)
    }

    fn with_default(default: LevelFilter) -> Self {
        Self {
            default,
            targets: Vec::new(),
        }
    }

    fn from_environment(fallback: Self) -> anyhow::Result<Self> {
        let spec = std::env::var("RUST_LOG").unwrap_or_default();
        Self::parse(&spec).map(|filter| filter.unwrap_or(fallback))
    }

    fn parse(spec: &str) -> anyhow::Result<Option<Self>> {
        let mut filter = Self::off();
        let mut found = false;

        for directive in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            found = true;
            if directive.contains(['[', ']', '{', '}']) {
                anyhow::bail!(
                    "span and field filters are not supported in RUST_LOG: {directive:?}"
                );
            }

            if let Some((target, level)) = directive.rsplit_once('=') {
                let target = target.trim();
                if target.is_empty() {
                    anyhow::bail!("missing target in RUST_LOG directive: {directive:?}");
                }
                filter
                    .targets
                    .push((target.into(), parse_level(level.trim())?));
            } else if let Ok(level) = directive.parse() {
                filter.default = level;
            } else {
                if directive.chars().any(char::is_whitespace) {
                    anyhow::bail!("invalid RUST_LOG directive: {directive:?}");
                }
                filter.targets.push((directive.into(), LevelFilter::Trace));
            }
        }

        Ok(found.then_some(filter))
    }

    fn enabled(&self, target: &str, level: Level) -> bool {
        let mut selected = self.default;
        let mut selected_len = 0;
        for (prefix, filter) in &self.targets {
            if prefix.len() >= selected_len && target.starts_with(prefix.as_ref()) {
                selected = *filter;
                selected_len = prefix.len();
            }
        }
        selected >= level.to_level_filter()
    }

    fn max_level(&self) -> LevelFilter {
        self.targets
            .iter()
            .map(|(_, level)| *level)
            .fold(self.default, std::cmp::max)
    }
}

struct Logger {
    console: TargetFilter,
    console_max_level: u8,
    active_max_level: AtomicU8,
    color: bool,
    file: FileSink,
}

impl Logger {
    fn new(console: TargetFilter, file: FileSink) -> Self {
        let console_max_level = level_rank(console.max_level());
        let active_max_level = console_max_level.max(file.max_level_rank());
        Self {
            console,
            console_max_level,
            active_max_level: AtomicU8::new(active_max_level),
            color: io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
            file,
        }
    }

    fn enabled(&self, target: &str, level: Level) -> bool {
        // ⚠️ 内存缓冲必须在**这里**放行，不能只在 `emit()` 里判断：
        // `log` crate 在 `enabled()` 返回 false 时直接跳过，**根本不会**调用 `log()`/`emit()`。
        // 所以缓冲有自己的级别阈值（默认 INFO），与 console/file 无关。
        if let Some(buf) = MEMORY_BUFFER.get() {
            if buf.accepts(level) {
                return true;
            }
        }
        level_is_enabled(self.active_max_level.load(Ordering::Acquire), level)
            && (self.console_enabled(target, level) || self.file.enabled(target, level))
    }

    fn console_enabled(&self, target: &str, level: Level) -> bool {
        level_is_enabled(self.console_max_level, level) && self.console.enabled(target, level)
    }

    fn max_level(&self) -> LevelFilter {
        self.console
            .max_level()
            .max(self.file.max_level())
            .max(memory_buffer_level())
    }

    fn emit(&self, level: Level, target: &str, message: &str) {
        let console_enabled = self.console_enabled(target, level);
        let file_enabled = self.file.enabled(target, level);
        let memory = MEMORY_BUFFER.get();
        let memory_enabled = memory.is_some_and(|buf| buf.accepts(level));
        if !console_enabled && !file_enabled && !memory_enabled {
            return;
        }

        let timestamp = timestamp_rfc3339_utc();
        if console_enabled {
            let line = format_line(&timestamp, level, target, message, self.color);
            let _ = if matches!(level, Level::Error | Level::Warn) {
                io::stderr().lock().write_all(line.as_bytes())
            } else {
                io::stdout().lock().write_all(line.as_bytes())
            };
        }

        if file_enabled {
            self.file.emit(&timestamp, level, target, message);
        }

        if let Some(buf) = memory {
            if buf.accepts(level) {
                buf.push(timestamp, level, target, message);
            }
        }
    }

    fn set_file_level(&self, level: LevelFilter) -> anyhow::Result<()> {
        // `set_level` 的第二参是与 file 合并进 active max 的「另一侧」；
        // 必须把 memory buffer 级别算进去，否则 reload 文件级别会把全局 max 压回去。
        let other = self.console.max_level().max(memory_buffer_level());
        self.file
            .set_level(level, other, &self.active_max_level)
    }

    fn file_level(&self) -> LevelFilter {
        self.file.level()
    }

    fn flush_file(&self) {
        self.file.flush();
    }
}

fn level_rank(level: LevelFilter) -> u8 {
    match level {
        LevelFilter::Off => 0,
        LevelFilter::Error => 1,
        LevelFilter::Warn => 2,
        LevelFilter::Info => 3,
        LevelFilter::Debug => 4,
        LevelFilter::Trace => 5,
    }
}

fn level_is_enabled(max_level: u8, level: Level) -> bool {
    max_level >= level_rank(level.to_level_filter())
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &LogMetadata<'_>) -> bool {
        self.enabled(metadata.target(), metadata.level())
    }

    fn log(&self, record: &LogRecord<'_>) {
        let target = record.target();
        let level = record.level();
        // Only format args for content matching when demote could apply; otherwise
        // skip formatting until the record is actually enabled.
        if level == Level::Error && target.starts_with("wintun") {
            let message = record.args().to_string();
            let level = effective_log_level(target, level, &message);
            if self.enabled(target, level) {
                self.emit(level, target, &message);
            }
            return;
        }
        if self.enabled(target, level) {
            self.emit(level, target, &record.args().to_string());
        }
    }

    fn flush(&self) {
        self.flush_file();
    }
}

/// rust-tun always tries Adapter::open before create; a miss logs ERROR from the
/// native WinTun DLL even when create then succeeds. Demote that expected noise.
fn effective_log_level(target: &str, level: Level, message: &str) -> Level {
    if level == Level::Error
        && target.starts_with("wintun")
        && message.contains("Failed to find matching adapter")
    {
        Level::Debug
    } else {
        level
    }
}

fn format_line(timestamp: &str, level: Level, target: &str, message: &str, color: bool) -> String {
    let mut line = String::with_capacity(timestamp.len() + target.len() + message.len() + 32);
    if color {
        let color = match level {
            Level::Error => "\x1b[31m",
            Level::Warn => "\x1b[33m",
            Level::Info => "\x1b[32m",
            Level::Debug => "\x1b[34m",
            Level::Trace => "\x1b[90m",
        };
        let _ = writeln!(
            line,
            "{timestamp} {color}{level:<5}\x1b[0m {target}: {message}"
        );
    } else {
        let _ = writeln!(line, "{timestamp} {level:<5} {target}: {message}");
    }
    line
}

fn timestamp_rfc3339_utc() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration.as_secs();
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_date_from_unix_days((seconds / 86_400) as i64);
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;

    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        duration.subsec_millis()
    )
}

fn civil_date_from_unix_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn tracing_level(level: &tracing::Level) -> Level {
    match *level {
        tracing::Level::ERROR => Level::Error,
        tracing::Level::WARN => Level::Warn,
        tracing::Level::INFO => Level::Info,
        tracing::Level::DEBUG => Level::Debug,
        tracing::Level::TRACE => Level::Trace,
    }
}

fn emit_event(logger: &Logger, event: &Event<'_>) {
    let metadata = event.metadata();
    let level = tracing_level(metadata.level());
    if !logger.enabled(metadata.target(), level) {
        return;
    }

    let mut fields = EventFields::default();
    event.record(&mut fields);
    logger.emit(level, metadata.target(), &fields.finish());
}

#[derive(Default)]
struct EventFields {
    message: Option<String>,
    fields: String,
}

impl EventFields {
    fn write_field(&mut self, field: &Field, value: impl fmt::Display) {
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        let _ = write!(self.fields, "{}={value}", field.name());
    }

    fn finish(self) -> String {
        match (self.message, self.fields.is_empty()) {
            (Some(message), false) => format!("{message} {}", self.fields),
            (Some(message), true) => message,
            (None, _) => self.fields,
        }
    }
}

impl Visit for EventFields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        } else {
            self.write_field(field, format_args!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_owned());
        } else {
            self.write_field(field, format_args!("{value:?}"));
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.write_field(field, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "management")]
    use crate::common::config::FileLoggerConfig;

    struct EnvVarGuard {
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(value: Option<&str>) -> Self {
            let previous = std::env::var_os("RUST_LOG");
            match value {
                Some(value) => unsafe { std::env::set_var("RUST_LOG", value) },
                None => unsafe { std::env::remove_var("RUST_LOG") },
            }
            Self { previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var("RUST_LOG", value) },
                None => unsafe { std::env::remove_var("RUST_LOG") },
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn default_console_only_enables_core_info() {
        let _env = EnvVarGuard::set(None);
        let filter = TargetFilter::console(None).unwrap();

        assert!(filter.enabled("CORE::peer", Level::Info));
        assert!(!filter.enabled("CORE", Level::Debug));
        assert!(!filter.enabled("other", Level::Error));
    }

    #[test]
    #[serial_test::serial]
    fn additional_default_console_targets_are_scoped() {
        let _env = EnvVarGuard::set(None);
        let filter =
            TargetFilter::console_with_default_targets(None, &[LOG_TARGET, "easytier_web"])
                .unwrap();

        assert!(filter.enabled("CORE::peer", Level::Info));
        assert!(filter.enabled("easytier_web::client_manager", Level::Info));
        assert!(!filter.enabled("easytier_web", Level::Debug));
        assert!(!filter.enabled("other", Level::Error));
    }

    #[test]
    fn rust_log_supports_global_and_target_levels() {
        let filter = TargetFilter::parse("warn,easytier_core=debug,hyper=off")
            .unwrap()
            .unwrap();

        assert!(filter.enabled("easytier_core::peers", Level::Debug));
        assert!(!filter.enabled("hyper::client", Level::Error));
        assert!(!filter.enabled("other", Level::Info));
        assert!(filter.enabled("other", Level::Warn));
    }

    #[test]
    fn demotes_wintun_adapter_miss_error_to_debug() {
        let msg = "WinTun: Failed to find matching adapter name: 找不到元素。 (Code 0x00000490)";
        assert_eq!(
            effective_log_level("wintun::log", Level::Error, msg),
            Level::Debug
        );
        assert_eq!(
            effective_log_level("wintun", Level::Error, msg),
            Level::Debug
        );
    }

    #[test]
    fn does_not_demote_unrelated_wintun_or_other_errors() {
        assert_eq!(
            effective_log_level(
                "wintun::log",
                Level::Error,
                "WinTun: Failed to create adapter"
            ),
            Level::Error
        );
        assert_eq!(
            effective_log_level(
                "other::log",
                Level::Error,
                "Failed to find matching adapter name"
            ),
            Level::Error
        );
        assert_eq!(
            effective_log_level(
                "wintun::log",
                Level::Warn,
                "Failed to find matching adapter name"
            ),
            Level::Warn
        );
    }

    #[test]
    fn rust_log_target_without_level_enables_trace() {
        let filter = TargetFilter::parse("easytier_core").unwrap().unwrap();

        assert!(filter.enabled("easytier_core::peer", Level::Trace));
        assert!(!filter.enabled("other", Level::Error));
    }

    #[test]
    fn formatting_is_compact_and_color_is_optional() {
        let plain = format_line(
            "2026-07-22T12:00:00+08:00",
            Level::Info,
            "CORE",
            "ready",
            false,
        );
        let colored = format_line(
            "2026-07-22T12:00:00+08:00",
            Level::Info,
            "CORE",
            "ready",
            true,
        );

        assert_eq!(plain, "2026-07-22T12:00:00+08:00 INFO  CORE: ready\n");
        assert!(colored.contains("\x1b[32mINFO \x1b[0m"));
    }

    #[test]
    fn unix_day_conversion_matches_known_dates() {
        assert_eq!(civil_date_from_unix_days(0), (1970, 1, 1));
        assert_eq!(civil_date_from_unix_days(20_656), (2026, 7, 22));
    }

    #[test]
    #[cfg(feature = "management")]
    fn default_file_logger_is_not_opened_without_reload() {
        let file = FileSink::from_config(FileLoggerConfig::default(), false).unwrap();
        assert!(!file.is_open());
    }

    #[test]
    #[cfg(feature = "management")]
    #[serial_test::serial]
    fn file_logger_uses_rust_log_when_enabled() {
        let _env = EnvVarGuard::set(Some("debug"));
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("env-filter.log");
        let config = FileLoggerConfig {
            level: Some("info".to_owned()),
            file: Some("env-filter.log".to_owned()),
            dir: Some(temp_dir.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let file = FileSink::from_config(config, true).unwrap();
        let logger = Logger::new(TargetFilter::off(), file);

        logger.emit(Level::Debug, LOG_TARGET, "env-filter-marker");
        logger.flush_file();

        let content = std::fs::read_to_string(log_path).unwrap();
        assert!(content.contains("env-filter-marker"));
    }

    #[test]
    #[cfg(feature = "management")]
    #[serial_test::serial]
    fn reloading_file_logger_preserves_rust_log_and_supports_off() {
        let _env = EnvVarGuard::set(Some("debug"));
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("reload.log");
        let config = FileLoggerConfig {
            file: Some("reload.log".to_owned()),
            dir: Some(temp_dir.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let file = FileSink::from_config(config, true).unwrap();
        let logger = std::sync::Arc::new(Logger::new(TargetFilter::off(), file));

        // File sink defaults to Warn when no explicit level is set, but
        // RUST_LOG=debug (set above) still raises the effective max level.
        assert_eq!(
            logger.active_max_level.load(Ordering::Relaxed),
            level_rank(LevelFilter::Debug)
        );

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(9));
        let threads = (0..8)
            .map(|thread_index| {
                let logger = logger.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for iteration in 0..500 {
                        let level = if (thread_index + iteration) % 2 == 0 {
                            LevelFilter::Info
                        } else {
                            LevelFilter::Off
                        };
                        logger.set_file_level(level).unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }

        logger.file.assert_level_state(
            logger.console_max_level,
            logger.active_max_level.load(Ordering::Relaxed),
        );

        logger.set_file_level(LevelFilter::Info).unwrap();
        assert_eq!(
            logger.active_max_level.load(Ordering::Relaxed),
            level_rank(LevelFilter::Debug)
        );
        assert_eq!(logger.file.max_level_rank(), level_rank(LevelFilter::Debug));
        logger.emit(Level::Debug, LOG_TARGET, "enabled-by-env");
        logger.set_file_level(LevelFilter::Off).unwrap();
        assert_eq!(
            logger.active_max_level.load(Ordering::Relaxed),
            level_rank(LevelFilter::Off)
        );
        logger.emit(Level::Error, LOG_TARGET, "disabled-despite-env");
        logger.flush_file();

        let content = std::fs::read_to_string(log_path).unwrap();
        assert!(content.contains("enabled-by-env"));
        assert!(!content.contains("disabled-despite-env"));
        assert_eq!(logger.file_level(), LevelFilter::Off);
    }

    #[test]
    #[cfg(all(feature = "management", not(feature = "tracing")))]
    #[serial_test::serial]
    fn tracing_events_and_direct_log_records_share_the_file_sink() {
        let _env = EnvVarGuard::set(None);
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("shared-sink.log");
        let config = FileLoggerConfig {
            level: Some("info".to_owned()),
            file: Some("shared-sink.log".to_owned()),
            dir: Some(temp_dir.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let file = FileSink::from_config(config, false).unwrap();
        let logger = Box::leak(Box::new(Logger::new(TargetFilter::off(), file)));
        let dispatch = tracing::Dispatch::new(tracing_backend::EventSubscriber::new(logger));

        tracing::dispatcher::with_default(&dispatch, || {
            let span = tracing::info_span!(target: LOG_TARGET, "ignored-span", peer = 7);
            let _entered = span.enter();
            tracing::info!(target: LOG_TARGET, answer = 42, "tracing-event");
        });
        log::Log::log(
            logger,
            &LogRecord::builder()
                .level(Level::Info)
                .target("dependency")
                .args(format_args!("direct-log-record"))
                .build(),
        );
        logger.flush_file();

        let content = std::fs::read_to_string(log_path).unwrap();
        assert!(content.contains("tracing-event answer=42"));
        assert!(content.contains("direct-log-record"));
        assert!(!content.contains("ignored-span"));
    }
}
