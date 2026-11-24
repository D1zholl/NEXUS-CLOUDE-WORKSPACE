//! Безопасные файловые операции внутри закреплённого workspace.
//!
//! Любой путь сначала нормализуется как относительный, затем проверяется на
//! выход за root, symlink/junction/reparse points и канонизируется
//! непосредственно перед операцией.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    pub fn new(workspace_root: &str, allowed_roots: &[PathBuf]) -> Result<Self> {
        let raw = Path::new(workspace_root);
        if !raw.is_absolute() {
            bail!("workspace_root должен быть абсолютным путём");
        }
        if !raw.exists() {
            bail!("workspace_root не существует на Runtime");
        }
        let canonical = std::fs::canonicalize(raw)
            .with_context(|| format!("workspace_root недоступен: {workspace_root}"))?;
        if !canonical.is_dir() {
            bail!("workspace_root не является каталогом");
        }
        if is_reparse_point(&canonical)? {
            bail!("workspace_root не может быть symlink/junction");
        }
        let allowed = allowed_roots
            .iter()
            .any(|allowed| canonical.starts_with(allowed) && canonical != *allowed);
        if !allowed {
            bail!(
                "workspace_root не входит в разрешённые roots Runtime: {}",
                canonical.display()
            );
        }
        if path_has_reparse_ancestor(&canonical, allowed_roots) {
            bail!("workspace_root содержит symlink/junction в пути");
        }
        Ok(Self { root: canonical })
    }

    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Разрешает путь для чтения/удаления: цель может не существовать только
    /// для операций, где это проверяется отдельно.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf> {
        let relative = relative.trim();
        if relative.is_empty() {
            return Ok(self.root.clone());
        }
        let normalized = normalize_relative(relative)?;
        let candidate = self.root.join(&normalized);
        // Длиннейший существующий предок проверяем на reparse и containment.
        let mut existing = candidate.clone();
        while !existing.exists() {
            if !existing.pop() {
                bail!("некорректный путь");
            }
            if !existing.starts_with(&self.root) {
                bail!("путь выходит за пределы workspace");
            }
        }
        let canonical_existing = std::fs::canonicalize(&existing)
            .with_context(|| format!("не удалось проверить путь {}", existing.display()))?;
        if !canonical_existing.starts_with(&self.root) {
            bail!("путь выходит за пределы workspace");
        }
        if is_reparse_point(&existing)? {
            bail!("symlink/junction запрещён в workspace");
        }
        if let Some(parent) = candidate.parent() {
            let mut cursor = self.root.clone();
            if let Ok(rest) = parent.strip_prefix(&self.root) {
                for component in rest.components() {
                    if let Component::Normal(name) = component {
                        cursor.push(name);
                        if cursor.exists() && is_reparse_point(&cursor)? {
                            bail!("symlink/junction запрещён в пути workspace");
                        }
                    }
                }
            }
        }
        Ok(candidate)
    }

    pub fn resolve_existing(&self, relative: &str) -> Result<PathBuf> {
        let path = self.resolve(relative)?;
        if path == self.root {
            bail!("операция над корнем workspace запрещена");
        }
        if !path.exists() {
            bail!("файл или каталог не найден: {relative}");
        }
        if is_reparse_point(&path)? {
            bail!("symlink/junction запрещён");
        }
        let canonical = std::fs::canonicalize(&path)?;
        if !canonical.starts_with(&self.root) {
            bail!("путь выходит за пределы workspace");
        }
        Ok(canonical)
    }
}

pub fn normalize_relative(value: &str) -> Result<PathBuf> {
    if value.contains('\0') {
        bail!("путь содержит NUL");
    }
    let normalized = value.replace('\\', "/");
    if normalized.starts_with('/') || normalized.starts_with("//") {
        bail!("абсолютные пути запрещены");
    }
    if normalized.len() >= 2 && normalized.as_bytes()[1] == b':' {
        bail!("абсолютные пути с буквой диска запрещены");
    }
    let mut result = PathBuf::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            bail!("'..' запрещён в путях workspace");
        }
        if part.contains(':') {
            bail!("':' запрещён в путях workspace");
        }
        if part.ends_with(' ') || part.ends_with('.') {
            bail!("имена с завершающей точкой/пробелом запрещены");
        }
        if part.len() > 200 {
            bail!("слишком длинный компонент пути");
        }
        result.push(part);
    }
    if result.as_os_str().is_empty() {
        bail!("пустой путь");
    }
    Ok(result)
}

