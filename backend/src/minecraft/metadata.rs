//! Метаданные версий Minecraft и загрузчиков.
//!
//! Используются только официальные источники: манифест версий Mojang,
//! Fabric Meta, Forge Maven, NeoForge Maven, Quilt Meta. Ответы кешируются
//! в `minecraft_loader_versions`; внешние ошибки не раскрываются клиенту.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;

const MOJANG_MANIFEST: &str = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";
const FABRIC_GAME: &str = "https://meta.fabricmc.net/v2/versions/game";
const FABRIC_LOADER: &str = "https://meta.fabricmc.net/v2/versions/loader";
const FORGE_MAVEN: &str =
    "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";
const FORGE_PROMOTIONS: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
const NEOFORGE_MAVEN: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";
const QUILT_LOADER: &str = "https://meta.quiltmc.org/v3/versions/loader";

fn client() -> ApiResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("NEXUS-WORKSPACE/0.1")
        .build()
        .map_err(|_| ApiError::internal("не удалось создать HTTP-клиент"))
}

async fn get_json(client: &reqwest::Client, url: &str) -> ApiResult<Value> {
    let response = client.get(url).send().await.map_err(|error| {
        tracing::warn!(%error, url, "внешний источник метаданных недоступен");
        ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "UPSTREAM_UNAVAILABLE",
            "Источник метаданных временно недоступен. Повторите позже.",
        )
    })?;
    if !response.status().is_success() {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "UPSTREAM_UNAVAILABLE",
            "Источник метаданных вернул ошибку.",
        ));
    }
    response.json::<Value>().await.map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "UPSTREAM_INVALID",
            "Источник метаданных вернул некорректный ответ.",
        )
    })
}

async fn get_text(client: &reqwest::Client, url: &str) -> ApiResult<String> {
    let response = client.get(url).send().await.map_err(|error| {
        tracing::warn!(%error, url, "внешний источник метаданных недоступен");
        ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "UPSTREAM_UNAVAILABLE",
            "Источник метаданных временно недоступен. Повторите позже.",
        )
    })?;
    if !response.status().is_success() {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "UPSTREAM_UNAVAILABLE",
            "Источник метаданных вернул ошибку.",
        ));
    }
    response.text().await.map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "UPSTREAM_INVALID",
            "Источник метаданных вернул некорректный ответ.",
        )
    })
}

