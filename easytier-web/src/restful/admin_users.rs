use axum::{
    Json, Router,
    extract::Path,
    http::StatusCode,
    routing::{get, put},
};
use axum_login::login_required;
use easytier::proto::common::Void;

use super::{
    AppStateInner, HttpHandleError, other_error,
    users::{AdminCreateUser, AuthSession, Backend, ChangePassword, MeResponse, UserInfo},
};

pub fn router() -> Router<AppStateInner> {
    Router::new()
        .route("/api/v1/auth/me", get(get_me))
        .route("/api/v1/users", get(list_users).post(create_user))
        .route("/api/v1/users/{id}", axum::routing::delete(delete_user))
        .route("/api/v1/users/{id}/password", put(reset_password))
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

async fn get_me(auth_session: AuthSession) -> Result<Json<MeResponse>, HttpHandleError> {
    let Some(user) = auth_session.user.as_ref() else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json::from(other_error("Not logged in")),
        ));
    };

    let is_admin = auth_session
        .backend
        .user_is_admin(user)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json::from(other_error(format!("{:?}", e))),
            )
        })?;

    // Unified with list/OIDC semantics: live table rows win, empty collapses
    // to "" — the `revoked_*` placeholder from sync_primary is never exposed.
    let (config_token, config_tokens) =
        super::users::resolve_tokens_and_primary(&user.db_user.config_token, user.tokens.clone());
    Ok(Json(MeResponse {
        id: user.db_user.id,
        username: user.db_user.username.clone(),
        is_admin,
        config_token,
        config_tokens,
    }))
}

async fn list_users(auth_session: AuthSession) -> Result<Json<Vec<UserInfo>>, HttpHandleError> {
    require_admin(&auth_session).await?;

    match auth_session.backend.list_users().await {
        Ok(users) => Ok(Json(users)),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json::from(other_error(format!("{:?}", e))),
        )),
    }
}

async fn create_user(
    auth_session: AuthSession,
    Json(req): Json<AdminCreateUser>,
) -> Result<Json<UserInfo>, HttpHandleError> {
    require_admin(&auth_session).await?;

    match auth_session.backend.create_user_by_admin(&req).await {
        Ok(user) => Ok(Json(user)),
        Err(e) => {
            tracing::error!("Failed to create user: {:?}", e);
            Err((
                StatusCode::BAD_REQUEST,
                Json::from(other_error(format!("{:?}", e))),
            ))
        }
    }
}

async fn delete_user(
    auth_session: AuthSession,
    Path(id): Path<i32>,
) -> Result<Json<Void>, HttpHandleError> {
    require_admin(&auth_session).await?;

    let Some(actor) = auth_session.user.as_ref() else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json::from(other_error("Not logged in")),
        ));
    };

    match auth_session.backend.delete_user(id, actor.db_user.id).await {
        Ok(()) => Ok(Json(Void::default())),
        Err(e) => {
            tracing::error!("Failed to delete user {}: {:?}", id, e);
            Err((
                StatusCode::BAD_REQUEST,
                Json::from(other_error(format!("{:?}", e))),
            ))
        }
    }
}

async fn reset_password(
    auth_session: AuthSession,
    Path(id): Path<i32>,
    Json(req): Json<ChangePassword>,
) -> Result<Json<Void>, HttpHandleError> {
    require_admin(&auth_session).await?;

    if req.new_password.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json::from(other_error("Password is required")),
        ));
    }

    match auth_session.backend.change_password(id, &req).await {
        Ok(()) => Ok(Json(Void::default())),
        Err(e) => {
            tracing::error!("Failed to reset password for user {}: {:?}", id, e);
            Err((
                StatusCode::BAD_REQUEST,
                Json::from(other_error(format!("{:?}", e))),
            ))
        }
    }
}
