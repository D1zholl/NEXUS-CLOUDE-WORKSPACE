//! Проверка путей внутри сборки.
//!
//! Все пути хранятся и сравниваются в нормализованном виде: прямые слэши,
//! без пустых сегментов, без `.`/`..`, без ведущего слэша. Регистр и нормализация
//! Unicode учитываются при проверке дублей.

use std::collections::HashMap;

use unicode_normalization::UnicodeNormalization;

use crate::core::error::{ApiError, ApiResult};

pub const PATH_MAX: usize = 512;
pub const SEGMENT_MAX: usize = 128;

pub const CATEGORIES: &[&str] = &[
    "mods",
    "config",
    "resourcepacks",
    "shaderpacks",
    "scripts",
    "overrides",
    "other",
];

pub const SIDES: &[&str] = &["client", "server", "both"];
pub const LOADERS: &[&str] = &["vanilla", "fabric", "forge", "neoforge", "quilt"];

/// Канонические каталоги инстанса Minecraft.
pub const BUILD_ROOT_FOLDERS: &[&str] = &["mods", "shaderpacks", "resourcepacks", "config"];
/// Legacy-категории NEXUS: сохраняются, но не входят в канонический скелет.
pub const BUILD_LEGACY_ROOTS: &[&str] = &["scripts", "overrides", "other"];
/// Расширения из политики `safe_repository_path` клиента MyLauncher.
pub const LAUNCHER_EXTENSIONS: &[&str] = &[
    "jar",
    "zip",
    "json",
    "json5",
    "toml",
    "bak",
    "backup",
    "cfg",
    "conf",
    "properties",
    "txt",
    "md",
    "png",
    "jpg",
    "jpeg",
    "webp",
    "mcmeta",
    "dat",
    "nbt",
    "snbt",
    "mcfunction",
    "vsh",
    "fsh",
    "glsl",
    "lang",
    "yml",
    "yaml",
    "xml",
    "ini",
];

/// Ключ сравнения пути: NFC, нижний регистр, прямые слэши.
pub fn path_key(path: &str) -> String {
    path.nfc().collect::<String>().to_lowercase()
}

/// Нормализует относительный путь сборки.
pub fn normalize_path(raw: &str) -> ApiResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ApiError::validation("Путь файла не может быть пустым."));
    }
    if trimmed.len() > PATH_MAX {
        return Err(ApiError::validation(
            "Путь файла не может быть длиннее 512 символов.",
        ));
    }
    if trimmed.contains('\0') {
        return Err(ApiError::validation("Путь содержит недопустимый символ."));
    }
    let unified = trimmed.replace('\\', "/");
    let normalized = unified.nfc().collect::<String>();
    if normalized.starts_with('/') {
        return Err(ApiError::validation(
            "Путь должен быть относительным: абсолютные пути запрещены.",
        ));
    }
    // Windows-диски (`C:`) и UNC-пути недопустимы.
    if normalized.len() > 1 && normalized.as_bytes()[1] == b':' {
        return Err(ApiError::validation(
            "Путь должен быть относительным: абсолютные пути запрещены.",
        ));
    }

    let mut segments: Vec<&str> = Vec::new();
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(ApiError::validation(
                "Путь не может содержать переход на уровень выше (..).",
            ));
        }
        if segment.chars().count() > SEGMENT_MAX {
            return Err(ApiError::validation(
                "Имя файла или папки не может быть длиннее 128 символов.",
            ));
        }
        if segment.chars().any(|ch| ch.is_control()) {
            return Err(ApiError::validation(
                "Имя файла не может содержать управляющие символы.",
            ));
        }
        // Символы, недопустимые в путях Windows: сборка может ставиться на Windows.
        if segment
            .chars()
            .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        {
            return Err(ApiError::validation(
                "Имя файла содержит символ, недопустимый в путях Windows.",
            ));
        }
        if segment.ends_with(' ') || segment.ends_with('.') {
            return Err(ApiError::validation(
                "Имя файла не может заканчиваться точкой или пробелом.",
            ));
        }
        if is_reserved_windows_name(segment) {
            return Err(ApiError::validation(
                "Имя файла совпадает с зарезервированным именем Windows.",
            ));
        }
        segments.push(segment);
    }

    if segments.is_empty() {
        return Err(ApiError::validation("Путь файла не может быть пустым."));
    }
    Ok(segments.join("/"))
}

