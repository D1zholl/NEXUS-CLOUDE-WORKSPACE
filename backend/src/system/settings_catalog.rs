//! Реестр системных настроек (PLAN §9.1).
//!
//! Единый источник истины: какие ключи существуют, какого они типа, какие
//! границы допустимы, требуется ли перезапуск и какое право нужно для изменения.
//! `PUT /system/settings/{key}` валидирует значение только по этому реестру.

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Int,
    Bool,
    String,
    Enum(&'static [&'static str]),
    Json,
}

impl Kind {
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Int => "int",
            Kind::Bool => "bool",
            Kind::String => "string",
            Kind::Enum(_) => "enum",
            Kind::Json => "json",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SettingSpec {
    pub key: &'static str,
    pub kind: Kind,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub requires_restart: bool,
    pub permission: &'static str,
    pub label: &'static str,
}

const MANAGE: &str = "system.settings.manage";

const QUALITY: &[&str] = &["auto", "high", "eco"];

pub const SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        key: "limits.max_upload_mb",
        kind: Kind::Int,
        min: Some(1),
        max: Some(102_400),
        requires_restart: false,
        permission: MANAGE,
        label: "Максимальный размер файла, МБ",
    },
    SettingSpec {
        key: "limits.chunk_max_mb",
        kind: Kind::Int,
        min: Some(1),
        max: Some(256),
        requires_restart: false,
        permission: MANAGE,
        label: "Размер чанка загрузки, МБ",
    },
    SettingSpec {
        key: "limits.upload_ttl_hours",
        kind: Kind::Int,
        min: Some(1),
        max: Some(720),
        requires_restart: false,
        permission: MANAGE,
        label: "Срок жизни незавершённой загрузки, ч",
    },
    SettingSpec {
        key: "limits.share_ttl_hours",
        kind: Kind::Int,
        min: Some(1),
        max: Some(8_760),
        requires_restart: false,
        permission: MANAGE,
        label: "Срок действия ссылки по умолчанию, ч",
    },
    SettingSpec {
        key: "limits.export_ttl_hours",
        kind: Kind::Int,
        min: Some(1),
        max: Some(720),
        requires_restart: false,
        permission: MANAGE,
        label: "Срок хранения ZIP-архива, ч",
    },
    SettingSpec {
        key: "limits.trash_retention_days",
        kind: Kind::Int,
        min: Some(1),
        max: Some(3_650),
        requires_restart: false,
        permission: MANAGE,
        label: "Срок хранения корзины, дней",
    },
    SettingSpec {
        key: "limits.ai_workspace_retention_days",
        kind: Kind::Int,
        min: Some(1),
        max: Some(3_650),
        requires_restart: false,
        permission: MANAGE,
        label: "Срок хранения удалённых AI workspace, дней",
    },
    SettingSpec {
        key: "limits.quota_cloud_gb",
        kind: Kind::Int,
        min: Some(1),
        max: Some(1_000_000),
        requires_restart: false,
        permission: MANAGE,
        label: "Квота Cloud по умолчанию, ГБ",
    },
    SettingSpec {
        key: "limits.quota_minecraft_gb",
        kind: Kind::Int,
        min: Some(1),
        max: Some(1_000_000),
        requires_restart: false,
        permission: MANAGE,
        label: "Квота Minecraft по умолчанию, ГБ",
    },
    SettingSpec {
        key: "jobs.max_concurrent",
        kind: Kind::Int,
        min: Some(1),
        max: Some(8),
        requires_restart: false,
        permission: MANAGE,
        label: "Лимит параллельных задач",
    },
    SettingSpec {
        key: "cloud.upload.concurrency",
        kind: Kind::Int,
        min: Some(1),
        max: Some(6),
        requires_restart: false,
        permission: MANAGE,
        label: "Параллельные загрузки в браузере",
    },
    SettingSpec {
        key: "security.session_ttl_hours",
        kind: Kind::Int,
        min: Some(1),
        max: Some(720),
        requires_restart: false,
        permission: MANAGE,
        label: "Срок жизни сессии, ч",
    },
    SettingSpec {
        key: "security.login_max_failures",
        kind: Kind::Int,
        min: Some(3),
        max: Some(50),
        requires_restart: false,
        permission: MANAGE,
        label: "Неудачных попыток входа до блокировки",
    },
    SettingSpec {
        key: "security.password_min_length",
        kind: Kind::Int,
        min: Some(crate::auth::password::PASSWORD_MIN_ALLOWED as i64),
        max: Some(crate::auth::password::PASSWORD_MAX_ALLOWED as i64),
        requires_restart: false,
        permission: MANAGE,
        label: "Минимальная длина пароля",
    },
    SettingSpec {
        key: "network.allowed_origins",
        kind: Kind::Json,
        min: None,
        max: None,
        requires_restart: true,
        permission: MANAGE,
        label: "Разрешённые источники (CORS)",
    },
    SettingSpec {
        key: "network.trust_proxy",
        kind: Kind::Bool,
        min: None,
        max: None,
        requires_restart: true,
        permission: MANAGE,
        label: "Доверять заголовкам прокси",
    },
    SettingSpec {
        key: "appearance.background.quality",
        kind: Kind::Enum(QUALITY),
        min: None,
        max: None,
        requires_restart: false,
        permission: MANAGE,
        label: "Качество 3D-фона по умолчанию",
    },
    SettingSpec {
        key: "appearance.background.defaults",
        kind: Kind::Json,
        min: None,
        max: None,
        requires_restart: false,
        permission: MANAGE,
        label: "Параметры 3D-фона по умолчанию",
    },
];

