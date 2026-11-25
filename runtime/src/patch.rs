//! Ограниченный unified diff для `fs.apply_patch`.
//!
//! Патч применяется только к файлам внутри workspace. Сначала все изменения
//! вычисляются в памяти; запись выполняется только если каждый hunk корректен.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::{Value, json};

use crate::filesystem::{Workspace, normalize_relative, read_text, write_atomic};

#[derive(Debug, Clone)]
struct Hunk {
    old_start: usize,
    old_len: usize,
    lines: Vec<(char, String)>,
}

#[derive(Debug, Clone)]
struct FilePatch {
    old_path: Option<String>,
    new_path: Option<String>,
    hunks: Vec<Hunk>,
}

#[derive(Debug)]
struct AppliedFile {
    path: PathBuf,
    relative: String,
    action: &'static str,
    content: Option<Vec<u8>>,
    before: Option<Vec<u8>>,
    additions: usize,
    deletions: usize,
    old_size: u64,
    new_size: u64,
}

/// Изменение одного файла, пригодное для снимка (до/после).
#[derive(Debug, Clone)]
pub struct PatchSnapshotFile {
    pub path: String,
    pub action: String,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct PatchOutcome {
    pub summary: Value,
    pub snapshots: Vec<PatchSnapshotFile>,
}

/// Квоты workspace для предварительной проверки патча.
#[derive(Debug, Clone, Copy)]
pub struct Quota {
    pub used_bytes: u64,
    pub used_files: u64,
    pub quota_bytes: i64,
    pub max_files: i64,
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn apply(workspace: &Workspace, patch: &str, strip: usize, max_bytes: u64) -> Result<Value> {
    Ok(apply_checked(workspace, patch, strip, max_bytes, None)?.summary)
}

pub fn apply_checked(
    workspace: &Workspace,
    patch: &str,
    strip: usize,
    max_bytes: u64,
    quota: Option<&Quota>,
) -> Result<PatchOutcome> {
    if patch.len() > 1024 * 1024 {
        bail!("патч превышает 1 МБ");
    }
    if patch.trim().is_empty() {
        bail!("пустой патч");
    }
    let patches = parse_patch(patch)?;
    if patches.len() > 20 {
        bail!("патч изменяет слишком много файлов");
    }

    let mut applied = Vec::new();
    for file_patch in patches {
        let old_relative = file_patch
            .old_path
            .as_deref()
            .filter(|path| *path != "/dev/null")
            .map(|path| strip_path(path, strip))
            .transpose()?;
        let new_relative = file_patch
            .new_path
            .as_deref()
            .filter(|path| *path != "/dev/null")
            .map(|path| strip_path(path, strip))
            .transpose()?;
        let target_relative = new_relative
            .clone()
            .or_else(|| old_relative.clone())
            .ok_or_else(|| anyhow::anyhow!("патч не содержит имени файла"))?;

        let mut old_size = 0u64;
        let mut before_bytes: Option<Vec<u8>> = None;
        let old_content = match old_relative.as_deref() {
            Some(relative) if !relative.is_empty() => {
                let path = workspace.resolve_existing(relative)?;
                let metadata = std::fs::metadata(&path)?;
                if metadata.len() > max_bytes {
                    bail!("файл {relative} больше допустимого размера");
                }
                old_size = metadata.len();
                before_bytes = Some(std::fs::read(&path)?);
                let (text, _) = read_text(&path, max_bytes as usize)?;
                split_lines(&text)
            }
            _ => Vec::new(),
        };
        let mut lines = old_content.clone();
        let mut delta: isize = 0;
        let mut additions = 0usize;
        let mut deletions = 0usize;
        for hunk in &file_patch.hunks {
            let old_block: Vec<&str> = hunk
                .lines
                .iter()
                .filter(|(kind, _)| *kind == ' ' || *kind == '-')
                .map(|(_, line)| line.as_str())
                .collect();
            let new_block: Vec<String> = hunk
                .lines
                .iter()
                .filter(|(kind, _)| *kind == ' ' || *kind == '+')
                .map(|(_, line)| line.clone())
                .collect();
            additions += hunk.lines.iter().filter(|(kind, _)| *kind == '+').count();
            deletions += hunk.lines.iter().filter(|(kind, _)| *kind == '-').count();

            let expected = if hunk.old_start == 0 {
                0isize
            } else {
                hunk.old_start as isize - 1 + delta
            };
            let found = find_block(&lines, &old_block, expected);
            let Some(position) = found else {
                bail!(
                    "hunk @@ -{},{} не совпадает с содержимым {}",
                    hunk.old_start,
                    hunk.old_len,
                    target_relative
                );
            };
            lines.splice(
                position..position + old_block.len(),
                new_block.iter().cloned(),
            );
            delta += new_block.len() as isize - old_block.len() as isize;
        }

        let path = workspace.resolve(&target_relative)?;
        let (action, content) = if new_relative.is_none() {
            ("delete", None)
        } else if old_relative.is_none() {
            ("create", Some(join_lines(&lines)))
        } else {
            ("modify", Some(join_lines(&lines)))
        };
        if let Some(bytes) = &content {
            if bytes.len() as u64 > max_bytes {
                bail!("результат патча для {target_relative} превышает лимит файла");
            }
        }
        let new_size = content
            .as_ref()
            .map(|bytes| bytes.len() as u64)
            .unwrap_or(0);
        applied.push(AppliedFile {
            path,
            relative: target_relative,
            action,
            content,
            before: before_bytes,
            additions,
            deletions,
            old_size,
            new_size,
        });
    }

    if let Some(quota) = quota {
        let mut delta_bytes: i64 = 0;
        let mut delta_files: i64 = 0;
        for file in &applied {
            delta_bytes += file.new_size as i64 - file.old_size as i64;
            if file.action == "create" {
                delta_files += 1;
            } else if file.action == "delete" {
                delta_files -= 1;
            }
        }
        if quota.quota_bytes > 0 && quota.used_bytes as i64 + delta_bytes > quota.quota_bytes {
            bail!(
                "патч превышает квоту workspace: {} + {} > {} байт",
                quota.used_bytes,
                delta_bytes,
                quota.quota_bytes
            );
        }
        if quota.max_files > 0 && quota.used_files as i64 + delta_files > quota.max_files {
            bail!(
                "патч превышает лимит файлов workspace: {} + {} > {}",
                quota.used_files,
                delta_files,
                quota.max_files
            );
        }
    }

    // Двухфазная запись: все проверки пройдены, применяем изменения.
    let mut summary = Vec::new();
    let mut snapshots = Vec::new();
    for file in applied {
        let after_bytes = match file.action {
            "delete" => {
                if file.path.exists() {
                    if file.path.is_dir() {
                        bail!("патч пытается удалить каталог {}", file.relative);
                    }
                    std::fs::remove_file(&file.path)
                        .with_context(|| format!("не удалось удалить {}", file.relative))?;
                }
                None
            }
            _ => {
                let bytes = file.content.clone().unwrap_or_default();
                write_atomic(&file.path, &bytes, file.action == "modify")?;
                Some(bytes)
            }
        };
        snapshots.push(PatchSnapshotFile {
            path: file.relative.clone(),
            action: file.action.to_owned(),
            before: file.before,
            after: after_bytes,
        });
        summary.push(json!({
            "path": file.relative,
            "action": file.action,
            "additions": file.additions,
            "deletions": file.deletions,
        }));
    }
    Ok(PatchOutcome {
        summary: json!({
            "files": summary,
            "filesChanged": summary.len(),
            "note": "Изменения применены внутри workspace Runtime."
        }),
        snapshots,
    })
}

fn parse_patch(patch: &str) -> Result<Vec<FilePatch>> {
    let normalized = patch.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let hunk_re = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")?;
    let mut patches = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with("--- ")
            && index + 1 < lines.len()
            && lines[index + 1].starts_with("+++ ")
        {
            let old_path = header_path(line, "--- ");
            let new_path = header_path(lines[index + 1], "+++ ");
            index += 2;
            let mut hunks = Vec::new();
            while index < lines.len() {
                if lines[index].starts_with("--- ") {
                    break;
                }
                if lines[index].starts_with("@@") {
                    let captures = hunk_re
                        .captures(lines[index])
                        .ok_or_else(|| anyhow::anyhow!("некорректный hunk header"))?;
                    let old_start = captures[1].parse::<usize>()?;
                    let old_len = captures
                        .get(2)
                        .map(|value| value.as_str())
                        .filter(|value| !value.is_empty())
                        .map(|value| value.parse::<usize>())
                        .transpose()?
                        .unwrap_or(1);
                    let _new_start = captures[3].parse::<usize>()?;
                    let new_len = captures
                        .get(4)
                        .map(|value| value.as_str())
                        .filter(|value| !value.is_empty())
                        .map(|value| value.parse::<usize>())
                        .transpose()?
                        .unwrap_or(1);
                    index += 1;
                    let mut hunk_lines = Vec::new();
                    while index < lines.len() {
                        let next = lines[index];
                        if next.starts_with("@@") || next.starts_with("--- ") {
                            break;
                        }
                        if next.starts_with(' ') || next.starts_with('+') || next.starts_with('-') {
                            let kind = next.chars().next().unwrap();
                            hunk_lines.push((kind, next[1..].to_owned()));
                            index += 1;
                        } else if next.starts_with('\\') || next.is_empty() {
                            index += 1;
                        } else {
                            bail!("некорректная строка патча: {next}");
                        }
                    }
                    let old_count = hunk_lines
                        .iter()
                        .filter(|(kind, _)| *kind == ' ' || *kind == '-')
                        .count();
                    let new_count = hunk_lines
                        .iter()
                        .filter(|(kind, _)| *kind == ' ' || *kind == '+')
                        .count();
                    if old_count != old_len || new_count != new_len {
                        // Некоторые генераторы опускают длины; принимаем hunk,
                        // если фактическое число строк совпадает по смыслу.
                        if old_len != 0 && old_count != old_len {
                            bail!(
                                "hunk lengths не совпадают: ожидалось {old_len}/{new_len}, получено {old_count}/{new_count}"
                            );
                        }
                    }
                    hunks.push(Hunk {
                        old_start,
                        old_len: old_count,
                        lines: hunk_lines,
                    });
                } else {
                    index += 1;
                }
            }
            if hunks.is_empty() {
                bail!("патч содержит файл без hunks");
            }
            patches.push(FilePatch {
                old_path,
                new_path,
                hunks,
            });
        } else {
            index += 1;
        }
    }
    if patches.is_empty() {
        bail!("в патче не найден ни один файл");
    }
    Ok(patches)
}

fn header_path(line: &str, prefix: &str) -> Option<String> {
    let value = line.strip_prefix(prefix)?.trim();
    let value = value.split('\t').next().unwrap_or(value).trim();
    Some(value.to_owned())
}

fn strip_path(path: &str, strip: usize) -> Result<String> {
    let mut parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() <= strip {
        bail!("strip={strip} удаляет всё имя файла из патча");
    }
    parts.drain(0..strip);
    let joined = parts.join("/");
    normalize_relative(&joined)?;
    Ok(joined)
}

fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let normalized = text.replace("\r\n", "\n");
    let mut lines: Vec<String> = normalized.split('\n').map(ToOwned::to_owned).collect();
    if normalized.ends_with('\n') {
        lines.pop();
        let mut result = Vec::with_capacity(lines.len() + 1);
        result.extend(lines.into_iter().map(|line| format!("{line}\n")));
        result
    } else {
        lines.into_iter().map(|line| format!("{line}\n")).collect()
    }
}