/// Проверяет путь сборки по каноническому дереву и политике лаунчера.
///
/// `normalize_path` отвечает за переносимость (Windows-имена, `..`, абсолютные
/// пути), а эта функция — за ASCII-политику клиента, allowlist расширений и
/// структуру узлов. Проверка выполняется на каждой мутации, а не только при
/// публикации.
pub fn check_build_path(raw: &str, kind: &str) -> ApiResult<String> {
    let normalized = normalize_path(raw)?;
    let mut segments: Vec<String> = normalized.split('/').map(str::to_owned).collect();
    let first = segments
        .first()
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    for segment in &segments {
        if !segment
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || " ._-+@()§".contains(character))
        {
            return Err(ApiError::validation(format!(
                "Сегмент «{segment}» содержит символ вне политики лаунчера: разрешены только ASCII-буквы, цифры и \" _-+@()§\"."
            )));
        }
    }

    if first == "options.txt" {
        if kind != "file" {
            return Err(ApiError::validation("options.txt — файл, а не папка."));
        }
        if segments.len() != 1 {
            return Err(ApiError::validation(
                "options.txt допускается только в корне версии Minecraft.",
            ));
        }
        return Ok("options.txt".to_owned());
    }

    if ["versions", "assets", "libraries"].contains(&first.as_str()) {
        return Err(ApiError::validation(format!(
            "Каталог «{}» зарезервирован лаунчером (versions/assets/libraries).",
            segments[0]
        )));
    }
    if [
        ".launcher_meta.json",
        ".launcher_meta.json.tmp",
        ".launcher_meta.json.backup",
    ]
    .contains(&first.as_str())
    {
        return Err(ApiError::validation(
            "Служебные файлы .launcher_meta.json запрещены в сборке.",
        ));
    }

    let canonical = BUILD_ROOT_FOLDERS.contains(&first.as_str());
    let legacy = BUILD_LEGACY_ROOTS.contains(&first.as_str());
    if canonical {
        // Канонический корень всегда пишется строчными: иначе на Windows
        // `Mods` и `mods` — один каталог.
        segments[0] = first.clone();
    } else if !legacy {
        return Err(ApiError::validation(format!(
            "Корневой каталог «{}» не входит в каноническое дерево: допустимы options.txt, mods, shaderpacks, resourcepacks, config.",
            segments[0]
        )));
    }

    if kind == "file" && segments.len() == 1 {
        return Err(ApiError::validation(
            "В корне версии разрешён только options.txt.",
        ));
    }

    let extension = segments
        .last()
        .and_then(|segment| {
            segment
                .rsplit_once('.')
                .map(|(_, ext)| ext.to_ascii_lowercase())
        })
        .unwrap_or_default();
    let is_defaults = segments
        .last()
        .map(|segment| segment == ".defaults")
        .unwrap_or(false);
    if kind == "file"
        && canonical
        && !is_defaults
        && !LAUNCHER_EXTENSIONS.contains(&extension.as_str())
    {
        return Err(ApiError::validation(if extension.is_empty() {
            "У файла нет расширения, разрешённого политикой лаунчера.".to_owned()
        } else {
            format!("Расширение .{extension} не разрешено политикой лаунчера.")
        }));
    }

    if canonical {
        match first.as_str() {
            "mods" => {
                if kind == "file" {
                    if segments.len() != 2 {
                        return Err(ApiError::validation(
                            "mods — плоский каталог: файл должен лежать прямо в mods/, подпапки запрещены.",
                        ));
                    }
                    if extension != "jar" {
                        return Err(ApiError::validation("В mods разрешены только файлы .jar."));
                    }
                } else if segments.len() != 1 {
                    return Err(ApiError::validation(
                        "mods не поддерживает подпапки. Конфиги складывайте в config/.",
                    ));
                }
            }
            "shaderpacks" | "resourcepacks" => {
                if kind == "file" {
                    if segments.len() != 2 {
                        return Err(ApiError::validation(format!(
                            "{first} — плоский каталог: файл должен лежать прямо в {first}/."
                        )));
                    }
                    if extension != "zip" {
                        return Err(ApiError::validation(format!(
                            "В {first} разрешены только .zip."
                        )));
                    }
                } else if segments.len() != 1 {
                    return Err(ApiError::validation(format!(
                        "{first} не поддерживает подпапки."
                    )));
                }
            }
            "config" if kind == "file" && segments.len() < 2 => {
                return Err(ApiError::validation(
                    "Файл конфига должен лежать внутри config/ или его подпапки.",
                ));
            }
            "config" => {}
            _ => {}
        }
    }

    Ok(segments.join("/"))
}

