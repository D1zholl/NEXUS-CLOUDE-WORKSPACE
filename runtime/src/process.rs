//! Запуск только зарегистрированных программ. Модель не формирует shell-команду:
//! executable берётся из конфигурации Runtime, аргументы проверяются политикой,
//! процессы запускаются без shell и завершаются вместе с деревом.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use crate::config::{LimitsConfig, ProgramConfig};

pub struct ProgramRuntime {
    pub config: ProgramConfig,
    patterns: Vec<Regex>,
    pub available: bool,
}

pub struct ProgramRegistry {
    programs: HashMap<String, ProgramRuntime>,
    limits: LimitsConfig,
}

#[derive(Debug)]
pub struct ProcessOutcome {
    pub result: Value,
    pub result_text: String,
    pub exit_code: Option<i32>,
    pub duration_ms: i64,
}

impl ProgramRegistry {
    pub fn new(programs: &[ProgramConfig], limits: &LimitsConfig) -> Self {
        let mut map = HashMap::new();
        for program in programs {
            let patterns = program
                .arg_patterns
                .iter()
                .filter_map(|pattern| match Regex::new(pattern) {
                    Ok(regex) => Some(regex),
                    Err(error) => {
                        tracing::warn!(
                            program = %program.program_id,
                            %error,
                            "некорректный arg_pattern игнорируется"
                        );
                        None
                    }
                })
                .collect();
            let available = std::path::Path::new(&program.executable).is_file();
            if !available {
                tracing::warn!(
                    program = %program.program_id,
                    executable = %program.executable,
                    "программа не найдена; она будет отмечена как недоступная"
                );
            }
            map.insert(
                program.program_id.clone(),
                ProgramRuntime {
                    config: program.clone(),
                    patterns,
                    available,
                },
            );
        }
        Self {
            programs: map,
            limits: limits.clone(),
        }
    }

    pub fn descriptors(&self) -> Vec<crate::protocol::ProgramDescriptor> {
        let mut items: Vec<_> = self
            .programs
            .values()
            .map(|program| crate::protocol::ProgramDescriptor {
                program_id: program.config.program_id.clone(),
                name: if program.config.name.is_empty() {
                    program.config.program_id.clone()
                } else {
                    program.config.name.clone()
                },
                executable: program.config.executable.clone(),
                version: program.config.version.clone(),
                available: program.available,
                risk: program.config.risk.clone(),
                allowed_args: json!({
                    "allowAnyArgs": program.config.allow_any_args,
                    "patterns": program.patterns.len(),
                    "maxArgs": 32,
                }),
            })
            .collect();
        items.sort_by(|a, b| a.program_id.cmp(&b.program_id));
        items
    }

    pub fn validate_args(&self, program_id: &str, args: &[String]) -> Result<()> {
        let program = self
            .programs
            .get(program_id)
            .ok_or_else(|| anyhow::anyhow!("программа не зарегистрирована"))?;
        if !program.available {
            bail!("программа недоступна на этом Runtime");
        }
        if args.len() > 32 {
            bail!("слишком много аргументов программы");
        }
        for arg in args {
            if arg.len() > 500 {
                bail!("аргумент программы длиннее 500 символов");
            }
            if arg.contains('\0') || arg.chars().any(|c| c.is_control()) {
                bail!("аргумент содержит управляющие символы");
            }
            // Даже зарегистрированная программа не должна получать абсолютные
            // локальные пути и traversal в аргументах: рабочая область ограничена
            // workspace, а не всей файловой системой.
            if arg.len() >= 2 && arg.as_bytes()[1] == b':' {
                bail!("абсолютные пути в аргументах программы запрещены");
            }
            if arg.starts_with(r"\\") || arg.starts_with("//") {
                bail!("UNC-пути в аргументах программы запрещены");
            }
            if arg.split(['/', '\\']).any(|component| component == "..") {
                bail!("'..' в аргументах программы запрещён");
            }
        }
        if program.config.allow_any_args {
            return Ok(());
        }
        if program.patterns.is_empty() {
            if !args.is_empty() {
                bail!("программа не принимает аргументы");
            }
            return Ok(());
        }
        for arg in args {
            if !program.patterns.iter().any(|pattern| pattern.is_match(arg)) {
                bail!("аргумент программы не соответствует разрешённому шаблону: {arg}");
            }
        }
        Ok(())
    }