/// Версии Minecraft из официального манифеста Mojang.
pub async fn minecraft_versions(state: &Arc<AppState>) -> ApiResult<Value> {
    let cache_key = "metadata.minecraft.versions";
    if let Some(cached) = cached_json(&state.db, cache_key).await? {
        return Ok(cached);
    }
    let client = client()?;
    let payload = get_json(&client, MOJANG_MANIFEST).await?;
    let versions: Vec<Value> = payload
        .get("versions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let id = item.get("id").and_then(Value::as_str)?;
                    let kind = item.get("type").and_then(Value::as_str)?;
                    if kind != "release" {
                        return None;
                    }
                    Some(json!({
                        "id": id,
                        "releasedAt": item.get("releaseTime").and_then(Value::as_str),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    let result = json!({ "latest": payload.get("latest"), "versions": versions });
    store_cache(&state.db, cache_key, &result).await?;
    Ok(result)
}

/// Версии Fabric: игра + загрузчик.
pub async fn fabric_versions(state: &Arc<AppState>) -> ApiResult<Value> {
    let cache_key = "metadata.fabric.versions";
    if let Some(cached) = cached_json(&state.db, cache_key).await? {
        return Ok(cached);
    }
    let client = client()?;
    let games = get_json(&client, FABRIC_GAME).await?;
    let loaders = get_json(&client, FABRIC_LOADER).await?;
    let game_list: Vec<String> = games
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("stable").and_then(Value::as_bool).unwrap_or(true))
                .filter_map(|item| item.get("version").and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let loader_list: Vec<String> = loaders
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("version").and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();

    for game in &game_list {
        for loader in loader_list.iter().take(20) {
            remember_loader(&state.db, "fabric", loader, game).await?;
        }
    }

    let result = json!({ "minecraft": game_list, "loader": loader_list });
    store_cache(&state.db, cache_key, &result).await?;
    Ok(result)
}

/// Версии Forge из официального Maven-каталога.
pub async fn forge_versions(state: &Arc<AppState>) -> ApiResult<Value> {
    let cache_key = "metadata.forge.versions";
    if let Some(cached) = cached_json(&state.db, cache_key).await? {
        return Ok(cached);
    }
    let client = client()?;
    let xml = get_text(&client, FORGE_MAVEN).await?;
    let entries = parse_forge_entries(&xml);
    // Каталог Maven отсортирован от старых версий к новым: сохраняем свежие.
    for (game, loader) in entries.iter().rev().take(2000) {
        remember_loader(&state.db, "forge", loader, game).await?;
    }
    let result = json!({
        "entries": entries
            .iter()
            .rev()
            .take(500)
            .map(|(game, loader)| json!({ "minecraft": game, "loader": loader }))
            .collect::<Vec<_>>(),
    });
    store_cache(&state.db, cache_key, &result).await?;
    Ok(result)
}

/// Версии NeoForge из официального Maven-каталога.
pub async fn neoforge_versions(state: &Arc<AppState>) -> ApiResult<Value> {
    let cache_key = "metadata.neoforge.versions";
    if let Some(cached) = cached_json(&state.db, cache_key).await? {
        return Ok(cached);
    }
    let client = client()?;
    let xml = get_text(&client, NEOFORGE_MAVEN).await?;
    let entries = parse_neoforge_entries(&xml);
    for (game, loader) in entries.iter().rev().take(2000) {
        remember_loader(&state.db, "neoforge", loader, game).await?;
    }
    let result = json!({
        "entries": entries
            .iter()
            .rev()
            .take(500)
            .map(|(game, loader)| json!({ "minecraft": game, "loader": loader }))
            .collect::<Vec<_>>(),
    });
    store_cache(&state.db, cache_key, &result).await?;
    Ok(result)
}

/// Версии Quilt: загрузчик.
pub async fn quilt_versions(state: &Arc<AppState>) -> ApiResult<Value> {
    let cache_key = "metadata.quilt.versions";
    if let Some(cached) = cached_json(&state.db, cache_key).await? {
        return Ok(cached);
    }
    let client = client()?;
    let loaders = get_json(&client, QUILT_LOADER).await?;
    let loader_list: Vec<String> = loaders
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("version").and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let result = json!({ "loader": loader_list });
    store_cache(&state.db, cache_key, &result).await?;
    Ok(result)
}

/// Совместимые версии загрузчика для конкретной версии игры.
///
/// Источник истины — официальные API загрузчиков. Ответ кешируется на 6 часов,
/// а найденные пары сохраняются в `minecraft_loader_versions`, чтобы проверка
/// сборки работала и при недоступном внешнем источнике.
pub async fn loaders_for(
    db: &PgPool,
    loader_type: &str,
    minecraft_version: &str,
) -> ApiResult<Value> {
    let versions = if loader_type == "vanilla" {
        Vec::new()
    } else {
        compatible_loaders(db, loader_type, minecraft_version).await?
    };
    Ok(json!({ "loader": loader_type, "minecraft": minecraft_version, "versions": versions }))
}

#[derive(Debug, Clone)]
struct VersionEntry {
    /// 0 — рекомендованная/стабильная, 1 — стабильная, 2 — прочая.
    priority: u8,
    version: String,
}

impl VersionEntry {
    fn new(version: impl Into<String>, priority: u8) -> Self {
        Self {
            version: version.into(),
            priority,
        }
    }
}

fn version_numbers(version: &str) -> Vec<u64> {
    version
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn is_stable_loader_version(version: &str) -> bool {
    let version = version.to_ascii_lowercase();
    !["alpha", "beta", "rc", "snapshot", "pre"]
        .iter()
        .any(|marker| version.contains(marker))
}

/// Новые версии сверху, рекомендованные — раньше остальных.
fn sort_version_entries(entries: Vec<VersionEntry>) -> Vec<String> {
    let mut entries = entries;
    entries.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| version_numbers(&right.version).cmp(&version_numbers(&left.version)))
            .then_with(|| right.version.cmp(&left.version))
    });
    let mut seen = std::collections::HashSet::new();
    entries
        .into_iter()
        .filter(|entry| seen.insert(entry.version.clone()))
        .map(|entry| entry.version)
        .collect()
}

async fn compatible_loaders(
    db: &PgPool,
    loader_type: &str,
    minecraft_version: &str,
) -> ApiResult<Vec<String>> {
    let cache_key = format!("metadata.loaders.{loader_type}.{minecraft_version}");
    if let Some(cached) = cached_json(db, &cache_key).await? {
        if let Some(versions) = cached.get("versions").and_then(Value::as_array) {
            return Ok(versions
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect());
        }
    }

    let client = client()?;
    let fetched = match loader_type {
        "fabric" => fabric_loaders_for(&client, minecraft_version).await,
        "forge" => forge_loaders_for(&client, minecraft_version).await,
        "neoforge" => neoforge_loaders_for(&client, minecraft_version).await,
        "quilt" => quilt_loaders_for(&client, minecraft_version).await,
        _ => Ok(Vec::new()),
    };

    match fetched {
        Ok(entries) => {
            let versions = sort_version_entries(entries);
            if !versions.is_empty() {
                for version in &versions {
                    remember_loader(db, loader_type, version, minecraft_version).await?;
                }
                store_cache(db, &cache_key, &json!({ "versions": versions })).await?;
            }
            Ok(versions)
        }
        Err(error) => {
            // Внешний источник недоступен — используем сохранённые пары.
            let cached: Vec<String> = sqlx::query_scalar(
                "select distinct loader_version from minecraft_loader_versions
                 where loader_type = $1 and minecraft_version = $2",
            )
            .bind(loader_type)
            .bind(minecraft_version)
            .fetch_all(db)
            .await?;
            if cached.is_empty() {
                return Err(error);
            }
            tracing::warn!(
                loader = loader_type,
                minecraft = minecraft_version,
                "источник версий загрузчика недоступен, используется кеш"
            );
            Ok(sort_version_entries(
                cached
                    .into_iter()
                    .map(|version| VersionEntry::new(version, 1))
                    .collect(),
            ))
        }
    }
}

async fn loaders_response(client: &reqwest::Client, url: &str) -> ApiResult<Option<Vec<Value>>> {
    let response = client.get(url).send().await.map_err(|error| {
        tracing::warn!(%error, url, "внешний источник метаданных недоступен");
        ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "UPSTREAM_UNAVAILABLE",
            "Источник метаданных временно недоступен. Повторите позже.",
        )
    })?;
    if matches!(
        response.status(),
        axum::http::StatusCode::BAD_REQUEST | axum::http::StatusCode::NOT_FOUND
    ) {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "UPSTREAM_UNAVAILABLE",
            "Источник метаданных вернул ошибку.",
        ));
    }
    let payload = response.json::<Value>().await.map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "UPSTREAM_INVALID",
            "Источник метаданных вернул некорректный ответ.",
        )
    })?;
    Ok(payload.as_array().cloned())
}

