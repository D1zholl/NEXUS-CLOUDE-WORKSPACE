//! Короткоживущие тикеты скачивания публичных ссылок.
//!
//! Тикет заменяет передачу пароля в URL: пароль проверяется один раз в
//! `POST /public/shares/{token}/unlock`, после чего клиент скачивает файл по
//! подписанному тикету. Тикет привязан к текущему `password_hash` ссылки,
//! поэтому смена или удаление пароля автоматически инвалидирует старые тикеты.
//!
//! Формат: `base64url(payload).base64url(hmac)`, где payload — 16 байт UUID
//! ссылки и 8 байт срока действия (unix timestamp, big-endian).

use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// Время жизни тикета скачивания.
pub const SHARE_TICKET_TTL_SECS: i64 = 60 * 60;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareTicket {
    pub share_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

type HmacSha256 = Hmac<Sha256>;

fn b64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        }
    }
    out
}

fn b64url_decode(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some((byte - b'A') as u32),
            b'a'..=b'z' => Some((byte - b'a') as u32 + 26),
            b'0'..=b'9' => Some((byte - b'0') as u32 + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let chunk_len = (bytes.len() - index).min(4);
        let mut buffer = [0u32; 4];
        for offset in 0..chunk_len {
            buffer[offset] = value(bytes[index + offset])?;
        }
        let triple = (buffer[0] << 18) | (buffer[1] << 12) | (buffer[2] << 6) | buffer[3];
        out.push(((triple >> 16) & 0xff) as u8);
        if chunk_len > 2 {
            out.push(((triple >> 8) & 0xff) as u8);
        }
        if chunk_len > 3 {
            out.push((triple & 0xff) as u8);
        }
        index += chunk_len;
    }
    Some(out)
}

fn sign(secret: &str, payload: &[u8], password_hash: Option<&str>) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC принимает ключ любой длины");
    mac.update(b"nexus-share-ticket:v1:");
    mac.update(payload);
    mac.update(b"|");
    mac.update(password_hash.unwrap_or("").as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Выпускает тикет для ссылки. `password_hash` — текущий хэш пароля ссылки
/// (`None`, если пароль не задан).
pub fn issue_share_ticket(
    secret: &str,
    share_id: Uuid,
    password_hash: Option<&str>,
) -> (String, DateTime<Utc>) {
    let expires_at = Utc::now() + Duration::seconds(SHARE_TICKET_TTL_SECS);
    let mut payload = Vec::with_capacity(24);
    payload.extend_from_slice(share_id.as_bytes());
    payload.extend_from_slice(&expires_at.timestamp().to_be_bytes());
    let signature = sign(secret, &payload, password_hash);
    (
        format!("{}.{}", b64url_encode(&payload), b64url_encode(&signature)),
        expires_at,
    )
}

/// Проверяет тикет: подпись, срок действия и привязку к паролю.
pub fn verify_share_ticket(
    secret: &str,
    ticket: &str,
    password_hash: Option<&str>,
) -> Option<ShareTicket> {
    let (payload_text, signature_text) = ticket.split_once('.')?;
    if signature_text.contains('.') {
        return None;
    }
    let payload = b64url_decode(payload_text)?;
    if payload.len() != 24 {
        return None;
    }
    let signature = b64url_decode(signature_text)?;
    let expected = sign(secret, &payload, password_hash);
    if signature.len() != expected.len() || !bool::from(signature.ct_eq(&expected)) {
        return None;
    }
    let share_id = Uuid::from_slice(&payload[..16]).ok()?;
    let expires = i64::from_be_bytes(payload[16..24].try_into().ok()?);
    if expires <= Utc::now().timestamp() {
        return None;
    }
    Some(ShareTicket {
        share_id,
        expires_at: DateTime::from_timestamp(expires, 0)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_roundtrip_binds_password_hash() {
        let secret = "test-secret-test-secret-test-secret";
        let id = Uuid::new_v4();
        let (ticket, _) = issue_share_ticket(secret, id, Some("hash-a"));
        let parsed = verify_share_ticket(secret, &ticket, Some("hash-a")).expect("тикет валиден");
        assert_eq!(parsed.share_id, id);

        assert!(verify_share_ticket(secret, &ticket, Some("hash-b")).is_none());
        assert!(verify_share_ticket(secret, &ticket, None).is_none());
        assert!(
            verify_share_ticket("other-secret-other-secret", &ticket, Some("hash-a")).is_none()
        );
    }

    #[test]
    fn ticket_rejects_tampering_and_expiry() {
        let secret = "test-secret-test-secret-test-secret";
        let (ticket, _) = issue_share_ticket(secret, Uuid::new_v4(), None);
        let mut tampered = ticket.clone();
        tampered.push('A');
        assert!(verify_share_ticket(secret, &tampered, None).is_none());
        assert!(verify_share_ticket(secret, "", None).is_none());
        assert!(verify_share_ticket(secret, "abc", None).is_none());
    }

    #[test]
    fn base64url_roundtrip() {
        let data = b"\x00\x01\xfe\xffhello";
        let encoded = b64url_encode(data);
        assert!(!encoded.contains('+') && !encoded.contains('/') && !encoded.contains('='));
        assert_eq!(b64url_decode(&encoded).unwrap(), data);
    }

    #[test]
    fn expired_ticket_is_rejected() {
        let secret = "test-secret-test-secret-test-secret";
        let mut payload = Vec::new();
        payload.extend_from_slice(Uuid::new_v4().as_bytes());
        payload.extend_from_slice(&(Utc::now().timestamp() - 1).to_be_bytes());
        let signature = sign(secret, &payload, None);
        let ticket = format!("{}.{}", b64url_encode(&payload), b64url_encode(&signature));
        assert!(verify_share_ticket(secret, &ticket, None).is_none());
    }
}
