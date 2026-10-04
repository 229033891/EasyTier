use axum::{
    Router,
    http::StatusCode,
    routing::{get, post, put},
};
use axum_login::login_required;

use crate::restful::users::Backend;

use super::{
    AppStateInner,
    users::{AuthSession, Credentials},
};

pub fn router() -> Router<AppStateInner> {
    let r = Router::new()
        .route("/api/v1/auth/password", put(self::put::change_password))
        .route(
            "/api/v1/auth/check_login_status",
            get(self::get::check_login_status),
        )
        .route_layer(login_required!(Backend));
    Router::new()
        .merge(r)
        .route("/api/v1/auth/login", post(self::post::login))
        .route("/api/v1/auth/logout", get(self::get::logout))
}

mod put {
    use axum::Json;
    use axum_login::AuthUser;
    use easytier::proto::common::Void;

    use crate::restful::{HttpHandleError, users::ChangePassword};

    use super::*;

    pub async fn change_password(
        mut auth_session: AuthSession,
        Json(req): Json<ChangePassword>,
    ) -> Result<Json<Void>, HttpHandleError> {
        let user_id = auth_session
            .user
            .as_ref()
            .ok_or((
                StatusCode::UNAUTHORIZED,
                Json::from(crate::restful::other_error_with_code(
                    "Not authenticated",
                    "unauthorized",
                )),
            ))?
            .id();
        if let Err(e) = auth_session.backend.change_password(user_id, &req).await {
            tracing::error!("Failed to change password: {e:?}");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json::from(crate::restful::other_error_with_code(
                    "Failed to change password",
                    "auth_error",
                )),
            ));
        }

        let _ = auth_session.logout().await;

        Ok(Void::default().into())
    }
}

mod post {
    use axum::Json;
    use easytier::proto::common::Void;

    use crate::restful::{HttpHandleError, other_error};

    use super::*;

    pub async fn login(
        mut auth_session: AuthSession,
        Json(creds): Json<Credentials>,
    ) -> Result<Json<Void>, HttpHandleError> {
        let user = match auth_session.authenticate(creds.clone()).await {
            Ok(Some(user)) => user,
            Ok(None) => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json::from(other_error("Invalid credentials")),
                ));
            }
            Err(e) => {
                tracing::error!("Failed to authenticate: {e:?}");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json::from(crate::restful::other_error_with_code(
                        "Authentication failed",
                        "auth_error",
                    )),
                ));
            }
        };

        if let Err(e) = auth_session.login(&user).await {
            tracing::error!("Failed to create login session: {e:?}");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json::from(crate::restful::other_error_with_code(
                    "Failed to create login session",
                    "auth_error",
                )),
            ));
        }

        Ok(Void::default().into())
    }
}

mod get {
    use crate::restful::{HttpHandleError, other_error};
    use axum::Json;
    use easytier::proto::common::Void;

    use super::*;

    pub async fn logout(mut auth_session: AuthSession) -> Result<Json<Void>, HttpHandleError> {
        match auth_session.logout().await {
            Ok(_) => Ok(Json(Void::default())),
            Err(e) => {
                tracing::error!("Failed to logout: {e:?}");
                Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json::from(crate::restful::other_error_with_code(
                        "Failed to logout",
                        "auth_error",
                    )),
                ))
            }
        }
    }

    pub async fn check_login_status(
        auth_session: AuthSession,
    ) -> Result<Json<Void>, HttpHandleError> {
        if auth_session.user.is_some() {
            Ok(Json(Void::default()))
        } else {
            Err((
                StatusCode::UNAUTHORIZED,
                Json::from(other_error("Not logged in")),
            ))
        }
    }
}
