use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::paths::{AppPaths, VERSION, write_private};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Initializing,
    CheckingEnvironment,
    PreparingDatabase,
    StartingPostgres,
    DatabaseReady,
    RunningMigrations,
    CreatingAdministrator,
    StartingBackend,
    Ready,
    Stopping,
    Stopped,
    Error,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Initializing => "Initializing",
            Phase::CheckingEnvironment => "Checking environment",
            Phase::PreparingDatabase => "Preparing database",
            Phase::StartingPostgres => "Starting PostgreSQL",
            Phase::DatabaseReady => "Database ready",
            Phase::RunningMigrations => "Applying migrations",
            Phase::CreatingAdministrator => "Creating administrator",
            Phase::StartingBackend => "Starting backend",
            Phase::Ready => "NEXUS ready",
            Phase::Stopping => "Stopping",
            Phase::Stopped => "Stopped",
            Phase::Error => "Error",
        }
    }

    pub fn label_ru(self) -> &'static str {
        match self {
            Phase::Initializing => "Инициализация",
            Phase::CheckingEnvironment => "Проверка окружения",
            Phase::PreparingDatabase => "Подготовка базы данных",
            Phase::StartingPostgres => "Запуск PostgreSQL",
            Phase::DatabaseReady => "База данных готова",
            Phase::RunningMigrations => "Применение миграций",
            Phase::CreatingAdministrator => "Создание администратора",
            Phase::StartingBackend => "Запуск backend",
            Phase::Ready => "NEXUS готов",
            Phase::Stopping => "Остановка",
            Phase::Stopped => "Остановлен",
            Phase::Error => "Ошибка",
        }
    }

}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub phase: Phase,
    pub detail: String,
    pub error: String,
    pub warning: String,
    pub app_port: u16,
    pub db_port: u16,
    pub backend_pid: u32,
    pub launcher_pid: u32,
    pub database_running: bool,
    pub backend_running: bool,
    pub admin_present: bool,
    pub runtime_included: bool,
    pub runtime_configured: bool,
    pub started_at: String,
    pub version: String,
    pub updated_at: String,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            phase: Phase::Initializing,
            detail: "Запуск NEXUS".into(),
            error: String::new(),
            warning: String::new(),
            app_port: 8080,
            db_port: 55432,
            backend_pid: 0,
            launcher_pid: std::process::id(),
            database_running: false,
            backend_running: false,
            admin_present: false,
            runtime_included: false,
            runtime_configured: false,
            started_at: String::new(),
            version: VERSION.into(),
            updated_at: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct Ctx {
    pub paths: AppPaths,
    pub shared: Arc<Mutex<UiState>>,
    pub hwnd: isize,
    /// Только владелец жизненного цикла (GUI/--start/--stop) обновляет
    /// state.json. Служебные CLI-команды не должны затирать состояние
    /// работающего лаунчера.
    pub persist: bool,
}

impl Ctx {
    pub fn new(paths: AppPaths) -> Self {
        let mut initial = read_state(&paths).unwrap_or_default();
        initial.launcher_pid = std::process::id();
        Self {
            paths,
            shared: Arc::new(Mutex::new(initial)),
            hwnd: 0,
            persist: true,
        }
    }

    /// CLI-операция, которая не управляет жизненным циклом NEXUS.
    pub fn new_cli(paths: AppPaths) -> Self {
        let mut ctx = Self::new(paths);
        ctx.persist = false;
        ctx
    }

    pub fn with_hwnd(mut self, hwnd: isize) -> Self {
        self.hwnd = hwnd;
        self
    }

    pub fn state(&self) -> UiState {
        self.shared.lock().unwrap().clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut UiState)) {
        let snapshot = {
            let mut state = self.shared.lock().unwrap();
            f(&mut state);
            state.updated_at = chrono::Local::now().to_rfc3339();
            state.clone()
        };
        if self.persist {
            let json = serde_json::to_string_pretty(&snapshot).unwrap_or_default();
            let _ = write_private(&self.paths.state, json.as_bytes());
        }
        self.redraw();
    }

    pub fn set_phase(&self, phase: Phase, detail: impl Into<String>) {
        self.update(|state| {
            state.phase = phase;
            state.detail = detail.into();
            if phase != Phase::Error {
                state.error.clear();
            }
        });
    }

    pub fn set_error(&self, message: impl Into<String>) {
        let message = message.into();
        crate::paths::append_log(&self.paths.launcher_log, &format!("ERROR: {message}"));
        self.update(|state| {
            state.phase = Phase::Error;
            state.detail = "Не удалось запустить NEXUS".into();
            state.error = message;
            state.backend_running = false;
        });
    }

    fn redraw(&self) {
        #[cfg(windows)]
        if self.hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    self.hwnd as _,
                    crate::ui::WM_APP_UPDATE,
                    0,
                    0,
                );
            }
        }
    }
}

pub fn read_state(paths: &AppPaths) -> Option<UiState> {
    let raw = std::fs::read_to_string(&paths.state).ok()?;
    serde_json::from_str(&raw).ok()
}