fn path_has_reparse_ancestor(path: &Path, roots: &[PathBuf]) -> bool {
    let Some(root) = roots.iter().find(|root| path.starts_with(root)) else {
        return true;
    };
    let mut cursor = root.clone();
    let Ok(rest) = path.strip_prefix(root) else {
        return true;
    };
    for component in rest.components() {
        if let Component::Normal(name) = component {
            cursor.push(name);
            if cursor.exists() && is_reparse_point(&cursor).unwrap_or(true) {
                return true;
            }
        }
    }
    false
}

#[cfg(windows)]
pub fn is_reparse_point(path: &Path) -> Result<bool> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(true);
    }
    Ok(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(not(windows))]
pub fn is_reparse_point(path: &Path) -> Result<bool> {
    Ok(std::fs::symlink_metadata(path)?.file_type().is_symlink())
}

pub fn read_text(path: &Path, max_bytes: usize) -> Result<(String, bool)> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        bail!("путь не является файлом");
    }
    if metadata.len() as usize > max_bytes {
        bail!(
            "файл больше допустимого размера для чтения ({} байт)",
            max_bytes
        );
    }
    let file = std::fs::File::open(path)?;
    let mut buffer = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes as u64 + 1).read_to_end(&mut buffer)?;
    let truncated = buffer.len() > max_bytes;
    if truncated {
        buffer.truncate(max_bytes);
    }
    let text =
        String::from_utf8(buffer).map_err(|_| anyhow::anyhow!("файл не является UTF-8 текстом"))?;
    Ok((text, truncated))
}

pub fn write_atomic(path: &Path, content: &[u8], overwrite: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("нет родительского каталога"))?;
    if !parent.is_dir() {
        bail!("родительский каталог не существует");
    }
    if path.exists() && !overwrite {
        bail!("файл уже существует; укажите overwrite=true");
    }
    let temp = parent.join(format!(".nexus-tmp-{}", uuid::Uuid::new_v4().simple()));
    {
        let mut file = std::fs::File::create(&temp)
            .with_context(|| format!("не удалось создать {}", temp.display()))?;
        file.write_all(content)?;
        file.sync_all()?;
    }
    if path.exists() {
        std::fs::remove_file(path)
            .with_context(|| format!("не удалось заменить {}", path.display()))?;
    }
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("не удалось записать {}", path.display()));
    }
    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn list_entries(
    workspace: &Workspace,
    relative: &str,
    depth: usize,
    max_entries: usize,
    skip: &[String],
) -> Result<Value> {
    let start = if relative.trim().is_empty() {
        workspace.root.clone()
    } else {
        workspace.resolve_existing(relative)?
    };
    if !start.is_dir() {
        bail!("путь не является каталогом");
    }
    let mut entries = Vec::new();
    let mut stack = vec![(start.clone(), 0usize)];
    let mut truncated = false;
    while let Some((dir, level)) = stack.pop() {
        if level >= depth.max(1) {
            continue;
        }
        let mut children: Vec<_> = std::fs::read_dir(&dir)?.filter_map(Result::ok).collect();
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            if entries.len() >= max_entries {
                truncated = true;
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if skip.iter().any(|item| item == &name) {
                continue;
            }
            if is_reparse_point(&path).unwrap_or(true) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            let kind = if metadata.is_dir() { "dir" } else { "file" };
            entries.push(json!({
                "path": workspace.relative(&path),
                "name": name,
                "type": kind,
                "size": if metadata.is_file() { metadata.len() } else { 0 },
                "modified": metadata.modified().ok().map(|time| {
                    chrono::DateTime::<chrono::Utc>::from(time)
                }),
            }));
            if metadata.is_dir() {
                stack.push((path, level + 1));
            }
        }
        if truncated {
            break;
        }
    }
    Ok(json!({
        "path": if relative.trim().is_empty() { ".".to_owned() } else { relative.replace('\\', "/") },
        "entries": entries,
        "truncated": truncated,
    }))
}