/// Категория, жёстко определяемая каноническим путём.
pub fn canonical_category(path: &str) -> Option<&'static str> {
    let first = path.split('/').next()?.to_ascii_lowercase();
    match first.as_str() {
        "options.txt" => Some("config"),
        "mods" => Some("mods"),
        "shaderpacks" => Some("shaderpacks"),
        "resourcepacks" => Some("resourcepacks"),
        "config" => Some("config"),
        _ => None,
    }
}

pub fn legacy_category(path: &str) -> Option<&'static str> {
    let first = path.split('/').next()?.to_ascii_lowercase();
    match first.as_str() {
        "scripts" => Some("scripts"),
        "overrides" => Some("overrides"),
        "other" => Some("other"),
        _ => None,
    }
}

/// Категория по расширению — только для legacy-путей без явного корня.
pub fn infer_extension_category(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".jar") {
        "mods".to_owned()
    } else if lower.ends_with(".zip") {
        "resourcepacks".to_owned()
    } else if lower.ends_with(".toml")
        || lower.ends_with(".json")
        || lower.ends_with(".json5")
        || lower.ends_with(".cfg")
        || lower.ends_with(".conf")
        || lower.ends_with(".properties")
        || lower.ends_with(".txt")
        || lower.ends_with(".yml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".ini")
    {
        "config".to_owned()
    } else if lower.contains("shader") {
        "shaderpacks".to_owned()
    } else if lower.contains("script") || lower.ends_with(".zs") {
        "scripts".to_owned()
    } else {
        "other".to_owned()
    }
}

/// Итоговая категория: для канонических узлов путь главнее запроса.
pub fn resolve_category(path: &str, requested: Option<&str>) -> ApiResult<String> {
    if let Some(canonical) = canonical_category(path) {
        // Путь — источник истины. Ручная категория не может «перевесить»
        // канонический узел (иначе mods-файл оказался бы в config и т.п.).
        return Ok(canonical.to_owned());
    }
    if let Some(legacy) = legacy_category(path) {
        return match requested.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => validate_category(value),
            None => Ok(legacy.to_owned()),
        };
    }
    match requested.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => validate_category(value),
        None => Ok(infer_extension_category(path)),
    }
}

/// Корневые узлы скелета: их нельзя переименовать или удалить.
pub fn is_fixed_build_folder(path: &str) -> bool {
    let key = path_key(path);
    BUILD_ROOT_FOLDERS.contains(&key.as_str())
}

/// options.txt фиксирован по расположению, но его содержимое заменяемо.
pub fn is_fixed_build_root(path: &str) -> bool {
    let key = path_key(path);
    key == "options.txt" || BUILD_ROOT_FOLDERS.contains(&key.as_str())
}

/// Строгая проверка пути манифеста и артефакта.
///
/// Манифест — это контракт с клиентом, в том числе с Windows-клиентом. Поэтому
/// путь принимается только в каноническом переносимом виде: прямые слэши,
/// относительный, NFC, без диска/UNC/ADS и без имён, которые Windows трактует
/// как устройство. Обратный слэш здесь не нормализуется, а отклоняется: иначе
/// один и тот же путь имел бы два представления.
pub fn check_manifest_path(raw: &str) -> Result<(), &'static str> {
    if raw.is_empty() {
        return Err("путь пуст");
    }
    if raw.len() > PATH_MAX {
        return Err("путь длиннее 512 символов");
    }
    if raw.chars().any(char::is_control) {
        return Err("управляющий символ");
    }
    if raw.contains('\\') {
        return Err("обратный слэш");
    }
    if raw.starts_with('/') {
        return Err("абсолютный путь или UNC");
    }
    // Двоеточие — это буква диска (`C:`) или альтернативный поток данных
    // NTFS (`file.jar:stream`); в переносимом пути его быть не может.
    if raw.contains(':') {
        return Err("двоеточие: буква диска или ADS");
    }
    if raw.nfc().collect::<String>() != raw {
        return Err("путь должен быть в Unicode NFC");
    }
    for segment in raw.split('/') {
        if segment.is_empty() {
            return Err("пустой сегмент");
        }
        if segment == "." || segment == ".." {
            return Err("сегмент '.' или '..'");
        }
        if segment.chars().count() > SEGMENT_MAX {
            return Err("имя файла или папки длиннее 128 символов");
        }
        if segment.ends_with(' ') || segment.ends_with('.') {
            return Err("имя заканчивается точкой или пробелом");
        }
        if segment
            .chars()
            .any(|ch| matches!(ch, '<' | '>' | '"' | '|' | '?' | '*'))
        {
            return Err("символ, недопустимый в путях Windows");
        }
        if is_reserved_windows_name(segment) {
            return Err("зарезервированное имя устройства Windows");
        }
    }
    Ok(())
}

