use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response, Extension};
use std::path::Path;

/// Load existing token or generate and persist a new one.
pub fn load_or_create_token(token_file: &Path) -> std::io::Result<String> {
    if let Ok(token) = std::fs::read_to_string(token_file) {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }

    let token = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = token_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(token_file, &token)?;

    // chmod 0600 on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(token_file, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(token)
}

/// Check whether `header_value` is `"Bearer <token>"` matching `expected`.
pub fn validate_bearer(header_value: &str, expected: &str) -> bool {
    header_value
        .strip_prefix("Bearer ")
        .is_some_and(|t| t == expected)
}

/// Axum middleware: reject requests without valid Bearer token.
pub async fn require_auth(
    Extension(auth): Extension<AuthToken>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let header_value = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !validate_bearer(header_value, &auth.0) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(request).await)
}

/// Extension type to carry the expected token in request state.
#[derive(Clone)]
pub struct AuthToken(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_bearer_accepts_matching_token() {
        assert!(validate_bearer("Bearer secret123", "secret123"));
    }

    #[test]
    fn validate_bearer_rejects_wrong_token() {
        assert!(!validate_bearer("Bearer wrong", "secret123"));
    }

    #[test]
    fn validate_bearer_rejects_empty_string() {
        assert!(!validate_bearer("", "secret123"));
    }

    #[test]
    fn validate_bearer_rejects_missing_prefix() {
        assert!(!validate_bearer("secret123", "secret123"));
    }

    #[test]
    fn validate_bearer_rejects_lowercase_prefix() {
        assert!(!validate_bearer("bearer secret123", "secret123"));
    }
}
