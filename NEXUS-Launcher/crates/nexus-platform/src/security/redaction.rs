use serde_json::{Map, Value};

const REDACTED: &str = "[redacted]";

/// Field names that must never be written to logs or `activity_logs`.
pub fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', ' '], "_");
    key.contains("token")
        || key.contains("password")
        || key.contains("passwd")
        || key.contains("secret")
        || key.contains("authorization")
        || key.contains("cookie")
        || key.contains("api_key")
        || key.contains("apikey")
        || key.contains("xsts")
}

/// Recursively replaces sensitive values with `[redacted]`.
pub fn sanitize_details(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut output = Map::with_capacity(map.len());
            for (key, value) in map {
                if is_sensitive_key(key) {
                    output.insert(key.clone(), Value::String(REDACTED.to_owned()));
                } else {
                    output.insert(key.clone(), sanitize_details(value));
                }
            }
            Value::Object(output)
        }
        Value::Array(values) => Value::Array(values.iter().map(sanitize_details).collect()),
        other => other.clone(),
    }
}

/// Minimal text redaction for diagnostics: masks common `key=value` pairs.
pub fn redact_text(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for token in input.split_inclusive(char::is_whitespace) {
        let trimmed = token.trim_end_matches(char::is_whitespace);
        let suffix = &token[trimmed.len()..];
        let lower = trimmed.to_ascii_lowercase();
        let sensitive = lower.contains("access_token=")
            || lower.contains("refresh_token=")
            || lower.contains("password=")
            || lower.contains("authorization=")
            || lower.contains("secret=");
        if sensitive {
            output.push_str(REDACTED);
        } else {
            output.push_str(trimmed);
        }
        output.push_str(suffix);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sensitive_fields_are_removed_recursively() {
        let value = json!({
            "account": { "username": "Player", "refresh_token": "abc" },
            "items": [{ "accessToken": "xyz", "name": "mod" }],
            "password": "qwerty",
            "note": "safe"
        });
        let sanitized = sanitize_details(&value);
        assert_eq!(sanitized["account"]["refresh_token"], "[redacted]");
        assert_eq!(sanitized["account"]["username"], "Player");
        assert_eq!(sanitized["items"][0]["accessToken"], "[redacted]");
        assert_eq!(sanitized["password"], "[redacted]");
        assert_eq!(sanitized["note"], "safe");
        assert!(!sanitized.to_string().contains("abc"));
        assert!(!sanitized.to_string().contains("qwerty"));
    }

    #[test]
    fn text_redaction_masks_common_pairs() {
        let text = "GET /api access_token=abc123 password=qwerty normal=value";
        let redacted = redact_text(text);
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("qwerty"));
        assert!(redacted.contains("normal=value"));
        assert!(redacted.contains("[redacted]"));
    }
}
