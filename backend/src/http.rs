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

/// Sets `Cache-Control: private, max-age=<secs>, must-revalidate`. Browser
/// serves from its own cache for `secs` seconds, then revalidates on next
/// request. `private` because most of our data is per-account.
pub struct CacheSeconds(pub u32);

impl IntoResponseParts for CacheSeconds {
    type Error = std::convert::Infallible;

    fn into_response_parts(self, mut res: ResponseParts) -> Result<ResponseParts, Self::Error> {
        let v = format!("private, max-age={}, must-revalidate", self.0);
        let header = HeaderValue::from_str(&v).expect("cache-control value should always be ascii");
        res.headers_mut().insert(header::CACHE_CONTROL, header);
        Ok(res)
    }
}
