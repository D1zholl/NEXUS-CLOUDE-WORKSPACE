use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::paths::CommandNoWindow;

pub fn run_capture(
    program: &Path,
    args: &[String],
    cwd: &Path,
    envs: &[(String, String)],
) -> Result<Output> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags_no_window();
    for (key, value) in envs {
        command.env(key, value);
    }
    command
        .output()
        .with_context(|| format!("не удалось запустить {}", program.display()))
}

pub fn is_port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

pub fn find_free_port(start: u16) -> Option<u16> {
    (start..start.saturating_add(200)).find(|port| is_port_free(*port))
}

pub fn wait_tcp(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// Минимальный HTTP/1.1 GET без внешних зависимостей: используется только для
/// проверки готовности локального backend (loopback).
pub fn http_get(port: u16, path: &str, timeout: Duration) -> Result<(u16, String)> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .with_context(|| format!("нет соединения с 127.0.0.1:{port}"))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\
         User-Agent: NEXUS-Launcher/{}\r\nAccept: */*\r\n\r\n",
        crate::paths::VERSION
    );
    stream.write_all(request.as_bytes())?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .map(|(head, body)| (head, body.to_owned()))
        .unwrap_or((text.as_ref(), String::new()));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    Ok((status, body))
}

pub fn health_state(port: u16) -> (u16, String) {
    match http_get(port, "/api/v1/health", Duration::from_secs(3)) {
        Ok((status, body)) => {
            let state = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("data")
                        .and_then(|data| data.get("status"))
                        .and_then(|status| status.as_str())
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_else(|| "degraded".into());
            (status, state)
        }
        Err(_) => (0, "down".into()),
    }
}

pub fn wait_health(port: u16, timeout: Duration, alive: impl Fn() -> bool) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last = (0u16, "down".to_owned());
    while Instant::now() < deadline {
        if !alive() {
            bail!("backend завершился до готовности");
        }
        let (status, state) = health_state(port);
        last = (status, state.clone());
        if status == 200 && state == "ok" {
            return Ok(state);
        }
        // 503 при прогреве БД/фоновых задач — это ещё не ошибка, ждём дальше.
        std::thread::sleep(Duration::from_millis(400));
    }
    bail!(
        "backend не ответил за {} с (HTTP {}, состояние {})",
        timeout.as_secs(),
        last.0,
        last.1
    )
}

#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if pid == 0 {
        return false;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        ok && code == STILL_ACTIVE as u32
    }
}

#[cfg(not(windows))]
pub fn process_alive(_pid: u32) -> bool {
    false
}

/// Полный путь исполняемого файла процесса. Нужен, чтобы не принять
/// переиспользованный PID за свой backend и не завершить чужой процесс.
#[cfg(windows)]
pub fn process_image_path(pid: u32) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some(std::path::PathBuf::from(String::from_utf16_lossy(
            &buffer[..size as usize],
        )))
    }
}

#[cfg(not(windows))]
pub fn process_image_path(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

pub fn terminate(pid: u32, force: bool) {
    if pid == 0 {
        return;
    }
    let mut command = Command::new("taskkill.exe");
    command.arg("/PID").arg(pid.to_string()).arg("/T");
    if force {
        command.arg("/F");
    }
    let _ = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags_no_window()
        .status();
}

pub fn wait_process_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    !process_alive(pid)
}

#[cfg(windows)]
pub fn disk_space(path: &std::path::Path) -> Option<(u64, u64)> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let dir = wide(&path.to_string_lossy());
    let mut free = 0u64;
    let mut total = 0u64;
    unsafe {
        if GetDiskFreeSpaceExW(dir.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) != 0 {
            Some((free, total))
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
pub fn disk_space(_path: &std::path::Path) -> Option<(u64, u64)> {
    None
}

#[cfg(windows)]
pub fn open_external(target: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let operation = wide("open");
    let file = wide(target);
    unsafe {
        let result = ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
        result as isize > 32
    }
}

#[cfg(not(windows))]
pub fn open_external(_target: &str) -> bool {
    false
}