async fn fabric_loaders_for(
    client: &reqwest::Client,
    minecraft_version: &str,
) -> ApiResult<Vec<VersionEntry>> {
    let url = format!("{FABRIC_LOADER}/{minecraft_version}");
    let Some(items) = loaders_response(client, &url).await? else {
        return Ok(Vec::new());
    };
    Ok(items
        .iter()
        .filter_map(|item| {
            let version = item.pointer("/loader/version")?.as_str()?;
            let stable = item
                .pointer("/loader/stable")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| is_stable_loader_version(version));
            Some(VersionEntry::new(version, if stable { 1 } else { 2 }))
        })
        .collect())
}

async fn quilt_loaders_for(
    client: &reqwest::Client,
    minecraft_version: &str,
) -> ApiResult<Vec<VersionEntry>> {
    let url = format!("{QUILT_LOADER}/{minecraft_version}");
    let Some(items) = loaders_response(client, &url).await? else {
        return Ok(Vec::new());
    };
    Ok(items
        .iter()
        .filter_map(|item| {
            let version = item
                .pointer("/loader/version")
                .or_else(|| item.get("version"))?
                .as_str()?;
            let priority = if is_stable_loader_version(version) {
                1
            } else {
                2
            };
            Some(VersionEntry::new(version, priority))
        })
        .collect())
}