fn join_lines(lines: &[String]) -> Vec<u8> {
    let mut result = String::new();
    for line in lines {
        result.push_str(line.trim_end_matches('\n'));
        result.push('\n');
    }
    if lines.is_empty() {
        return Vec::new();
    }
    // Сохраняем отсутствие завершающего перевода, если последняя строка была
    // помечена в diff как "\ No newline at end of file" (упрощённо — по пустой
    // строке не восстанавливаем).
    result.into_bytes()
}

fn find_block(lines: &[String], block: &[&str], expected: isize) -> Option<usize> {
    let matches_at = |position: isize| -> bool {
        if position < 0 {
            return false;
        }
        let start = position as usize;
        if start + block.len() > lines.len() {
            return false;
        }
        block
            .iter()
            .enumerate()
            .all(|(offset, expected)| lines[start + offset].trim_end_matches('\n') == *expected)
    };
    if matches_at(expected) {
        return Some(expected as usize);
    }
    // Небольшой сдвиг допускается (контекст мог измениться по соседству).
    for distance in 1..=100isize {
        if matches_at(expected + distance) {
            return Some((expected + distance) as usize);
        }
        if matches_at(expected - distance) {
            return Some((expected - distance) as usize);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.txt"), "one\ntwo\nthree\n").unwrap();
        let allowed = vec![std::fs::canonicalize(dir.path()).unwrap()];
        let ws = Workspace::new(root.to_str().unwrap(), &allowed).unwrap();
        (dir, ws)
    }

    #[test]
    fn applies_simple_patch() {
        let (_dir, ws) = workspace();
        let patch = "--- a/main.txt\n+++ b/main.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";
        let result = apply(&ws, patch, 1, 1024 * 1024).unwrap();
        assert_eq!(result["filesChanged"], 1);
        let text = std::fs::read_to_string(ws.root.join("main.txt")).unwrap();
        assert_eq!(text, "one\nTWO\nthree\n");
    }

    #[test]
    fn rejects_patch_outside_workspace() {
        let (_dir, ws) = workspace();
        let patch = "--- a/../../secret.txt\n+++ b/../../secret.txt\n@@ -1 +1 @@\n-x\n+y\n";
        assert!(apply(&ws, patch, 1, 1024 * 1024).is_err());
    }

    #[test]
    fn creates_new_file() {
        let (_dir, ws) = workspace();
        let patch = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n";
        apply(&ws, patch, 1, 1024 * 1024).unwrap();
        assert_eq!(
            std::fs::read_to_string(ws.root.join("new.txt")).unwrap(),
            "hello\nworld\n"
        );
    }

    #[test]
    fn rejects_patch_exceeding_quota_before_write() {
        let (_dir, ws) = workspace();
        let patch = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n";
        let quota = Quota {
            used_bytes: 14,
            used_files: 1,
            quota_bytes: 14,
            max_files: 10,
        };
        let result = apply_checked(&ws, patch, 1, 1024 * 1024, Some(&quota));
        assert!(result.is_err(), "патч должен быть отклонён до записи");
        assert!(
            !ws.root.join("new.txt").exists(),
            "файл не должен быть создан при отказе по квоте"
        );
    }
}
