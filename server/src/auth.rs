use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;
const MAX_AGE_SECONDS: i64 = 300;

#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    #[error("webhook secret is not configured")]
    MissingSecret,
    #[error("webhook signature is missing or malformed")]
    Malformed,
    #[error("webhook timestamp is stale")]
    Stale,
    #[error("webhook signature is invalid")]
    Invalid,
}

pub fn verify_monitor(
    body: &[u8],
    timestamp: Option<&str>,
    signature: Option<&str>,
    secret: Option<&str>,
) -> Result<(), SignatureError> {
    let timestamp = timestamp.ok_or(SignatureError::Malformed)?;
    let signature = signature
        .and_then(|value| value.strip_prefix("sha256="))
        .ok_or(SignatureError::Malformed)?;
    verify(body, timestamp, signature, secret)
}

pub fn verify_elevenlabs(
    body: &[u8],
    signature: Option<&str>,
    secret: Option<&str>,
) -> Result<(), SignatureError> {
    let mut timestamp = None;
    let mut digest = None;
    for part in signature.ok_or(SignatureError::Malformed)?.split(',') {
        let (key, value) = part.split_once('=').ok_or(SignatureError::Malformed)?;
        match key.trim() {
            "t" => timestamp = Some(value.trim()),
            "v0" => digest = Some(value.trim()),
            _ => {}
        }
    }
    verify(
        body,
        timestamp.ok_or(SignatureError::Malformed)?,
        digest.ok_or(SignatureError::Malformed)?,
        secret,
    )
}

fn verify(
    body: &[u8],
    timestamp: &str,
    signature: &str,
    secret: Option<&str>,
) -> Result<(), SignatureError> {
    let secret = secret.ok_or(SignatureError::MissingSecret)?;
    let unix: i64 = timestamp.parse().map_err(|_| SignatureError::Malformed)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SignatureError::Stale)?
        .as_secs() as i64;
    if (now - unix).abs() > MAX_AGE_SECONDS {
        return Err(SignatureError::Stale);
    }
    let supplied = hex::decode(signature).map_err(|_| SignatureError::Malformed)?;
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| SignatureError::Invalid)?;
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.verify_slice(&supplied)
        .map_err(|_| SignatureError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(body: &[u8], timestamp: &str, secret: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn accepts_elevenlabs_signature_shape() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();
        let body = br#"{"type":"post_call_transcription"}"#;
        let header = format!("t={now},v0={}", sign(body, &now, "secret"));
        assert!(verify_elevenlabs(body, Some(&header), Some("secret")).is_ok());
    }

    #[test]
    fn rejects_changed_body() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();
        let header = format!("t={now},v0={}", sign(b"original", &now, "secret"));
        assert!(matches!(
            verify_elevenlabs(b"changed", Some(&header), Some("secret")),
            Err(SignatureError::Invalid)
        ));
    }
}
