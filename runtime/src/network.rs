//! Автоматическая сетевая изоляция запускаемых программ.
//!
//! Для программ с `network_policy = "deny"` Runtime временно добавляет
//! outbound-правило Windows Firewall для точного пути executable и удаляет его
//! после завершения. Если Runtime не имеет прав на изменение firewall, запуск
//! отклоняется (fail closed), а не выполняется без изоляции.
//!
//! Ограничение: Windows Firewall rule адресуется executable, а не конкретному
//! процессу, поэтому на время запуска блокируется сеть для всех процессов этого
//! executable. Правило учитывает счётчик одновременных запусков и удаляется
//! только после последнего.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::protocol::NetworkIsolationStatus;

struct State {
    refcounts: HashMap<String, u32>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            refcounts: HashMap::new(),
        })
    })
}

fn rule_name(key: &str) -> String {
    format!("NEXUS-AI-Runtime-Deny-{key}")
}

fn rule_key(executable: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(executable.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())[..16].to_owned()
}

#[cfg(windows)]
fn run_netsh_add(rule: &str, executable: &Path) -> Result<()> {
    let output = std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={rule}"),
            "dir=out",
            &format!("program={}", executable.display()),
            "action=block",
            "enable=yes",
            "profile=any",
        ])
        .output()
        .context("не удалось запустить netsh advfirewall")?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stdout)
            .chars()
            .take(500)
            .collect::<String>();
        bail!("не удалось добавить firewall-правило изоляции: {message}");
    }
    Ok(())
}

#[cfg(windows)]
fn run_netsh_delete(rule: &str) {
    let _ = std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            &format!("name={rule}"),
            "dir=out",
        ])
        .output();
}

#[cfg(not(windows))]
fn run_netsh_add(_rule: &str, _executable: &Path) -> Result<()> {
    bail!("автоматическая сетевая изоляция программ поддерживается только на Windows")
}

#[cfg(not(windows))]
fn run_netsh_delete(_rule: &str) {}

pub fn is_elevated() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        #[cfg(windows)]
        {
            // `net session` требует административных прав и не меняет состояние.
            std::process::Command::new("net")
                .arg("session")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        }
        #[cfg(not(windows))]
        {
            // На Unix изоляция не реализована; не выдаём её за доступную.
            false
        }
    })
}

pub fn status() -> NetworkIsolationStatus {
    #[cfg(windows)]
    {
        let elevated = is_elevated();
        NetworkIsolationStatus {
            supported: true,
            elevated,
            detail: if elevated {
                "Windows Firewall outbound block доступен".to_owned()
            } else {
                "Runtime запущен без прав администратора: program.run с networkPolicy=deny будет отклонён"
                    .to_owned()
            },
        }
    }
    #[cfg(not(windows))]
    {
        NetworkIsolationStatus {
            supported: false,
            elevated: false,
            detail: "сетевая изоляция программ реализована только для Windows".to_owned(),
        }
    }
}

/// Guard, удерживающий firewall-правило до завершения процесса.
pub struct NetworkGuard {
    key: Option<String>,
}

impl NetworkGuard {
    /// `policy` = "allow" отключает изоляцию; любое другое значение требует
    /// успешно применённого deny-правила.
    pub fn acquire(executable: &Path, policy: &str) -> Result<Self> {
        if policy == "allow" {
            return Ok(Self { key: None });
        }
        if policy != "deny" {
            bail!("неизвестная networkPolicy: {policy}");
        }
        #[cfg(not(windows))]
        {
            let _ = executable;
            bail!("сетевая изоляция программ не поддерживается на этой платформе");
        }
        #[cfg(windows)]
        {
            let executable = std::fs::canonicalize(executable)
                .with_context(|| format!("executable не найден: {}", executable.display()))?;
            let key = rule_key(&executable);
            let rule = rule_name(&key);
            {
                let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
                let count = state.refcounts.get(&key).copied().unwrap_or(0);
                if count == 0 {
                    run_netsh_add(&rule, &executable)?;
                }
                state.refcounts.insert(key.clone(), count + 1);
            }
            Ok(Self { key: Some(key) })
        }
    }
}

impl Drop for NetworkGuard {
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        let remove = {
            let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
            match state.refcounts.get_mut(&key) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                _ => {
                    state.refcounts.remove(&key);
                    true
                }
            }
        };
        if remove {
            run_netsh_delete(&rule_name(&key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_keys_are_stable_and_short() {
        let key = rule_key(Path::new("C:\\tools\\git.exe"));
        assert_eq!(key.len(), 16);
        assert_eq!(key, rule_key(Path::new("C:\\tools\\git.exe")));
        assert_ne!(key, rule_key(Path::new("C:\\tools\\python.exe")));
    }

    #[test]
    fn allow_policy_is_noop() {
        let guard = NetworkGuard::acquire(Path::new("does-not-exist.exe"), "allow").unwrap();
        drop(guard);
    }

    #[test]
    fn unknown_policy_rejected() {
        assert!(NetworkGuard::acquire(Path::new("x.exe"), "maybe").is_err());
    }
}
