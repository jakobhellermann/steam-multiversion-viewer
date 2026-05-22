//! Small response helpers shared across routes.

use axum::http::{HeaderValue, header};
use axum::response::{IntoResponseParts, ResponseParts};

/// Sets `Cache-Control: public, max-age=31536000, immutable`.
pub struct ImmutableCache;

impl IntoResponseParts for ImmutableCache {
    type Error = std::convert::Infallible;

    fn into_response_parts(self, mut res: ResponseParts) -> Result<ResponseParts, Self::Error> {
        res.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
        Ok(res)
    }
}
