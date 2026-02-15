use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct SendRequest {
    pub to: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct SendResponse {
    pub success: bool,
    pub message_id: String,
}

#[derive(Serialize)]
pub struct HandlesResponse {
    pub handles: Vec<String>,
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
}

