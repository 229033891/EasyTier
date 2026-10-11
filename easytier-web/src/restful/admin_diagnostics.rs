//! Admin-only system diagnostics and in-process runtime logs.

use std::sync::{Arc, OnceLock};

use super::{
    AppStateInner, HttpHandleError, other_error, other_error_with_code,
    users::{AuthSession, Backend},
};
use crate::client_manager::{ClientManager, HeartbeatPolicy};
use crate::db::Db;
use axum::{Extension, Json, Router, extract::Query, http::StatusCode, routing::get};
use axum_login::{AuthUser, login_required};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Cap concurrent log-file reads. The process uses a small tokio worker pool;
/// unbounded `spawn_blocking` from many admin tabs can starve other work.
const MAX_CONCURRENT_LOG_FILE_READS: usize = 2;

fn log_file_read_semaphore() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_LOG_FILE_READS)))
}

async fn acquire_log_file_read_permit() -> Result<OwnedSemaphorePermit, HttpHandleError> {
    Arc::clone(log_file_read_semaphore())
        .acquire_owned()
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json::from(other_error_with_code(
                    "log read queue shut down",
                    "log_read_unavailable",
                )),
            )
        })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeIntentSnapshot {
    pub version: String,
    pub api_listen: String,
    pub config_server_port: u16,
    pub config_server_protocol: String,
    pub heartbeat_min_response_ms: u64,
    pub heartbeat_timeout_ms: u64,
    pub db_path: String,
    pub console_log_level: Option<String>,
    pub file_log_dir: Option<String>,
    pub webhook_configured: bool,
    pub oidc_configured: bool,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsState {
    pub intent: RuntimeIntentSnapshot,
    pub client_mgr: Arc<ClientManager>,
    pub db: Db,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticCheck {
    pub id: String,
    /// 稳定机器标识；前端按 `web.system_diagnostics.checks.<id>` 取本地化标题。
    pub status: String,
    pub expected: String,
    pub actual: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<DiagnosticDetail>,
}

/// 说明字段：给稳定 code + 自由参数，由前端 i18n 渲染，避免后端硬编码中文。
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticDetail {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
}

impl DiagnosticDetail {
    fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            param: None,
        }
    }

    fn with_param(code: impl Into<String>, param: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            param: Some(param.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemDiagnosticsResponse {
    pub overall: String,
    pub summary: OverallSummary,
    pub generated_at: String,
    pub checks: Vec<DiagnosticCheck>,
    pub snapshot: DiagnosticsSnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsSnapshot {
    pub version: String,
    pub api_listen: String,
    pub config_server_port: u16,
    pub config_server_protocol: String,
    pub config_server_listening: Vec<String>,
    pub heartbeat_min_response_ms: u64,
    pub heartbeat_timeout_ms: u64,
    pub db_path: String,
    pub console_log_level: Option<String>,
    pub file_log_dir: Option<String>,
    pub webhook_configured: bool,
    pub oidc_configured: bool,
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    #[serde(default = "default_tail")]
    pub tail: usize,
}

fn default_tail() -> usize {
    500
}

#[derive(Debug, Clone, Serialize)]
pub struct LogLineJson {
    pub ts: String,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogsResponse {
    pub capacity: usize,
    pub lines: Vec<LogLineJson>,
}

pub fn router() -> Router<AppStateInner> {
    Router::new()
        .route(
            "/api/v1/admin/system-diagnostics",
            get(handle_system_diagnostics),
        )
        .route("/api/v1/admin/logs", get(handle_logs))
        .route("/api/v1/admin/logs/files", get(handle_log_files))
        .route("/api/v1/admin/logs/file", get(handle_log_file_query))
        .route_layer(login_required!(Backend))
}

/// Best-effort actor id for audit lines (never fails the request).
fn user_id_of(auth_session: &AuthSession) -> String {
    auth_session
        .user
        .as_ref()
        .map(|u| u.id().to_string())
        .unwrap_or_else(|| "anonymous".to_string())
}

async fn require_admin(auth_session: &AuthSession) -> Result<(), HttpHandleError> {
    let Some(user) = auth_session.user.as_ref() else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json::from(other_error("Not logged in")),
        ));
    };

    match auth_session.backend.user_is_admin(user).await {
        Ok(true) => Ok(()),
        Ok(false) => Err((
            StatusCode::FORBIDDEN,
            Json::from(other_error("Admin permission required")),
        )),
        Err(e) => {
            tracing::error!("Failed to check admin permission: {e:?}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json::from(super::other_error_with_code(
                    "Failed to check admin permission",
                    "internal_error",
                )),
            ))
        }
    }
}

