use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{self, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

static TOKEN_SECRET_KEY: Lazy<Arc<[u8; 32]>> = Lazy::new(|| Arc::new(rand::random()));

#[derive(Serialize, Deserialize, Clone)]
pub struct Token {
    pub sub: u64,
    iat: i64,
    exp: i64,
    sign: [u8; 32],
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match String::try_from(self) {
            Ok(s) => write!(f, "Bearer {s}"),
            Err(_) => write!(f, "Bearer [invalid]"),
        }
    }
}

impl TryFrom<&str> for Token {
    type Error = ();
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        STANDARD
            .decode(s)
            .map_err(|_| ())
            .and_then(|bytes| rmp_serde::from_slice(&bytes).map_err(|_| ()))
    }
}

impl TryFrom<&HeaderMap> for Token {
    type Error = ();
    fn try_from(headers: &HeaderMap) -> Result<Self, Self::Error> {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map_or(Err(()), |s| Self::try_from(s))
    }
}

impl TryFrom<&Token> for String {
    type Error = ();
    fn try_from(token: &Token) -> Result<Self, Self::Error> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        rmp_serde::to_vec(token)
            .map(|bytes| STANDARD.encode(bytes))
            .map_err(|_| ())
    }
}

impl TryFrom<&Token> for HeaderValue {
    type Error = ();
    fn try_from(token: &Token) -> Result<Self, Self::Error> {
        String::try_from(token)
            .and_then(|s| HeaderValue::try_from(&format!("Bearer {s}")).map_err(|_| ()))
    }
}

impl IntoResponse for Token {
    fn into_response(self) -> Response<Body> {
        match HeaderValue::try_from(&self) {
            Ok(head) => (StatusCode::OK, [(header::AUTHORIZATION, head)]).into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

impl Token {
    pub fn signature(secret_key: &[u8; 32], sub: u64, iat: i64, exp: i64) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(secret_key);
        hasher.update(sub.to_ne_bytes());
        hasher.update(iat.to_ne_bytes());
        hasher.update(exp.to_ne_bytes());
        hasher.finalize().into()
    }

    pub fn new(sub: u64) -> Self {
        let iat = time::UtcDateTime::now().unix_timestamp();
        let exp = iat + 2_000_000;
        Self {
            sign: Self::signature(&TOKEN_SECRET_KEY, sub, iat, exp),
            sub,
            iat,
            exp,
        }
    }

    pub fn is_valid(&self) -> bool {
        let now = time::UtcDateTime::now().unix_timestamp();
        self.sign == Self::signature(&TOKEN_SECRET_KEY, self.sub, self.iat, self.exp)
            && (self.iat - now) + (self.exp - now) > 0
    }

    pub fn update(self) -> Option<Self> {
        self.is_valid().then(|| {
            let iat = time::UtcDateTime::now().unix_timestamp();
            Self {
                sign: Self::signature(&TOKEN_SECRET_KEY, self.sub, iat, self.exp),
                sub: self.sub,
                iat,
                exp: self.exp,
            }
        })
    }
}

pub struct OptionalToken(Option<Token>);

impl OptionalToken {
    pub fn auth(&self) -> Result<u64, (StatusCode, &'static str)> {
        self.get_sub()
            .ok_or((StatusCode::UNAUTHORIZED, "Login required"))
    }

    pub fn get_sub(&self) -> Option<u64> {
        self.as_ref().map(|t| t.sub)
    }

    pub fn as_ref(&self) -> Option<&Token> {
        self.0.as_ref()
    }
}

pub async fn token_middleware(mut request: Request, next: Next) -> Response {
    let token = Arc::new(Mutex::new(OptionalToken(
        Token::try_from(request.headers())
            .ok()
            .and_then(|t| t.update()),
    )));
    request.extensions_mut().insert(token.clone());
    let mut response = next.run(request).await;

    if let Some(header) = token
        .lock()
        .await
        .as_ref()
        .and_then(|t| HeaderValue::try_from(t).ok())
    {
        response
            .headers_mut()
            .entry(header::AUTHORIZATION)
            .or_insert(header);
    }
    response
}