    pub async fn run(
        &self,
        program_id: &str,
        args: &[String],
        workspace_root: &std::path::Path,
        cwd: Option<&std::path::Path>,
        cancel: &CancellationToken,
    ) -> Result<ProcessOutcome> {
        self.validate_args(program_id, args)?;
        let program = self
            .programs
            .get(program_id)
            .ok_or_else(|| anyhow::anyhow!("программа не зарегистрирована"))?;
        let workdir = cwd.unwrap_or(workspace_root);
        if !workdir.is_dir() || !workdir.starts_with(workspace_root) {
            bail!("рабочий каталог программы должен находиться в workspace");
        }
        if crate::filesystem::is_reparse_point(workdir)? {
            bail!("рабочий каталог не может быть symlink/junction");
        }

        let started = Instant::now();
        let mut command = tokio::process::Command::new(&program.config.executable);
        command
            .args(args)
            .current_dir(workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env_clear();
        for key in &self.limits.env_allowlist {
            if let Ok(value) = std::env::var(key) {
                command.env(key, value);
            }
        }
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW: дочерний процесс не открывает консоль.
            command.creation_flags(0x08000000);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("не удалось запустить {}", program.config.program_id))?;
        let pid = child.id();
        let max_output = program
            .config
            .max_output_bytes
            .min(self.limits.max_tool_output_bytes);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_task = stdout.map(|stream| tokio::spawn(read_limited(stream, max_output)));
        let stderr_task = stderr.map(|stream| tokio::spawn(read_limited(stream, max_output)));
        let timeout = Duration::from_secs(
            program
                .config
                .timeout_seconds
                .clamp(1, 3600)
                .min(self.limits.default_tool_timeout_seconds.max(1) * 20),
        );

        let mut cancelled = false;
        let mut timed_out = false;
        let status = tokio::select! {
            status = child.wait() => Some(status.map_err(|error| anyhow::anyhow!("ошибка ожидания процесса: {error}"))?),
            _ = tokio::time::sleep(timeout) => {
                timed_out = true;
                kill_tree(pid, &mut child).await;
                child.wait().await.ok();
                None
            }
            _ = cancel.cancelled() => {
                cancelled = true;
                kill_tree(pid, &mut child).await;
                child.wait().await.ok();
                None
            }
        };

        let (stdout, stdout_truncated) = match stdout_task {
            Some(task) => task.await.unwrap_or_default(),
            None => (String::new(), false),
        };
        let (stderr, stderr_truncated) = match stderr_task {
            Some(task) => task.await.unwrap_or_default(),
            None => (String::new(), false),
        };
        let duration_ms = started.elapsed().as_millis() as i64;
        let exit_code = status.and_then(|status| status.code());
        let result_text = format!(
            "exit_code: {}\nstdout:\n{}\nstderr:\n{}\n",
            exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "terminated".to_owned()),
            stdout,
            stderr
        );
        let result = json!({
            "programId": program_id,
            "exitCode": exit_code,
            "stdout": stdout,
            "stderr": stderr,
            "truncated": stdout_truncated || stderr_truncated,
            "timedOut": timed_out,
            "cancelled": cancelled,
            "durationMs": duration_ms,
        });
        if timed_out {
            return Err(anyhow::anyhow!(
                "программа превысила timeout {} с и была остановлена",
                timeout.as_secs()
            ));
        }
        if cancelled {
            bail!("выполнение программы отменено");
        }
        Ok(ProcessOutcome {
            result,
            result_text,
            exit_code,
            duration_ms,
        })
    }
}

async fn read_limited<R: AsyncRead + Unpin>(mut reader: R, max_bytes: usize) -> (String, bool) {
    let mut collected = Vec::with_capacity(8 * 1024);
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                if collected.len() < max_bytes {
                    let remaining = max_bytes - collected.len();
                    let take = read.min(remaining);
                    collected.extend_from_slice(&buffer[..take]);
                    if take < read {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(_) => break,
        }
    }
    (String::from_utf8_lossy(&collected).to_string(), truncated)
}

async fn kill_tree(pid: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = pid {
        let _ = tokio::process::Command::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .arg("/F")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ProgramRegistry {
        let programs = vec![ProgramConfig {
            program_id: "demo".into(),
            name: "Demo".into(),
            executable: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .to_string(),
            version: "1".into(),
            risk: "high".into(),
            arg_patterns: vec!["^[a-z0-9-]+$".into()],
            allow_any_args: false,
            timeout_seconds: 10,
            max_output_bytes: 1024,
        }];
        ProgramRegistry::new(&programs, &LimitsConfig::default())
    }

    #[test]
    fn validates_argument_patterns() {
        let registry = registry();
        assert!(
            registry
                .validate_args("demo", &["ok-arg1".to_owned()])
                .is_ok()
        );
        assert!(
            registry
                .validate_args("demo", &["bad arg; rm".to_owned()])
                .is_err()
        );
        assert!(registry.validate_args("unknown", &[]).is_err());
    }

    #[test]
    fn rejects_control_characters() {
        let registry = registry();
        assert!(
            registry
                .validate_args("demo", &["a\nb".to_owned()])
                .is_err()
        );
    }
}