pub fn find(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|spec| spec.key == key)
}

pub fn all_keys() -> Vec<&'static str> {
    SETTINGS.iter().map(|spec| spec.key).collect()
}

/// Описывает реестр для UI.
pub fn catalog_json() -> Value {
    json!(
        SETTINGS
            .iter()
            .map(|spec| json!({
                "key": spec.key,
                "kind": spec.kind.label(),
                "min": spec.min,
                "max": spec.max,
                "requiresRestart": spec.requires_restart,
                "permission": spec.permission,
                "label": spec.label,
                "options": match spec.kind {
                    Kind::Enum(values) => json!(values),
                    _ => Value::Null,
                },
            }))
            .collect::<Vec<_>>()
    )
}

/// Приводит значение к типу, проверяет границы и допустимые варианты.
pub fn validate(key: &str, value: Value) -> Result<Value, String> {
    let spec = find(key).ok_or_else(|| {
        format!(
            "Неизвестный ключ настройки «{key}». Допустимые: {}.",
            all_keys().join(", ")
        )
    })?;

    let normalized = match spec.kind {
        Kind::Int => {
            let number = value
                .as_i64()
                .or_else(|| value.as_f64().map(|v| v.trunc() as i64))
                .ok_or_else(|| "Ожидается целое число.".to_owned())?;
            if let Some(min) = spec.min {
                if number < min {
                    return Err(format!("Значение должно быть не меньше {min}."));
                }
            }
            if let Some(max) = spec.max {
                if number > max {
                    return Err(format!("Значение должно быть не больше {max}."));
                }
            }
            Value::from(number)
        }
        Kind::Bool => Value::Bool(
            value
                .as_bool()
                .ok_or_else(|| "Ожидается true или false.".to_owned())?,
        ),
        Kind::String => {
            let text = value
                .as_str()
                .ok_or_else(|| "Ожидается строка.".to_owned())?
                .trim()
                .to_owned();
            if text.chars().count() > 512 {
                return Err("Строка слишком длинная (максимум 512 символов).".to_owned());
            }
            Value::String(text)
        }
        Kind::Enum(options) => {
            let text = value
                .as_str()
                .ok_or_else(|| "Ожидается строка.".to_owned())?
                .trim()
                .to_owned();
            if !options.contains(&text.as_str()) {
                return Err(format!("Допустимые значения: {}.", options.join(", ")));
            }
            Value::String(text)
        }
        Kind::Json => {
            if !value.is_array() && !value.is_object() {
                return Err("Ожидается массив или объект JSON.".to_owned());
            }
            value
        }
    };

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_each_kind() {
        assert_eq!(
            validate("limits.max_upload_mb", json!(100)).unwrap(),
            json!(100)
        );
        assert!(validate("limits.max_upload_mb", json!(0)).is_err());
        assert!(validate("limits.max_upload_mb", json!(10_u64.pow(9))).is_err());
        assert!(validate("limits.max_upload_mb", json!("abc")).is_err());
        assert!(validate("network.trust_proxy", json!(true)).is_ok());
        assert!(validate("network.trust_proxy", json!("yes")).is_err());
        assert!(validate("network.allowed_origins", json!(["https://a"])).is_ok());
        assert!(validate("network.allowed_origins", json!("https://a")).is_err());
        assert!(validate("appearance.background.quality", json!("eco")).is_ok());
        assert!(validate("appearance.background.quality", json!("ultra")).is_err());
    }

    #[test]
    fn unknown_key_is_rejected() {
        let error = validate("nope.key", json!(1)).unwrap_err();
        assert!(error.contains("Неизвестный ключ"));
    }

    #[test]
    fn catalog_is_complete() {
        let catalog = catalog_json();
        assert_eq!(catalog.as_array().unwrap().len(), SETTINGS.len());
        assert!(find("limits.chunk_max_mb").is_some());
        assert!(find("security.session_ttl_hours").is_some());
    }
}