fn normalize_protocol_list(spec: &str) -> Vec<String> {
    let mut parts: Vec<String> = spec
        .split(',')
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    parts.sort();
    parts.dedup();
    parts
}

fn join_list(parts: &[String]) -> String {
    if parts.is_empty() {
        // Language-neutral token; UI maps via web.system_diagnostics.value_none.
        "none".to_string()
    } else {
        parts.join(",")
    }
}

fn check_pass(id: &str, expected: &str, actual: &str) -> DiagnosticCheck {
    DiagnosticCheck {
        id: id.to_string(),
        status: "pass".to_string(),
        expected: expected.to_string(),
        actual: actual.to_string(),
        detail: None,
    }
}

fn check_fail(id: &str, expected: &str, actual: &str, detail: DiagnosticDetail) -> DiagnosticCheck {
    DiagnosticCheck {
        id: id.to_string(),
        status: "fail".to_string(),
        expected: expected.to_string(),
        actual: actual.to_string(),
        detail: Some(detail),
    }
}

/// 汇总时只回稳定 code + 参数，前端按 `web.system_diagnostics.overall_summary.<code>` 渲染。
#[derive(Debug, Clone, Serialize)]
pub struct OverallSummary {
    pub code: String,
    pub params: Vec<String>,
}

fn compute_overall(checks: &[DiagnosticCheck]) -> (&'static str, OverallSummary) {
    let mut has_fail = false;
    let mut critical_fail = false;
    let mut fail_ids = Vec::new();

    for c in checks {
        if c.status == "fail" {
            has_fail = true;
            if c.id == "db_ok" {
                critical_fail = true;
            }
            fail_ids.push(c.id.clone());
        }
    }

    if !has_fail {
        return (
            "healthy",
            OverallSummary {
                code: "all_passed".to_string(),
                params: Vec::new(),
            },
        );
    }

    let code = if critical_fail {
        "unhealthy"
    } else {
        "degraded"
    };
    (
        code,
        OverallSummary {
            code: code.to_string(),
            params: fail_ids,
        },
    )
}

