use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub struct AppError(pub anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = serde_json::to_string(&json!({
            "error": self.0.to_string()
        }))
        .unwrap();

        (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "application/json")],
            body,
        )
            .into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
