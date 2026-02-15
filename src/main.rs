mod error;
mod handlers;
mod session;
mod types;

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use log::info;
use tower_http::cors::CorsLayer;

use handlers::AppState;

async fn auth_middleware(
    req: Request,
    next: Next,
) -> Result<impl IntoResponse, StatusCode> {
    let api_key = std::env::var("IMESSAGE_API_KEY").unwrap_or_default();

    if api_key.is_empty() {
        return Ok(next.run(req).await);
    }

    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = auth_header.strip_prefix("Bearer ").unwrap_or(auth_header);

    if token == api_key {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init_timed();

    let default_data_dir =
        "/home/opc/.var/app/app.openbubbles.OpenBubbles/data/bluebubbles".to_string();
    let data_dir = std::env::var("IMESSAGE_DATA_DIR").unwrap_or(default_data_dir);
    let port: u16 = std::env::var("IMESSAGE_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);

    info!("Data dir: {}", data_dir);
    info!("Restoring session...");

    let (client, _conn, mut aps_receiver) = session::restore(&data_dir).await?;

    // Background APS pump: drain incoming messages to keep the connection alive
    tokio::spawn(async move {
        loop {
            match aps_receiver.recv().await {
                Ok(_msg) => {
                    log::debug!("APS message received (draining)");
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    log::warn!("APS receiver lagged by {} messages", n);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    log::error!("APS channel closed");
                    break;
                }
            }
        }
    });

    let state = Arc::new(AppState { client });

    let app = Router::new()
        .route("/api/send", post(handlers::send_message))
        .route("/api/handles", get(handlers::get_handles))
        .route("/api/health", get(handlers::health))
        .layer(middleware::from_fn(auth_middleware))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("Starting server on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
