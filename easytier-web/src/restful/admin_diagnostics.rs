//! Admin-only system diagnostics and in-process runtime logs.

use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::Query,
    http::StatusCode,
    routing::get,
};
use axum_login::login_required;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use super::{
    AppStateInner, HttpHandleError, other_error,
    users::{AuthSession, Backend},
};
use crate::client_manager::{ClientManager, HeartbeatPolicy};
use crate::db::Db;

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
        .route_layer(login_required!(Backend))
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

fn check_fail(
    id: &str,
    expected: &str,
    actual: &str,
    detail: DiagnosticDetail,
) -> DiagnosticCheck {
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

    let code = if critical_fail { "unhealthy" } else { "degraded" };
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
                .find(|c: char| c.is_whitespace() || matches!(c, '?' | '#' | '"' | '\'' | ',' | '}'))
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
}
