use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use log::info;
use rustpush::{ConversationData, IMClient, Message, MessageInst, MessageType, NormalMessage};

use crate::error::AppError;
use crate::types::{HandlesResponse, HealthResponse, SendRequest, SendResponse};

pub struct AppState {
    pub client: Arc<IMClient>,
}

fn format_phone(number: &str) -> String {
    let digits: String = number.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 10 {
        format!("tel:+1{}", digits)
    } else if digits.len() == 11 && digits.starts_with('1') {
        format!("tel:+{}", digits)
    } else if number.starts_with("tel:") {
        number.to_string()
    } else if number.starts_with('+') {
        format!("tel:{}", number)
    } else {
        format!("tel:+{}", digits)
    }
}

pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendRequest>,
) -> Result<impl IntoResponse, AppError> {
    let handles = state.client.identity.get_handles().await;
    let sender = handles
        .first()
        .ok_or_else(|| anyhow::anyhow!("No registered handles"))?
        .clone();

    let to = format_phone(&req.to);
    info!("Sending message to {} (formatted: {}) from {}", req.to, to, sender);

    let conversation = ConversationData {
        participants: vec![sender.clone(), to],
        cv_name: None,
        sender_guid: None,
        after_guid: None,
    };

    let normal = NormalMessage::new(req.message.clone(), MessageType::IMessage);
    let mut msg = MessageInst::new(conversation, &sender, Message::Message(normal));
    let message_id = msg.id.clone();

    let result = state.client.send(&mut msg).await?;

    if let Some(handle) = result.handle {
        let uuid = message_id.clone();
        tokio::spawn(async move {
            match handle.await {
                Ok(Ok(())) => info!("Message {} delivered", uuid),
                Ok(Err(e)) => log::warn!("Message {} delivery error: {}", uuid, e),
                Err(e) => log::warn!("Message {} join error: {}", uuid, e),
            }
        });
    }

    Ok((
        StatusCode::OK,
        Json(SendResponse {
            success: true,
            message_id,
        }),
    ))
}

pub async fn get_handles(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let handles = state.client.identity.get_handles().await.to_vec();
    Ok(Json(HandlesResponse { handles }))
}

pub async fn health(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let handles = state.client.identity.get_handles().await;
    let status = if handles.is_empty() {
        "no_handles"
    } else {
        "ok"
    };
    Ok(Json(HealthResponse {
        status: status.to_string(),
    }))
}
