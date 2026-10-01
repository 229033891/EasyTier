use axum::{
    Json, Router,
    extract::Path,
    http::StatusCode,
    routing::{get, put},
};
use axum_login::login_required;
use serde::{Deserialize, Serialize};

use super::{
    AppStateInner, HttpHandleError, convert_db_error, other_error,
    users::{AuthSession, Backend},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigTokenInfo {
    pub id: i32,
    pub user_id: i32,
    pub username: String,
    pub token: String,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateConfigTokenRequest {
    pub user_id: i32,
    /// Custom token (e.g. `admin`). Omit to auto-generate `et_<uuid>`.
    pub token: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateConfigTokenRequest {
    pub token: Option<String>,
    pub label: Option<String>,
}

pub fn router() -> Router<AppStateInner> {
    Router::new()
        .route(
            "/api/v1/admin/config-tokens",
            get(list_tokens).post(create_token),
        )
        .route(
            "/api/v1/admin/config-tokens/{id}",
            put(update_token).delete(delete_token),
        )
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
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json::from(other_error(format!("{:?}", e))),
        )),
    }
}

async fn list_tokens(auth_session: AuthSession) -> Result<Json<Vec<ConfigTokenInfo>>, HttpHandleError> {
    require_admin(&auth_session).await?;
    let rows = auth_session
        .backend
        .db()
        .list_config_tokens()
        .await
        .map_err(convert_db_error)?;
    Ok(Json(
        rows.into_iter()
            .map(|(row, username)| ConfigTokenInfo {
                id: row.id,
                user_id: row.user_id,
                username,
                token: row.token,
                label: row.label,
            })
            .collect(),
    ))
}

async fn create_token(
    auth_session: AuthSession,
    Json(req): Json<CreateConfigTokenRequest>,
) -> Result<Json<ConfigTokenInfo>, HttpHandleError> {
    require_admin(&auth_session).await?;
    let row = auth_session
        .backend
        .db()
        .create_config_token(req.user_id, req.token, req.label)
        .await
        .map_err(|e| {
            if crate::db::is_config_token_taken_err(&e) {
                // Exact sentinel from the pre-check.
                (
                    StatusCode::CONFLICT,
                    Json::from(other_error(crate::db::CONFIG_TOKEN_ALREADY_EXISTS_MSG)),
                )
            } else if crate::db::is_unique_violation_err(&e) {
                // Final defense: UNIQUE constraint fired (race the pre-check missed).
                (
                    StatusCode::CONFLICT,
                    Json::from(other_error(crate::db::CONFIG_TOKEN_ALREADY_EXISTS_MSG)),
                )
            } else {
                let msg = e.to_string();
                if msg.contains("User not found") {
                    (StatusCode::NOT_FOUND, Json::from(other_error(msg)))
                } else if crate::db::is_config_token_validation_err(&e) {
                    (StatusCode::BAD_REQUEST, Json::from(other_error(msg)))
                } else {
                    convert_db_error(e)
                }
            }
        })?;

    let username = auth_session
        .backend
        .db()
        .get_username_by_id(row.user_id)
        .await
        .map_err(convert_db_error)?
        .unwrap_or_default();

    Ok(Json(ConfigTokenInfo {
        id: row.id,
        user_id: row.user_id,
        username,
        token: row.token,
        label: row.label,
    }))
}

async fn update_token(
    auth_session: AuthSession,
    Path(id): Path<i32>,
    Json(req): Json<UpdateConfigTokenRequest>,
) -> Result<Json<ConfigTokenInfo>, HttpHandleError> {
    require_admin(&auth_session).await?;
    let row = auth_session
        .backend
        .db()
        .update_config_token(id, req.token, req.label)
        .await
        .map_err(|e| {
            if crate::db::is_config_token_taken_err(&e) {
                // Exact sentinel from the pre-check.
                (
                    StatusCode::CONFLICT,
                    Json::from(other_error(crate::db::CONFIG_TOKEN_ALREADY_EXISTS_MSG)),
                )
            } else if crate::db::is_unique_violation_err(&e) {
                // Final defense: UNIQUE constraint fired (race the pre-check missed).
                (
                    StatusCode::CONFLICT,
                    Json::from(other_error(crate::db::CONFIG_TOKEN_ALREADY_EXISTS_MSG)),
                )
            } else {
                let msg = e.to_string();
                if msg.contains("not found") {
                    (StatusCode::NOT_FOUND, Json::from(other_error(msg)))
                } else if crate::db::is_config_token_validation_err(&e) {
                    (StatusCode::BAD_REQUEST, Json::from(other_error(msg)))
                } else {
                    convert_db_error(e)
                }
            }
        })?;

    let username = auth_session
        .backend
        .db()
        .get_username_by_id(row.user_id)
        .await
        .map_err(convert_db_error)?
        .unwrap_or_default();

    Ok(Json(ConfigTokenInfo {
        id: row.id,
        user_id: row.user_id,
        username,
        token: row.token,
        label: row.label,
    }))
}

async fn delete_token(
    auth_session: AuthSession,
    Path(id): Path<i32>,
) -> Result<StatusCode, HttpHandleError> {
    require_admin(&auth_session).await?;
    auth_session
        .backend
        .db()
        .delete_config_token(id)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") {
                (StatusCode::NOT_FOUND, Json::from(other_error(msg)))
            } else {
                convert_db_error(e)
            }
        })?;
    Ok(StatusCode::NO_CONTENT)
}