async fn build_diagnostics(state: &DiagnosticsState) -> SystemDiagnosticsResponse {
    let intent = &state.intent;
    let listening = state.client_mgr.config_server_listening_schemes();
    let listening_ports = state.client_mgr.config_server_listening_ports();
    let expected_protocols = normalize_protocol_list(&intent.config_server_protocol);
    let expected_joined = join_list(&expected_protocols);
    let actual_joined = join_list(&listening);

    let mut checks = Vec::new();

    checks.push(check_pass("api_up", "reachable", "ok"));

    if expected_protocols == listening {
        checks.push(check_pass(
            "config_server_protocol",
            &expected_joined,
            &actual_joined,
        ));
    } else {
        checks.push(check_fail(
            "config_server_protocol",
            &expected_joined,
            &actual_joined,
            DiagnosticDetail::new("listener_mismatch"),
        ));
    }

    let expected_port = intent.config_server_port.to_string();
    let actual_ports = if listening_ports.is_empty() {
        "none".to_string()
    } else {
        listening_ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    if listening_ports.contains(&intent.config_server_port) {
        checks.push(check_pass(
            "config_server_port",
            &expected_port,
            &actual_ports,
        ));
    } else {
        checks.push(check_fail(
            "config_server_port",
            &expected_port,
            &actual_ports,
            DiagnosticDetail::new("port_not_bound"),
        ));
    }

    let db_ok = match sqlx::query("SELECT 1").fetch_one(&state.db.inner()).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, "diagnostics db probe failed");
            false
        }
    };
    if db_ok {
        checks.push(check_pass("db_ok", "readable", "ok"));
    } else {
        checks.push(check_fail(
            "db_ok",
            "readable",
            "error",
            DiagnosticDetail::new("db_query_failed"),
        ));
    }

    let hb_expected = format!(
        "min={}, timeout={}",
        intent.heartbeat_min_response_ms, intent.heartbeat_timeout_ms
    );
    match HeartbeatPolicy::from_millis(
        intent.heartbeat_min_response_ms,
        intent.heartbeat_timeout_ms,
    ) {
        Ok(_) => checks.push(check_pass("heartbeat_sane", &hb_expected, "ok")),
        Err(e) => checks.push(check_fail(
            "heartbeat_sane",
            &hb_expected,
            "invalid",
            // 这里是规则文案（如「interval 必须在 1000..60000」），保留原文；
            // 前端只把 param 当补充信息展示，不做 i18n。
            DiagnosticDetail::with_param("invalid_params", e.to_string()),
        )),
    }

    let webhook_actual = if intent.webhook_configured {
        "configured"
    } else {
        "not_configured"
    };
    checks.push(check_pass("webhook", "optional", webhook_actual));
    let oidc_actual = if intent.oidc_configured {
        "configured"
    } else {
        "not_configured"
    };
    checks.push(check_pass("oidc", "optional", oidc_actual));

    let (overall, summary) = compute_overall(&checks);

    SystemDiagnosticsResponse {
        overall: overall.to_string(),
        summary,
        generated_at: Utc::now().to_rfc3339(),
        checks,
        snapshot: DiagnosticsSnapshot {
            version: intent.version.clone(),
            api_listen: intent.api_listen.clone(),
            config_server_port: intent.config_server_port,
            config_server_protocol: intent.config_server_protocol.clone(),
            config_server_listening: listening,
            heartbeat_min_response_ms: intent.heartbeat_min_response_ms,
            heartbeat_timeout_ms: intent.heartbeat_timeout_ms,
            db_path: intent.db_path.clone(),
            console_log_level: intent.console_log_level.clone(),
            file_log_dir: intent.file_log_dir.clone(),
            webhook_configured: intent.webhook_configured,
            oidc_configured: intent.oidc_configured,
        },
    }
}

async fn handle_system_diagnostics(
    auth_session: AuthSession,
    Extension(state): Extension<Arc<DiagnosticsState>>,
) -> Result<Json<SystemDiagnosticsResponse>, HttpHandleError> {
    require_admin(&auth_session).await?;
    Ok(Json(build_diagnostics(&state).await))
}

/// Best-effort redact of tokens embedded in config-server / heartbeat log lines.
fn redact_log_message(message: &str) -> String {
    let mut out = message.to_string();
    for scheme in ["udp://", "tcp://", "ws://", "wss://", "UDP://", "TCP://"] {
        let mut search_from = 0;
        while let Some(rel) = out[search_from..].find(scheme) {
            let start = search_from + rel;
            let after_scheme = start + scheme.len();
            let Some(slash) = out[after_scheme..].find('/') else {
                break;
            };
            let token_start = after_scheme + slash + 1;
            let token_end = out[token_start..]
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, '?' | '#' | '"' | '\'' | ',' | '}')
                })
                .map(|i| token_start + i)
                .unwrap_or(out.len());
            if token_end > token_start {
                out.replace_range(token_start..token_end, "***");
                search_from = token_start + 3;
            } else {
                search_from = token_start;
            }
        }
    }
    // JSON-ish `"user_token":"..."`
    let key = "\"user_token\"";
    let mut search_from = 0;
    while let Some(rel) = out[search_from..].to_ascii_lowercase().find(key) {
        let key_at = search_from + rel;
        let after_key = key_at + key.len();
        let Some(quote_rel) = out[after_key..].find('"') else {
            break;
        };
        let value_start = after_key + quote_rel + 1;
        let Some(value_end_rel) = out[value_start..].find('"') else {
            break;
        };
        let value_end = value_start + value_end_rel;
        out.replace_range(value_start..value_end, "***");
        search_from = value_start + 3;
    }
    out
}

