use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // 5xx are bugs / surprises — log the message so the per-request
        // `request route=… status=500` line in the access log has a
        // companion entry telling you *why* it failed. 4xx are routine
        // (404 for missing files etc) so we stay quiet there.
        if self.status.is_server_error() {
            tracing::error!(
                status = self.status.as_u16(),
                message = %self.message,
                "api error response",
            );
        }
        (
            self.status,
            Json(ErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

impl<E: std::error::Error> From<E> for ApiError {
    fn from(err: E) -> Self {
        tracing::error!(error = %err, source = ?source_chain(&err), "request failed");
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.to_string(),
        }
    }
}

fn source_chain(err: &dyn std::error::Error) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = err.source();
    while let Some(e) = cur {
        out.push(e.to_string());
        cur = e.source();
    }
    out
}
