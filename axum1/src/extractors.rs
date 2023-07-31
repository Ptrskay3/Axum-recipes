use std::{convert::Infallible, ops::Deref};

use crate::{config::ApplicationSettings, error::ApiError, state::AppState};
use async_redis_session::RedisSessionStore;
use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
    Extension,
};
use sqlx::{pool, Postgres};

pub struct DatabaseConnection(pub pool::PoolConnection<Postgres>);

#[async_trait]
impl<S> FromRequestParts<S> for DatabaseConnection
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(_parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AppState { db_pool, .. } = AppState::from_ref(state);
        let conn = db_pool.acquire().await?;
        Ok(Self(conn))
    }
}

pub struct RedisConnection(pub RedisSessionStore);

#[async_trait]
impl<S> FromRequestParts<S> for RedisConnection
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Infallible;

    async fn from_request_parts(_parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AppState { redis_store, .. } = AppState::from_ref(state);

        Ok(Self(redis_store))
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy)]
pub struct AuthUser(uuid::Uuid);

impl AuthUser {
    fn new(uuid: uuid::Uuid) -> Self {
        Self(uuid)
    }
}

impl Deref for AuthUser {
    type Target = uuid::Uuid;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Extension(session) =
            Extension::<crate::session_ext::Session>::from_request_parts(parts, state)
                .await
                .expect("`SessionLayer` should be added");

        session
            .get::<uuid::Uuid>("user_id")
            .map(Self::new)
            .ok_or(ApiError::Unauthorized)
    }
}

pub struct MaybeAuthUser(pub Option<AuthUser>);

impl MaybeAuthUser {
    pub fn into_inner(self) -> Option<AuthUser> {
        self.0
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for MaybeAuthUser
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Extension(session) =
            Extension::<crate::session_ext::Session>::from_request_parts(parts, state)
                .await
                .expect("`SessionLayer` should be added");

        let user_id = session.get::<uuid::Uuid>("user_id");

        match user_id {
            Some(id) => Ok(Self(Some(AuthUser::new(id)))),
            None => Ok(Self(None)),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Uploader {
    pub id: uuid::Uuid,
    pub bytes_limit: i64,
}

#[async_trait]
impl<S> FromRequestParts<S> for Uploader
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Extension(session) =
            Extension::<crate::session_ext::Session>::from_request_parts(parts, state)
                .await
                .expect("`SessionLayer` should be added");

        let AppState {
            db_pool,
            config:
                ApplicationSettings {
                    daily_upload_limit_bytes,
                    ..
                },
            ..
        } = AppState::from_ref(state);

        let mut db = db_pool.acquire().await?;

        let user_id = session
            .get::<uuid::Uuid>("user_id")
            .ok_or(ApiError::Unauthorized)?;

        let bytes_limit = sqlx::query!(
            "SELECT COALESCE(SUM(bytes), 0) as upload_limit FROM uploads
            WHERE uploader_id = $1 AND created_at > current_timestamp - INTERVAL '1 days';",
            user_id
        )
        .fetch_one(&mut *db)
        .await?
        .upload_limit
        .unwrap_or(daily_upload_limit_bytes);

        if bytes_limit < daily_upload_limit_bytes {
            Ok(Self {
                bytes_limit,
                id: user_id,
            })
        } else {
            Err(ApiError::Forbidden)
        }
    }
}