async fn handle_logs(
    auth_session: AuthSession,
    Query(query): Query<LogsQuery>,
) -> Result<Json<LogsResponse>, HttpHandleError> {
    require_admin(&auth_session).await?;
    // Cap to ring capacity so callers cannot force oversized clones.
    let (capacity, _) = easytier::common::log::snapshot_memory_logs(0);
    let max_tail = capacity.max(1).min(1000);
    let tail = query.tail.clamp(1, max_tail);
    let (capacity, lines) = easytier::common::log::snapshot_memory_logs(tail);
    Ok(Json(LogsResponse {
        capacity,
        lines: lines
            .into_iter()
            .map(|l| LogLineJson {
                ts: l.ts,
                level: l.level,
                target: l.target,
                message: redact_log_message(&l.message),
            })
            .collect(),
    }))
}

const MAX_LOG_FILE_LIST: usize = 32;
const MAX_LOG_FILE_READ_BYTES: u64 = 1024 * 1024;
const MAX_LOG_FILE_TAIL: usize = 2000;
const MAX_LOG_FILE_LINE_CHARS: usize = 4096;

#[derive(Debug, Clone, Serialize)]
pub struct LogFileInfoJson {
    pub file_name: String,
    pub size_bytes: u64,
    pub modified_ms: i64,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogFilesResponse {
    pub enabled: bool,
    pub dir: String,
    pub level: String,
    pub files: Vec<LogFileInfoJson>,
}

#[derive(Debug, Deserialize)]
pub struct LogFileQuery {
    pub file: String,
    #[serde(default = "default_tail")]
    pub tail: usize,
    pub min_level: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub grep: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogFileLinesResponse {
    pub file: String,
    pub dir: String,
    pub level: String,
    pub total_scanned: usize,
    pub matched: usize,
    pub truncated_bytes: bool,
    pub lines: Vec<LogLineJson>,
}

fn is_easytier_log_file(name: &str) -> bool {
    name == "easytier.log" || name.starts_with("easytier.log.")
}

fn sanitize_log_file_name(raw: &str) -> Option<String> {
    let name = std::path::Path::new(raw)
        .file_name()
        .and_then(|n| n.to_str())?;
    if is_easytier_log_file(name) {
        Some(name.to_string())
    } else {
        None
    }
}

fn modified_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn truncate_log_message(message: &str) -> String {
    let mut chars = message.chars();
    let truncated: String = chars.by_ref().take(MAX_LOG_FILE_LINE_CHARS).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn level_rank_name(level: &str) -> Option<u8> {
    match level.to_ascii_uppercase().as_str() {
        "ERROR" => Some(5),
        "WARN" => Some(4),
        "INFO" => Some(3),
        "DEBUG" => Some(2),
        "TRACE" => Some(1),
        _ => None,
    }
}

/// Regular file only — never follow a symlink planted in the log directory:
/// `is_file()` would happily read a link pointing outside the directory.
fn is_regular_file(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

/// Parse a colorless `format_line` record. Target may contain `::`; split on `: `.
fn parse_file_log_line(line: &str) -> LogLineJson {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let Some((ts, rest)) = trimmed.split_once(' ') else {
        return unknown_log_line(trimmed);
    };
    let rest = rest.trim_start();
    let Some((level_tok, after_level)) = rest.split_once(|c: char| c.is_whitespace()) else {
        return unknown_log_line(trimmed);
    };
    let level = level_tok.trim().to_ascii_uppercase();
    if level_rank_name(&level).is_none() {
        return unknown_log_line(trimmed);
    }
    let after_level = after_level.trim_start();
    let Some((target, message)) = after_level.split_once(": ") else {
        return unknown_log_line(trimmed);
    };
    LogLineJson {
        ts: ts.to_string(),
        level,
        target: target.to_string(),
        message: truncate_log_message(&redact_log_message(message)),
    }
}

fn unknown_log_line(line: &str) -> LogLineJson {
    LogLineJson {
        ts: String::new(),
        level: "UNKNOWN".to_string(),
        target: String::new(),
        message: truncate_log_message(&redact_log_message(line)),
    }
}

fn parse_time_bound(raw: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M")
                .ok()
                .map(|naive| naive.and_utc())
        })
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .map(|naive| naive.and_utc())
        })
}