pub fn search(
    workspace: &Workspace,
    query: &str,
    relative: &str,
    max_files: usize,
    max_results: usize,
    skip: &[String],
    max_file_bytes: usize,
) -> Result<Value> {
    if query.is_empty() {
        bail!("пустой поисковый запрос");
    }
    let needle = query.to_lowercase();
    let start = if relative.trim().is_empty() {
        workspace.root.clone()
    } else {
        workspace.resolve_existing(relative)?
    };
    if !start.is_dir() {
        bail!("поиск возможен только в каталоге");
    }
    let mut results = Vec::new();
    let mut visited = 0usize;
    let mut truncated = false;
    let mut stack = vec![start];
    while let Some(dir) = stack.pop() {
        let mut children: Vec<_> = std::fs::read_dir(&dir)?.filter_map(Result::ok).collect();
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            if visited >= max_files || results.len() >= max_results {
                truncated = true;
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if skip.iter().any(|item| item == &name) {
                continue;
            }
            if is_reparse_point(&path).unwrap_or(true) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            if !metadata.is_file() || metadata.len() as usize > max_file_bytes {
                continue;
            }
            visited += 1;
            let Ok((text, _)) = read_text(&path, max_file_bytes) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                if line.to_lowercase().contains(&needle) {
                    results.push(json!({
                        "path": workspace.relative(&path),
                        "line": index + 1,
                        "preview": line.chars().take(300).collect::<String>(),
                    }));
                    if results.len() >= max_results {
                        truncated = true;
                        break;
                    }
                }
            }
        }
        if truncated {
            break;
        }
    }
    Ok(json!({
        "query": query,
        "results": results,
        "visitedFiles": visited,
        "truncated": truncated,
    }))
}

pub fn directory_usage(root: &Path, max_entries: usize) -> Result<(u64, u64)> {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .with_context(|| format!("не удалось прочитать {}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            if visited >= max_entries {
                bail!("превышен лимит обхода workspace при проверке квоты");
            }
            visited += 1;
            let path = entry.path();
            if is_reparse_point(&path).unwrap_or(true) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                bytes += metadata.len();
                files += 1;
            }
        }
    }
    Ok((bytes, files))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        let allowed = vec![std::fs::canonicalize(dir.path()).unwrap()];
        let ws = Workspace::new(root.to_str().unwrap(), &allowed).unwrap();
        (dir, ws)
    }

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        assert!(normalize_relative("../secret").is_err());
        assert!(normalize_relative("a/../../b").is_err());
        assert!(normalize_relative("C:/Windows").is_err());
        assert!(normalize_relative(r"\\server\share").is_err());
        assert_eq!(
            normalize_relative("src/main.rs").unwrap(),
            PathBuf::from("src/main.rs")
        );
    }

    #[test]
    fn resolves_inside_workspace() {
        let (_dir, ws) = workspace();
        std::fs::create_dir_all(ws.root.join("src")).unwrap();
        let path = ws.resolve_existing("src").unwrap();
        assert!(path.starts_with(&ws.root));
        assert!(ws.resolve("new/file.txt").unwrap().starts_with(&ws.root));
        assert!(ws.resolve_existing("missing").is_err());
    }

    #[test]
    fn workspace_must_be_below_allowed_root() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let allowed = vec![std::fs::canonicalize(other.path()).unwrap()];
        let result = Workspace::new(dir.path().to_str().unwrap(), &allowed);
        assert!(result.is_err());
    }

    #[test]
    fn atomic_write_respects_overwrite() {
        let (_dir, ws) = workspace();
        let path = ws.resolve("file.txt").unwrap();
        write_atomic(&path, b"one", false).unwrap();
        assert!(write_atomic(&path, b"two", false).is_err());
        write_atomic(&path, b"two", true).unwrap();
        let (text, _) = read_text(&path, 1024).unwrap();
        assert_eq!(text, "two");
    }

    #[test]
    fn search_returns_lines() {
        let (_dir, ws) = workspace();
        let path = ws.resolve("a.txt").unwrap();
        write_atomic(&path, b"hello\nworld\nhello again\n", false).unwrap();
        let result = search(&ws, "hello", "", 10, 10, &[], 1024).unwrap();
        assert_eq!(result["results"].as_array().unwrap().len(), 2);
    }
}