fn parse_forge_entries(xml: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    for chunk in xml.split("<version>").skip(1) {
        let Some(end) = chunk.find("</version>") else {
            continue;
        };
        let raw = chunk[..end].trim();
        // Формат Forge: `<minecraft>-<forge>`; в старых версиях у суффикса
        // бывает собственный дефис (`1.7.10-10.13.4.1614-1.7.10`).
        let Some((game, loader)) = raw.split_once('-') else {
            continue;
        };
        let valid_game = game.starts_with("1.")
            && game
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
        if valid_game && !loader.is_empty() && is_stable_loader_version(loader) {
            entries.push((game.to_owned(), loader.to_owned()));
        }
    }
    entries
}

fn parse_neoforge_entries(xml: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    for chunk in xml.split("<version>").skip(1) {
        let Some(end) = chunk.find("</version>") else {
            continue;
        };
        let raw = chunk[..end].trim();
        if !is_stable_loader_version(raw) {
            continue;
        }
        let Some(game) = neoforge_minecraft_version(raw) else {
            continue;
        };
        entries.push((game, raw.to_owned()));
    }
    entries
}

/// Определяет версию Minecraft по версии NeoForge.
///
/// NeoForge 1.20.1 использует линейку `47.1.x`, для 1.20.2–1.24 —
/// схему `20.2.x`, `21.0.x`, `21.1.x` (первая часть — minor версии игры),
/// а для годовых версий (25.x и новее) — `<minecraft>.<build>`.
fn neoforge_minecraft_version(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some((game, _)) = raw.split_once("-neoforge-") {
        return Some(game.to_owned());
    }
    if let Some((game, rest)) = raw.split_once('-') {
        if game.starts_with("1.") && !rest.is_empty() {
            return Some(game.to_owned());
        }
    }
    let parts: Vec<&str> = raw.split('.').collect();
    if parts.len() < 2
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let major: u64 = parts[0].parse().ok()?;
    let minor: u64 = parts[1].parse().ok()?;
    if major == 47 {
        return Some("1.20.1".to_owned());
    }
    if (20..=24).contains(&major) {
        return Some(if minor == 0 {
            format!("1.{major}")
        } else {
            format!("1.{major}.{minor}")
        });
    }
    if major >= 25 {
        // Годовая схема: последний компонент — номер сборки NeoForge.
        if parts.len() < 3 {
            return Some(raw.to_owned());
        }
        return Some(parts[..parts.len() - 1].join("."));
    }
    None
}

async fn forge_loaders_for(
    client: &reqwest::Client,
    minecraft_version: &str,
) -> ApiResult<Vec<VersionEntry>> {
    let mut entries = Vec::new();

    // Promotions дают рекомендованную и последнюю версию для игры.
    if let Ok(promotions) = get_json(client, FORGE_PROMOTIONS).await {
        if let Some(promos) = promotions.get("promos").and_then(Value::as_object) {
            for (channel, priority) in [("recommended", 0u8), ("latest", 1u8)] {
                if let Some(version) = promos
                    .get(&format!("{minecraft_version}-{channel}"))
                    .and_then(Value::as_str)
                {
                    if is_stable_loader_version(version) {
                        entries.push(VersionEntry::new(version, priority));
                    }
                }
            }
        }
    }

    // Полный список совместимых версий из Maven-каталога.
    let xml = get_text(client, FORGE_MAVEN).await?;
    let prefix = format!("{minecraft_version}-");
    for chunk in xml.split("<version>").skip(1) {
        let Some(end) = chunk.find("</version>") else {
            continue;
        };
        let raw = chunk[..end].trim();
        if let Some(loader) = raw.strip_prefix(&prefix) {
            if !loader.is_empty() && is_stable_loader_version(loader) {
                entries.push(VersionEntry::new(loader, 2));
            }
        }
    }
    Ok(entries)
}