fn line_passes_filters(
    line: &LogLineJson,
    min_level: Option<u8>,
    since: Option<chrono::DateTime<Utc>>,
    until: Option<chrono::DateTime<Utc>>,
    grep: Option<&str>,
) -> bool {
    if line.level != "UNKNOWN" {
        if let Some(min) = min_level {
            let Some(rank) = level_rank_name(&line.level) else {
                return false;
            };
            if rank < min {
                return false;
            }
        }
    }
    if since.is_some() || until.is_some() {
        if line.ts.is_empty() {
            // UNKNOWN / unparsed timestamps are kept (not dropped by time filter).
        } else if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&line.ts) {
            let ts = ts.with_timezone(&Utc);
            if since.is_some_and(|s| ts < s) {
                return false;
            }
            if until.is_some_and(|u| ts > u) {
                return false;
            }
        }
    }
    if let Some(g) = grep {
        let hay = format!("{} {}", line.target, line.message);
        if !hay.to_ascii_lowercase().contains(&g.to_ascii_lowercase()) {
            return false;
        }
    }
    true
}

/// Open a log file without following symlinks; reject hard-linked inodes.
///
/// `symlink_metadata` alone is not enough: `File::open` follows symlinks
/// (TOCTOU), and a same-dir hardlink can point at an inode outside the log dir.
fn open_log_file_readonly(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::io::{Error, ErrorKind};
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let meta = file.metadata()?;
        if meta.nlink() > 1 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "refusing hard-linked log file",
            ));
        }
        Ok(file)
    }
    #[cfg(windows)]
    {
        use std::io::{Error, ErrorKind};
        use std::os::windows::fs::MetadataExt;
        let pre = std::fs::symlink_metadata(path)?;
        if pre.file_type().is_symlink() || !pre.is_file() {
            return Err(Error::new(ErrorKind::InvalidInput, "not a regular file"));
        }
        let file = std::fs::File::open(path)?;
        let post = file.metadata()?;
        if pre.file_index() != post.file_index()
            || pre.volume_serial_number() != post.volume_serial_number()
        {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "log path changed during open",
            ));
        }
        if post.number_of_links() > 1 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "refusing hard-linked log file",
            ));
        }
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::File::open(path)
    }
}

fn read_file_tail(path: &std::path::Path, max_bytes: u64) -> std::io::Result<(String, bool)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = open_log_file_readonly(path)?;
    let len = file.metadata()?.len();
    let truncated = len > max_bytes;
    if truncated {
        file.seek(SeekFrom::End(-(max_bytes as i64)))?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let content = String::from_utf8_lossy(&buf);
    if truncated {
        if let Some(rest) = content.split_once('\n').map(|(_, rest)| rest) {
            return Ok((rest.to_string(), true));
        }
    }
    Ok((content.into_owned(), truncated))
}

fn list_log_files_sync() -> LogFilesResponse {
    let Some((dir, level)) = easytier::common::log::file_log_config() else {
        return LogFilesResponse {
            enabled: false,
            dir: String::new(),
            level: String::new(),
            files: Vec::new(),
        };
    };
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_regular_file(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !is_easytier_log_file(name) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            files.push(LogFileInfoJson {
                file_name: name.to_string(),
                size_bytes: meta.len(),
                modified_ms: modified_ms(&meta),
                active: name == "easytier.log",
            });
        }
    }
    files.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.modified_ms.cmp(&a.modified_ms))
            .then(a.file_name.cmp(&b.file_name))
    });
    files.truncate(MAX_LOG_FILE_LIST);
    LogFilesResponse {
        enabled: true,
        dir: dir.display().to_string(),
        level: level.to_string().to_ascii_uppercase(),
        files,
    }
}