/// Проверяет, что пути внутри одной сборки не совпадают без учёта регистра и
/// нормализации Unicode. На Windows `Mods/A.jar` и `mods/a.jar` — один файл,
/// поэтому такой снимок релиза недопустим.
pub fn check_unique_paths(paths: &[String]) -> Result<(), String> {
    let mut seen: HashMap<String, &str> = HashMap::new();
    for path in paths {
        let key = path_key(path);
        if let Some(previous) = seen.insert(key, path.as_str()) {
            return Err(format!(
                "пути «{previous}» и «{path}» совпадают без учёта регистра"
            ));
        }
    }
    Ok(())
}

fn is_reserved_windows_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// Проверяет категорию, сторону и загрузчик.
pub fn validate_category(value: &str) -> ApiResult<String> {
    let value = value.trim().to_ascii_lowercase();
    if CATEGORIES.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(ApiError::validation("Неизвестная категория файла."))
    }
}

pub fn validate_side(value: &str) -> ApiResult<String> {
    let value = value.trim().to_ascii_lowercase();
    if SIDES.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(ApiError::validation(
            "Сторона установки должна быть client, server или both.",
        ))
    }
}

pub fn validate_loader(value: &str) -> ApiResult<String> {
    let value = value.trim().to_ascii_lowercase();
    if LOADERS.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(ApiError::validation(
            "Загрузчик должен быть vanilla, fabric, forge, neoforge или quilt.",
        ))
    }
}

/// Проверяет slug: латиница, цифры, дефис.
pub fn validate_slug(raw: &str) -> ApiResult<String> {
    let slug = raw.trim().to_ascii_lowercase();
    if slug.len() < 2 || slug.len() > 64 {
        return Err(ApiError::validation(
            "Slug должен содержать от 2 до 64 символов.",
        ));
    }
    if !slug
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(ApiError::validation(
            "Slug может содержать только латинские буквы, цифры и дефис.",
        ));
    }
    if slug.starts_with('-') || slug.ends_with('-') || slug.contains("--") {
        return Err(ApiError::validation(
            "Slug не может начинаться или заканчиваться дефисом и не может содержать двойные дефисы.",
        ));
    }
    Ok(slug)
}

/// Проверяет версию сборки/релиза: `1.0.0`, `1.0.0-beta2`.
pub fn validate_version(raw: &str) -> ApiResult<String> {
    let version = raw.trim();
    if version.is_empty() || version.len() > 32 {
        return Err(ApiError::validation(
            "Версия должна содержать от 1 до 32 символов.",
        ));
    }
    if !version
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '+'))
    {
        return Err(ApiError::validation(
            "Версия может содержать буквы, цифры, точку, дефис, подчёркивание и плюс.",
        ));
    }
    Ok(version.to_owned())
}