async fn neoforge_loaders_for(
    client: &reqwest::Client,
    minecraft_version: &str,
) -> ApiResult<Vec<VersionEntry>> {
    let xml = get_text(client, NEOFORGE_MAVEN).await?;
    Ok(parse_neoforge_entries(&xml)
        .into_iter()
        .filter(|(game, _)| game == minecraft_version)
        .map(|(_, loader)| VersionEntry::new(loader, 1))
        .collect())
}

async fn remember_loader(
    db: &PgPool,
    loader_type: &str,
    loader_version: &str,
    minecraft_version: &str,
) -> ApiResult<()> {
    sqlx::query(
        "insert into minecraft_loader_versions
           (loader_type, loader_version, minecraft_version, fetched_at)
         values ($1, $2, $3, now())
         on conflict (loader_type, loader_version, minecraft_version)
         do update set fetched_at = now()",
    )
    .bind(loader_type)
    .bind(loader_version)
    .bind(minecraft_version)
    .execute(db)
    .await?;
    Ok(())
}

async fn cached_json(db: &PgPool, key: &str) -> ApiResult<Option<Value>> {
    let row: Option<(Value, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("select value, updated_at from system_settings where key = $1")
            .bind(key)
            .fetch_optional(db)
            .await?;
    match row {
        Some((value, updated_at)) => {
            if chrono::Utc::now() - updated_at > chrono::Duration::hours(6) {
                return Ok(None);
            }
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

async fn store_cache(db: &PgPool, key: &str, value: &Value) -> ApiResult<()> {
    sqlx::query(
        "insert into system_settings (key, value, updated_at) values ($1, $2, now())
         on conflict (key) do update set value = excluded.value, updated_at = now()",
    )
    .bind(key)
    .bind(value)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_neoforge_versions_to_minecraft_versions() {
        assert_eq!(
            neoforge_minecraft_version("47.1.106").as_deref(),
            Some("1.20.1")
        );
        assert_eq!(
            neoforge_minecraft_version("20.4.237").as_deref(),
            Some("1.20.4")
        );
        assert_eq!(
            neoforge_minecraft_version("21.0.167").as_deref(),
            Some("1.21")
        );
        assert_eq!(
            neoforge_minecraft_version("21.1.209").as_deref(),
            Some("1.21.1")
        );
        assert_eq!(
            neoforge_minecraft_version("1.20.4-20.4.237").as_deref(),
            Some("1.20.4")
        );
        assert_eq!(
            neoforge_minecraft_version("26.1.2").as_deref(),
            Some("26.1")
        );
        assert_eq!(
            neoforge_minecraft_version("26.1.2.5").as_deref(),
            Some("26.1.2")
        );
        assert_eq!(neoforge_minecraft_version("not-a-version"), None);
    }

    #[test]
    fn parses_only_compatible_forge_entries() {
        let xml = r#"<metadata><versioning><versions>
            <version>1.20.1-47.4.10</version>
            <version>1.20.1-47.5.0-beta</version>
            <version>1.21.1-52.1.20</version>
            <version>1.7.10-10.13.4.1614-1.7.10</version>
        </versions></versioning></metadata>"#;
        let entries = parse_forge_entries(xml);
        assert!(entries.contains(&("1.20.1".to_owned(), "47.4.10".to_owned())));
        assert!(entries.contains(&("1.21.1".to_owned(), "52.1.20".to_owned())));
        assert!(!entries.iter().any(|(_, loader)| loader.contains("beta")));
        assert!(entries.iter().all(|(game, _)| game != "not-a-version"));
    }

    #[test]
    fn sorts_compatible_versions_newest_first() {
        let sorted = sort_version_entries(vec![
            VersionEntry::new("0.9.0", 2),
            VersionEntry::new("0.15.11", 1),
            VersionEntry::new("0.15.9", 1),
            VersionEntry::new("0.16.0", 2),
        ]);
        assert_eq!(sorted, ["0.15.11", "0.15.9", "0.16.0", "0.9.0"]);
    }
}