struct FileQueryPlan {
    path: std::path::PathBuf,
    file_name: String,
    dir: String,
    level: String,
    tail: usize,
    min_level: Option<u8>,
    since: Option<chrono::DateTime<Utc>>,
    until: Option<chrono::DateTime<Utc>>,
    grep: Option<String>,
}

fn query_log_file_sync(plan: FileQueryPlan) -> std::io::Result<LogFileLinesResponse> {
    easytier::common::log::flush();
    let (content, truncated_bytes) = read_file_tail(&plan.path, MAX_LOG_FILE_READ_BYTES)?;
    let mut matched_rev = Vec::new();
    let mut total_scanned = 0usize;
    for raw in content.lines().rev() {
        if raw.is_empty() {
            continue;
        }
        total_scanned += 1;
        let parsed = parse_file_log_line(raw);
        if line_passes_filters(
            &parsed,
            plan.min_level,
            plan.since,
            plan.until,
            plan.grep.as_deref(),
        ) {
            matched_rev.push(parsed);
            if matched_rev.len() >= plan.tail {
                break;
            }
        }
    }
    matched_rev.reverse();
    let matched = matched_rev.len();
    Ok(LogFileLinesResponse {
        file: plan.file_name,
        dir: plan.dir,
        level: plan.level,
        total_scanned,
        matched,
        truncated_bytes,
        lines: matched_rev,
    })
}

async fn handle_log_files(
    auth_session: AuthSession,
) -> Result<Json<LogFilesResponse>, HttpHandleError> {
    require_admin(&auth_session).await?;
    let resp = tokio::task::spawn_blocking(list_log_files_sync)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json::from(other_error(format!("log list task failed: {e}"))),
            )
        })?;
    Ok(Json(resp))
}

