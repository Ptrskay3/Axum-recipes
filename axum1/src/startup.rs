use crate::{
    queue::email::EmailClient,
    routes::{admin_router, auth_router, ingredient_router, recipe_router},
    session::SessionLayer,
    utils::{oauth_client_discord, oauth_client_google, shutdown_signal},
};
use anyhow::Context;
use async_redis_session::RedisSessionStore;
use axum::{http::HeaderValue, response::IntoResponse, routing::get_service, Extension, Router};
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use tower_http::{cors::CorsLayer, services::ServeDir, trace::TraceLayer};

pub async fn application() -> Result<(), anyhow::Error> {
    dotenv::dotenv().ok();

    let config = crate::config::get_config().expect("Configuration file is missing");

    let addr = SocketAddr::from(([127, 0, 0, 1], config.application_port));

    let discord_oauth_client = oauth_client_discord(&config);
    let google_oauth_client = oauth_client_google(&config);

    let db_conn_str = config.database.connection_string();

    let db_pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .connect(&db_conn_str)
        .await
        .context("failed to connect to database")?;

    sqlx::migrate!().run(&db_pool).await?;

    let redis_conn_str = config.redis.connection_string();

    let store =
        RedisSessionStore::new(redis_conn_str.as_ref()).context("failed to connect redis")?;

    if let Some(sentry_dsn) = config.sentry_dsn {
        let _guard = sentry::init((
            sentry_dsn,
            sentry::ClientOptions {
                release: sentry::release_name!(),
                ..Default::default()
            },
        ));
    }

    let email_client = EmailClient::from_config(config.email_client);

    let app = Router::new()
        .nest("/i", ingredient_router())
        .nest("/r", recipe_router())
        .nest("/", auth_router())
        .nest("/admin", admin_router())
        .fallback(get_service(ServeDir::new("./static")).handle_error(handle_asset_error))
        // It's a little better use the `tower::ServiceBuilder` to avoid unnecessary boxing,
        // and maybe we can use
        // https://docs.rs/tower-http/latest/tower_http/trait.ServiceBuilderExt.html
        // in the future.
        .layer(
            tower::ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(Extension(db_pool))
                .layer(Extension(store.clone()))
                .layer(
                    SessionLayer::new(store, config.redis.secret_key.as_bytes()).with_secure(
                        std::env::var("APP_ENVIRONMENT").unwrap_or_else(|_| String::from("local"))
                            == "production",
                    ),
                )
                .layer(Extension(email_client.clone()))
                .layer(Extension(discord_oauth_client))
                .layer(Extension(google_oauth_client))
                .layer(
                    CorsLayer::very_permissive()
                        .allow_origin(config.frontend_url.parse::<HeaderValue>().unwrap())
                        .allow_credentials(true),
                ),
        );

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("Failed to start server")
}

async fn handle_asset_error(_err: std::io::Error) -> impl IntoResponse {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "something went wrong",
    )
}