/// Проверка версии Minecraft: `1.20.1`, `1.21`.
pub fn validate_minecraft_version(raw: &str) -> ApiResult<String> {
    let version = raw.trim();
    if version.is_empty() || version.len() > 32 {
        return Err(ApiError::validation("Укажите версию Minecraft."));
    }
    if !version
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '+' | 'w'))
    {
        return Err(ApiError::validation("Некорректная версия Minecraft."));
    }
    Ok(version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_safe_paths() {
        assert_eq!(
            normalize_path("mods/example.jar").unwrap(),
            "mods/example.jar"
        );
        assert_eq!(
            normalize_path("mods\\config\\a.toml").unwrap(),
            "mods/config/a.toml"
        );
        assert_eq!(
            normalize_path("./config/./x.json").unwrap(),
            "config/x.json"
        );
    }

    #[test]
    fn normalize_rejects_escapes() {
        assert!(normalize_path("../secret").is_err());
        assert!(normalize_path("config/../../etc/passwd").is_err());
        assert!(normalize_path("/etc/passwd").is_err());
        assert!(normalize_path("C:/Windows/x.dll").is_err());
        assert!(normalize_path("config/..").is_err());
        assert!(normalize_path("").is_err());
        assert!(normalize_path("mods/a?.jar").is_err());
        assert!(normalize_path("mods/CON.txt").is_err());
        assert!(normalize_path("mods/trailing.").is_err());
    }

    #[test]
    fn path_key_is_case_and_unicode_insensitive() {
        assert_eq!(path_key("Mods/Example.JAR"), path_key("mods/example.jar"));
        // «й» в NFC и NFD должны совпадать после нормализации.
        let composed = "config/й.json";
        let decomposed = "config/\u{0438}\u{0306}.json";
        assert_eq!(path_key(composed), path_key(decomposed));
    }

    #[test]
    fn slug_rules() {
        assert_eq!(validate_slug("create-expert").unwrap(), "create-expert");
        assert!(validate_slug("Create Expert").is_err());
        assert!(validate_slug("-bad").is_err());
        assert!(validate_slug("a--b").is_err());
        assert!(validate_slug("a").is_err());
    }

    #[test]
    fn version_rules() {
        assert!(validate_version("1.0.0").is_ok());
        assert!(validate_version("1.0.0-beta2").is_ok());
        assert!(validate_version("1.0 0").is_err());
        assert!(validate_minecraft_version("1.20.1").is_ok());
    }

    #[test]
    fn manifest_path_accepts_portable_relative_paths() {
        assert!(check_manifest_path("mods/a.jar").is_ok());
        assert!(check_manifest_path("config/deep/nested/file.json").is_ok());
        assert!(check_manifest_path("mods/Пример.jar").is_ok());
        assert!(check_manifest_path("resourcepacks/pack 1.zip").is_ok());
    }

    #[test]
    fn manifest_path_rejects_absolute_and_windows_forms() {
        // Абсолютный путь.
        assert!(check_manifest_path("/etc/passwd").is_err());
        // Буква диска с разделителем и без него.
        assert!(check_manifest_path("C:/Windows/system32/x.dll").is_err());
        assert!(check_manifest_path("C:evil.jar").is_err());
        // Обратные слэши в любом месте.
        assert!(check_manifest_path("mods\\a.jar").is_err());
        assert!(check_manifest_path("mods/a\\b.jar").is_err());
        // UNC в обоих представлениях.
        assert!(check_manifest_path("//server/share/a.jar").is_err());
        assert!(check_manifest_path("\\\\server\\share\\a.jar").is_err());
        // Переход на уровень выше.
        assert!(check_manifest_path("../a.jar").is_err());
        assert!(check_manifest_path("mods/../../a.jar").is_err());
        assert!(check_manifest_path("mods/..").is_err());
        assert!(check_manifest_path("mods/.").is_err());
        // ADS-двоеточия.
        assert!(check_manifest_path("mods/a.jar:stream").is_err());
        assert!(check_manifest_path("mods/a.jar::$DATA").is_err());
        // Зарезервированные имена устройств.
        assert!(check_manifest_path("CON").is_err());
        assert!(check_manifest_path("mods/nul.txt").is_err());
        assert!(check_manifest_path("mods/COM1.jar").is_err());
        assert!(check_manifest_path("mods/LPT9").is_err());
        // Завершающие точки и пробелы.
        assert!(check_manifest_path("mods/trailing.").is_err());
        assert!(check_manifest_path("mods/trailing ").is_err());
        // Пустые сегменты, управляющие символы и недопустимые символы.
        assert!(check_manifest_path("mods//a.jar").is_err());
        assert!(check_manifest_path("mods/a\u{1}.jar").is_err());
        assert!(check_manifest_path("mods/a?.jar").is_err());
        assert!(check_manifest_path("mods/a|b.jar").is_err());
        assert!(check_manifest_path("").is_err());
    }

    #[test]
    fn manifest_path_rejects_non_nfc() {
        let decomposed = "config/\u{0438}\u{0306}.json";
        assert!(check_manifest_path(decomposed).is_err());
        assert!(check_manifest_path("config/й.json").is_ok());
    }

    #[test]
    fn duplicate_paths_are_detected_case_insensitively() {
        let same_case = vec!["Mods/A.jar".to_owned(), "mods/a.jar".to_owned()];
        let error = check_unique_paths(&same_case).expect_err("коллизия регистра");
        assert!(error.contains("совпадают"));

        // Unicode NFC/NFD — тоже один файл.
        let unicode = vec![
            "config/й.json".to_owned(),
            "config/\u{0438}\u{0306}.json".to_owned(),
        ];
        assert!(check_unique_paths(&unicode).is_err());

        let unique = vec!["mods/a.jar".to_owned(), "mods/b.jar".to_owned()];
        assert!(check_unique_paths(&unique).is_ok());
    }
}