async fn handle_log_file_query(
    auth_session: AuthSession,
    Query(query): Query<LogFileQuery>,
) -> Result<Json<LogFileLinesResponse>, HttpHandleError> {
    require_admin(&auth_session).await?;
    let _permit = acquire_log_file_read_permit().await?;
    let Some(file_name) = sanitize_log_file_name(&query.file) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json::from(other_error_with_code(
                "invalid log file name",
                "invalid_log_file",
            )),
        ));
    };
    let Some((dir, level)) = easytier::common::log::file_log_config() else {
        return Err((
            StatusCode::NOT_FOUND,
            Json::from(other_error_with_code(
                "file logging is not enabled",
                "file_logging_disabled",
            )),
        ));
    };
    let path = dir.join(&file_name);
    // Log lines can carry tokens / internal addresses, so record who pulled
    // them (the memory-ring endpoint stays un-audited for parity with console).
    tracing::info!(
        actor = %user_id_of(&auth_session),
        file = %file_name,
        tail = query.tail.clamp(1, MAX_LOG_FILE_TAIL),
        min_level = ?query.min_level,
        since = ?query.since,
        until = ?query.until,
        grep = query.grep.is_some(),
        "admin read console log file"
    );
    // Rolling appender creates the active file lazily on first write. While
    // file logging is enabled but nothing has been emitted yet, treat as empty
    // rather than 404 so the UI can show an empty state. A non-regular file
    // (symlink planted in the log dir, directory, …) is rejected instead of
    // being silently followed or reported as "no logs".
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json::from(other_error_with_code(
                    "log path is not a regular file",
                    "invalid_log_file",
                )),
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Json(LogFileLinesResponse {
                file: file_name,
                dir: dir.display().to_string(),
                level: level.to_string().to_ascii_uppercase(),
                total_scanned: 0,
                matched: 0,
                truncated_bytes: false,
                lines: Vec::new(),
            }));
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json::from(other_error(format!("cannot stat log file: {e}"))),
            ));
        }
    }
    let min_level = match query.min_level.as_deref() {
        None => None,
        Some(raw) => match level_rank_name(raw) {
            Some(rank) => Some(rank),
            None => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json::from(other_error_with_code(
                        "invalid min_level",
                        "invalid_min_level",
                    )),
                ));
            }
        },
    };
    let since = match query.since.as_deref() {
        None => None,
        Some(raw) => Some(parse_time_bound(raw).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json::from(other_error_with_code("invalid since", "invalid_since")),
            )
        })?),
    };
    let until = match query.until.as_deref() {
        None => None,
        Some(raw) => Some(parse_time_bound(raw).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json::from(other_error_with_code("invalid until", "invalid_until")),
            )
        })?),
    };
    let plan = FileQueryPlan {
        path,
        file_name: file_name.clone(),
        dir: dir.display().to_string(),
        level: level.to_string().to_ascii_uppercase(),
        tail: query.tail.clamp(1, MAX_LOG_FILE_TAIL),
        min_level,
        since,
        until,
        grep: query.grep.filter(|s| !s.is_empty()),
    };
    let resp = tokio::task::spawn_blocking(move || query_log_file_sync(plan))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json::from(other_error(format!("log read task failed: {e}"))),
            )
        })?
        .map_err(|e| {
            let status = if e.kind() == std::io::ErrorKind::InvalidInput {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            let code = if e.kind() == std::io::ErrorKind::InvalidInput {
                "invalid_log_file"
            } else {
                "log_read_failed"
            };
            (
                status,
                Json::from(other_error_with_code(e.to_string(), code)),
            )
        })?;
    Ok(Json(resp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_log_message_masks_url_path_token_and_user_token_json() {
        let masked = redact_log_message(
            r#"dial udp://10.0.0.1:22020/secret-token heartbeat {"user_token":"abc"}"#,
        );
        assert!(masked.contains("udp://10.0.0.1:22020/***"));
        assert!(!masked.contains("secret-token"));
        assert!(masked.contains(r#""user_token":"***""#));
        assert!(!masked.contains("abc"));
    }

    #[test]
    fn join_list_empty_is_language_neutral_none_token() {
        assert_eq!(join_list(&[]), "none");
        assert_eq!(join_list(&["udp".into(), "tcp".into()]), "udp,tcp");
    }

    #[test]
    fn sanitize_log_file_name_rejects_path_traversal() {
        assert!(sanitize_log_file_name("../et.db").is_none());
        assert!(sanitize_log_file_name("/etc/passwd").is_none());
        assert_eq!(
            sanitize_log_file_name("easytier.log").as_deref(),
            Some("easytier.log")
        );
        assert_eq!(
            sanitize_log_file_name("logs/easytier.log.1").as_deref(),
            Some("easytier.log.1")
        );
    }

    #[test]
    fn parse_file_log_line_keeps_rust_target_path() {
        let line = "2026-10-11T02:03:04.567Z WARN  easytier_web::client_manager: dial failed";
        let parsed = parse_file_log_line(line);
        assert_eq!(parsed.level, "WARN");
        assert_eq!(parsed.target, "easytier_web::client_manager");
        assert_eq!(parsed.message, "dial failed");
        assert_eq!(parsed.ts, "2026-10-11T02:03:04.567Z");
    }

    #[test]
    fn parse_file_log_line_unknown_keeps_stack_context() {
        let parsed = parse_file_log_line("   at easytier_web::foo");
        assert_eq!(parsed.level, "UNKNOWN");
        assert!(parsed.message.contains("at easytier_web::foo"));
    }

    #[test]
    fn line_filters_min_level_keeps_unknown() {
        let warn = parse_file_log_line("2026-10-11T02:03:04.567Z WARN  easytier_web: boom");
        let info = parse_file_log_line("2026-10-11T02:03:04.567Z INFO  easytier_web: ok");
        let unknown = unknown_log_line("stack frame");
        let min = level_rank_name("warn");
        assert!(line_passes_filters(&warn, min, None, None, None));
        assert!(!line_passes_filters(&info, min, None, None, None));
        assert!(line_passes_filters(&unknown, min, None, None, None));
    }

    #[test]
    fn redact_applies_inside_parsed_file_line() {
        let line = r#"2026-10-11T02:03:04.567Z WARN  easytier_web: dial udp://h:22020/tok"#;
        let parsed = parse_file_log_line(line);
        assert!(parsed.message.contains("***"));
        assert!(!parsed.message.contains("/tok"));
    }
}
