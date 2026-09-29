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

    use crate::restful::{HttpHandleError, other_error, users::ChangePassword};

    use super::*;

    pub async fn change_password(
        mut auth_session: AuthSession,
        Json(req): Json<ChangePassword>,
    ) -> Result<Json<Void>, HttpHandleError> {
        if let Err(e) = auth_session
            .backend
            .change_password(auth_session.user.as_ref().unwrap().id(), &req)
            .await
        {
            tracing::error!("Failed to change password: {:?}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json::from(other_error(format!("{:?}", e))),
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
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json::from(other_error(format!("{:?}", e))),
                ));
            }
        };

        if let Err(e) = auth_session.login(&user).await {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json::from(other_error(format!("{:?}", e))),
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
                tracing::error!("Failed to logout: {:?}", e);
                Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json::from(other_error(format!("{:?}", e))),
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
