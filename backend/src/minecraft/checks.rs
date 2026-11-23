//! Проверка сборки перед публикацией.

use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::validation::path_key;
use crate::core::error::ApiResult;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub code: String,
    pub message: String,
    pub file_id: Option<Uuid>,
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub valid: bool,
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
    pub file_count: i64,
    pub total_bytes: i64,
}

fn error(code: &str, message: impl Into<String>) -> Issue {
    Issue {
        code: code.to_owned(),
        message: message.into(),
        file_id: None,
        path: None,
    }
}

fn warn(code: &str, message: impl Into<String>) -> Issue {
    error(code, message)
}

/// Выполняет полную проверку сборки.
pub async fn validate(db: &PgPool, build_id: Uuid) -> ApiResult<ValidationReport> {
    let build = super::builds::load(db, build_id).await?;
    let mut errors: Vec<Issue> = Vec::new();
    let mut warnings: Vec<Issue> = Vec::new();

    if build.name.trim().is_empty() {
        errors.push(error("NAME_EMPTY", "Не заполнено название сборки."));
    }
    if build.slug.trim().is_empty() {
        errors.push(error("SLUG_EMPTY", "Не заполнен slug сборки."));
    }
    if build.minecraft_version.trim().is_empty() {
        errors.push(error(
            "MINECRAFT_VERSION_MISSING",
            "Не выбрана версия Minecraft.",
        ));
    }
    if build.loader_type != "vanilla" && build.loader_version.trim().is_empty() {
        errors.push(error(
            "LOADER_VERSION_MISSING",
            "Не указана версия загрузчика.",
        ));
    }
    if build.java_major < 8 || build.java_major > 30 {
        errors.push(error("JAVA_INVALID", "Некорректные требования к Java."));
    }
    if build.java_min > build.java_major {
        errors.push(error(
            "JAVA_MIN_ABOVE_MAJOR",
            "Минимальная версия Java выше основной.",
        ));
    }

    // Совместимость загрузчика и версии игры.
    if !build.minecraft_version.is_empty() && build.loader_type != "vanilla" {
        let cached: Vec<String> = sqlx::query_scalar(
            "select distinct loader_version from minecraft_loader_versions
             where loader_type = $1 and minecraft_version = $2",
        )
        .bind(&build.loader_type)
        .bind(&build.minecraft_version)
        .fetch_all(db)
        .await?;
        if cached.is_empty() {
            warnings.push(warn(
                "LOADER_NOT_VERIFIED",
                "Совместимость версии загрузчика не подтверждена справочником: обновите метаданные загрузчиков.",
            ));
        } else if !cached.iter().any(|v| v == &build.loader_version) {
            errors.push(error(
                "LOADER_INCOMPATIBLE",
                format!(
                    "Загрузчик {} {} не поддерживает Minecraft {}.",
                    build.loader_type, build.loader_version, build.minecraft_version
                ),
            ));
        }
    }

    #[derive(sqlx::FromRow)]
    struct FileCheckRow {
        id: Uuid,
        path: String,
        side: String,
        required: bool,
        enabled: bool,
        object_id: Option<Uuid>,
        size_bytes: i64,
        sha256: String,
        object_size: Option<i64>,
        object_sha: Option<String>,
        state: Option<String>,
    }

    let rows: Vec<FileCheckRow> = sqlx::query_as(
        "select f.id, f.path, f.side, f.required, f.enabled, f.object_id, f.size_bytes,
                f.sha256, o.size_bytes as object_size, o.sha256 as object_sha, o.state
         from minecraft_build_files f
         left join storage_objects o on o.id = f.object_id
         where f.build_id = $1 and f.kind = 'file'
         order by lower(f.path)",
    )
    .bind(build_id)
    .fetch_all(db)
    .await?;

    if rows.is_empty() {
        errors.push(error("FILES_EMPTY", "В сборке нет ни одного файла."));
    }

    let mut seen: Vec<(String, String)> = Vec::new();
    let mut total_bytes = 0i64;
    let mut required_files = 0i64;

    for FileCheckRow {
        id,
        path,
        side,
        required,
        enabled,
        object_id,
        size_bytes: size,
        sha256,
        object_size,
        object_sha,
        state,
    } in &rows
    {
        if *enabled {
            total_bytes += size;
        }
        if *required {
            required_files += 1;
        }
        let key = path_key(path);
        if let Some((_, other)) = seen.iter().find(|(existing, _)| existing == &key) {
            errors.push(Issue {
                code: "PATH_CONFLICT".to_owned(),
                message: format!("Путь «{path}» дублирует «{other}»."),
                file_id: Some(*id),
                path: Some(path.clone()),
            });
        } else {
            seen.push((key, path.clone()));
        }
        if path.split('/').any(|segment| segment == "..") || path.starts_with('/') {
            errors.push(Issue {
                code: "PATH_INVALID".to_owned(),
                message: format!("Недопустимый путь «{path}»."),
                file_id: Some(*id),
                path: Some(path.clone()),
            });
        }
        if !matches!(side.as_str(), "client" | "server" | "both") {
            errors.push(Issue {
                code: "SIDE_INVALID".to_owned(),
                message: format!("Неизвестная сторона установки для «{path}»."),
                file_id: Some(*id),
                path: Some(path.clone()),
            });
        }
        if object_id.is_none() {
            errors.push(Issue {
                code: "OBJECT_MISSING".to_owned(),
                message: format!("Файл «{path}» не имеет объекта хранения."),
                file_id: Some(*id),
                path: Some(path.clone()),
            });
            continue;
        }
        if state.as_deref() != Some("committed") {
            errors.push(Issue {
                code: "OBJECT_NOT_READY".to_owned(),
                message: format!("Объект файла «{path}» недоступен в хранилище."),
                file_id: Some(*id),
                path: Some(path.clone()),
            });
            continue;
        }
        if object_size.unwrap_or(-1) != *size {
            errors.push(Issue {
                code: "SIZE_MISMATCH".to_owned(),
                message: format!("Размер «{path}» не совпадает с объектом хранения."),
                file_id: Some(*id),
                path: Some(path.clone()),
            });
        }
        if sha256.is_empty() || sha256.len() != 64 {
            errors.push(Issue {
                code: "SHA256_MISSING".to_owned(),
                message: format!("Для «{path}» не рассчитан SHA-256."),
                file_id: Some(*id),
                path: Some(path.clone()),
            });
        } else if object_sha.as_deref() != Some(sha256.as_str()) {
            errors.push(Issue {
                code: "SHA256_MISMATCH".to_owned(),
                message: format!("SHA-256 «{path}» не совпадает с объектом хранения."),
                file_id: Some(*id),
                path: Some(path.clone()),
            });
        }
    }

    // --- launcher-ready: точный контракт MyLauncher -----------------------
    if !super::launcher_manifest::valid_pack_id(&build.slug) {
        errors.push(error(
            "LAUNCHER_PACK_ID",
            "Slug сборки не подходит для MyLauncher: нужно 3–64 символа a-z0-9-, первый и последний — буква или цифра.",
        ));
    }
    if !super::launcher_manifest::SUPPORTED_LOADERS.contains(&build.loader_type.as_str()) {
        errors.push(error(
            "LAUNCHER_LOADER",
            format!(
                "Загрузчик {} не поддерживается MyLauncher.",
                build.loader_type
            ),
        ));
    }
    if !build.minecraft_version.is_empty() {
        let required = super::launcher_manifest::required_java_major(&build.minecraft_version);
        if build.java_major as u8 != required {
            errors.push(error(
                "LAUNCHER_JAVA",
                format!(
                    "MyLauncher требует Java {} для Minecraft {}, в сборке указана Java {}.",
                    required, build.minecraft_version, build.java_major
                ),
            ));
        }
    }
    let mut launcher_total: u64 = 0;
    let mut launcher_files = 0i64;
    for row in &rows {
        if !row.enabled {
            if row.required {
                errors.push(Issue {
                    code: "REQUIRED_DISABLED".to_owned(),
                    message: format!(
                        "Обязательный файл «{}» отключён и не попадёт в манифест.",
                        row.path
                    ),
                    file_id: Some(row.id),
                    path: Some(row.path.clone()),
                });
            }
            continue;
        }
        if row.side != "client" && row.side != "both" {
            continue;
        }
        launcher_files += 1;
        if let Err(reason) = super::launcher_manifest::check_launcher_path(&row.path) {
            errors.push(Issue {
                code: "LAUNCHER_PATH".to_owned(),
                message: format!("Файл «{}» несовместим с MyLauncher: {reason}.", row.path),
                file_id: Some(row.id),
                path: Some(row.path.clone()),
            });
        }
        if row.size_bytes < 0 {
            errors.push(Issue {
                code: "LAUNCHER_SIZE".to_owned(),
                message: format!("Файл «{}» имеет отрицательный размер.", row.path),
                file_id: Some(row.id),
                path: Some(row.path.clone()),
            });
        } else {
            launcher_total = launcher_total.saturating_add(row.size_bytes as u64);
            if row.size_bytes as u64 > super::launcher_manifest::MAX_FILE_BYTES {
                errors.push(Issue {
                    code: "LAUNCHER_SIZE".to_owned(),
                    message: format!("Файл «{}» больше 8 ГиБ.", row.path),
                    file_id: Some(row.id),
                    path: Some(row.path.clone()),
                });
            }
        }
        let lower = row.sha256.to_ascii_lowercase();
        if lower.len() != 64 || !lower.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            errors.push(Issue {
                code: "LAUNCHER_HASH".to_owned(),
                message: format!("Файл «{}» не имеет корректного SHA-256.", row.path),
                file_id: Some(row.id),
                path: Some(row.path.clone()),
            });
        } else if row.sha256 != lower {
            errors.push(Issue {
                code: "LAUNCHER_HASH".to_owned(),
                message: format!(
                    "SHA-256 файла «{}» должен быть в нижнем регистре.",
                    row.path
                ),
                file_id: Some(row.id),
                path: Some(row.path.clone()),
            });
        }
    }
    if launcher_total > super::launcher_manifest::MAX_TOTAL_BYTES {
        errors.push(error(
            "LAUNCHER_SIZE",
            "Сборка для MyLauncher превышает 20 ГБ.",
        ));
    }
    if launcher_files == 0 && !rows.is_empty() {
        warnings.push(warn(
            "LAUNCHER_NO_CLIENT_FILES",
            "Нет включённых файлов для клиента (side client/both): лаунчер установит только Minecraft и загрузчик.",
        ));
    }

    if required_files == 0 && !rows.is_empty() {
        warnings.push(warn(
            "NO_REQUIRED_FILES",
            "Нет обязательных файлов: лаунчер может установить сборку не полностью.",
        ));
    }
    if build.visibility == "public" && build.loader_type == "vanilla" && rows.is_empty() {
        warnings.push(warn(
            "EMPTY_PUBLIC_BUILD",
            "Публичная сборка без файлов будет бесполезной для клиента.",
        ));
    }

    Ok(ValidationReport {
        valid: errors.is_empty(),
        errors,
        warnings,
        file_count: rows.len() as i64,
        total_bytes,
    })
}

/// Публичный JSON для UI проверки.
pub fn report_json(report: &ValidationReport) -> serde_json::Value {
    json!({
        "valid": report.valid,
        "errors": report.errors,
        "warnings": report.warnings,
        "fileCount": report.file_count,
        "totalBytes": report.total_bytes,
    })
}
