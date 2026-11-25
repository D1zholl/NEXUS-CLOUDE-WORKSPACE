#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, CreateFontW, CreatePen, CreateSolidBrush,
    DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_RIGHT, DT_SINGLELINE,
    DT_TOP, DT_VCENTER, DT_WORDBREAK, DeleteObject, DrawTextW, Ellipse, EndPaint, FF_DONTCARE,
    FillRect, GetDC, GetDeviceCaps, GetStockObject, InvalidateRect, LOGPIXELSX, NULL_PEN,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, ReleaseDC, RoundRect, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT, UpdateWindow, HDC, HFONT, HOLLOW_BRUSH,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::paths::{append_log, write_text};
use crate::proc::open_external;
use crate::setup;
use crate::state::{Ctx, Phase};

pub const WM_APP_UPDATE: u32 = WM_APP + 1;
const WM_APP_EXIT: u32 = WM_APP + 2;
const TIMER_ID: usize = 1;
const CLASS_NAME: &str = "NexusStandaloneLauncher";

static GLOBAL: OnceLock<Ctx> = OnceLock::new();
static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
static HOVER: AtomicI32 = AtomicI32::new(-1);
static BUSY: AtomicBool = AtomicBool::new(false);
static TICKS: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Open,
    Stop,
    Restart,
    Logs,
    Diagnostics,
    Backup,
    Exit,
}

#[derive(Clone, Copy)]
struct Fonts {
    title: HFONT,
    heading: HFONT,
    body: HFONT,
    small: HFONT,
    mono: HFONT,
}

// HFONT — непрозрачный дескриптор GDI, созданный и используемый только в
// UI-потоке. Mutex нужен лишь для безопасного доступа из отрисовки.
unsafe impl Send for Fonts {}

#[derive(Clone)]
struct Layout {
    title: RECT,
    subtitle: RECT,
    version: RECT,
    card: RECT,
    dot: (i32, i32),
    phase: RECT,
    detail: RECT,
    right: RECT,
    info: RECT,
    info_rows: Vec<RECT>,
    message: RECT,
    buttons: Vec<(Action, RECT, bool)>,
    footer: RECT,
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

const C_BG: COLORREF = 0x000e_0805;
const C_CARD: COLORREF = 0x0020_140d;
const C_CARD2: COLORREF = 0x001b_100a;
const C_BORDER: COLORREF = 0x003f_2b1c;
const C_ACCENT: COLORREF = 0x00ff_cd4e;
const C_TEXT: COLORREF = 0x00ff_f3eb;
const C_MUTED: COLORREF = 0x00b2_9d8b;
const C_GREEN: COLORREF = 0x0096_dc40;
const C_AMBER: COLORREF = 0x0050_be_f5;
const C_RED: COLORREF = 0x006e_5f_ff;

fn scale_for(hwnd: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        96
    } else {
        dpi as i32
    }
}

fn s(value: i32, scale: i32) -> i32 {
    (value * scale + 48) / 96
}

fn rect(x: i32, y: i32, w: i32, h: i32, scale: i32) -> RECT {
    RECT {
        left: s(x, scale),
        top: s(y, scale),
        right: s(x + w, scale),
        bottom: s(y + h, scale),
    }
}

fn layout(scale: i32) -> Layout {
    let buttons = vec![
        (Action::Open, rect(32, 452, 196, 46, scale), true),
        (Action::Stop, rect(238, 452, 136, 46, scale), false),
        (Action::Restart, rect(384, 452, 136, 46, scale), false),
        (Action::Logs, rect(530, 452, 118, 46, scale), false),
        (Action::Diagnostics, rect(32, 508, 196, 38, scale), false),
        (Action::Backup, rect(238, 508, 136, 38, scale), false),
        (Action::Exit, rect(384, 508, 136, 38, scale), false),
    ];
    Layout {
        title: rect(32, 22, 220, 44, scale),
        subtitle: rect(150, 34, 300, 24, scale),
        version: rect(470, 30, 178, 22, scale),
        card: rect(32, 84, 616, 108, scale),
        dot: (s(54, scale), s(116, scale)),
        phase: rect(76, 94, 380, 30, scale),
        detail: rect(76, 126, 540, 50, scale),
        right: rect(452, 96, 180, 52, scale),
        info: rect(32, 204, 616, 172, scale),
        info_rows: (0..6)
            .map(|index| rect(52, 220 + index * 25, 576, 24, scale))
            .collect(),
        message: rect(32, 388, 616, 56, scale),
        buttons,
        footer: rect(530, 512, 118, 30, scale),
    }
}

fn create_font(scale: i32, size: i32, weight: i32, face: &str) -> HFONT {
    let face = wide(face);
    unsafe {
        CreateFontW(
            -s(size, scale),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face.as_ptr(),
        )
    }
}

fn init_fonts(scale: i32) {
    let mut guard = FONTS.lock().unwrap();
    if guard.is_some() {
        return;
    }
    *guard = Some(Fonts {
        title: create_font(scale, 34, 700, "Segoe UI"),
        heading: create_font(scale, 21, 600, "Segoe UI"),
        body: create_font(scale, 14, 400, "Segoe UI"),
        small: create_font(scale, 12, 400, "Segoe UI"),
        mono: create_font(scale, 13, 400, "Consolas"),
    });
}

fn font_for(kind: &str) -> HFONT {
    let guard = FONTS.lock().unwrap();
    let fonts = guard.as_ref().copied().unwrap_or(Fonts {
        title: std::ptr::null_mut(),
        heading: std::ptr::null_mut(),
        body: std::ptr::null_mut(),
        small: std::ptr::null_mut(),
        mono: std::ptr::null_mut(),
    });
    match kind {
        "title" => fonts.title,
        "heading" => fonts.heading,
        "small" => fonts.small,
        "mono" => fonts.mono,
        _ => fonts.body,
    }
}

fn fill_round(hdc: HDC, area: RECT, radius: i32, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        let old_brush = SelectObject(hdc, brush as _);
        let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
        RoundRect(hdc, area.left, area.top, area.right, area.bottom, radius, radius);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
        DeleteObject(brush as _);
    }
}

fn stroke_round(hdc: HDC, area: RECT, radius: i32, color: COLORREF) {
    unsafe {
        let pen = CreatePen(PS_SOLID, 1, color);
        let old_pen = SelectObject(hdc, pen as _);
        let old_brush = SelectObject(hdc, GetStockObject(HOLLOW_BRUSH));
        RoundRect(hdc, area.left, area.top, area.right, area.bottom, radius, radius);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        DeleteObject(pen as _);
    }
}

fn draw_text(hdc: HDC, text: &str, mut area: RECT, font: &str, color: COLORREF, flags: u32) {
    if text.is_empty() {
        return;
    }
    let buffer = wide(text);
    unsafe {
        let old_font = SelectObject(hdc, font_for(font) as _);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, color);
        DrawTextW(
            hdc,
            buffer.as_ptr(),
            (buffer.len() - 1) as i32,
            &mut area,
            flags,
        );
        SelectObject(hdc, old_font);
    }
}

fn phase_color(phase: Phase) -> COLORREF {
    match phase {
        Phase::Ready => C_GREEN,
        Phase::Error => C_RED,
        Phase::Stopping | Phase::Stopped => C_MUTED,
        Phase::DatabaseReady => C_GREEN,
        _ => C_AMBER,
    }
}

fn is_busy(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::Initializing
            | Phase::CheckingEnvironment
            | Phase::PreparingDatabase
            | Phase::StartingPostgres
            | Phase::RunningMigrations
            | Phase::CreatingAdministrator
            | Phase::StartingBackend
            | Phase::Stopping
    )
}

fn uptime_text(started_at: &str) -> String {
    let Ok(started) = chrono::DateTime::parse_from_rfc3339(started_at) else {
        return String::new();
    };
    let seconds = (chrono::Local::now() - started.with_timezone(&chrono::Local))
        .num_seconds()
        .max(0);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours} ч {minutes} мин")
    } else {
        format!("{minutes} мин")
    }
}

fn paint(hwnd: HWND) {
    let ctx = GLOBAL.get();
    let state = ctx
        .map(|ctx| ctx.state())
        .unwrap_or_else(crate::state::UiState::default);
    let scale = scale_for(hwnd);
    init_fonts(scale);
    let layout = layout(scale);
    let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    unsafe {
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        let bg = CreateSolidBrush(C_BG);
        FillRect(hdc, &client, bg as _);
        DeleteObject(bg as _);

        draw_text(
            hdc,
            "NEXUS",
            layout.title,
            "title",
            C_ACCENT,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
        draw_text(
            hdc,
            "WORKSPACE",
            layout.subtitle,
            "small",
            C_MUTED,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
        draw_text(
            hdc,
            &format!("portable release {}", state.version),
            layout.version,
            "small",
            C_MUTED,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE,
        );

        fill_round(hdc, layout.card, s(14, scale), C_CARD);
        stroke_round(hdc, layout.card, s(14, scale), C_BORDER);
        let color = phase_color(state.phase);
        let dot_brush = CreateSolidBrush(color);
        let old = SelectObject(hdc, dot_brush as _);
        let r = s(7, scale);
        let pen = CreatePen(PS_SOLID, 1, color);
        let old_pen = SelectObject(hdc, pen as _);
        Ellipse(hdc, layout.dot.0 - r, layout.dot.1 - r, layout.dot.0 + r, layout.dot.1 + r);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old);
        DeleteObject(dot_brush as _);
        DeleteObject(pen as _);

        draw_text(
            hdc,
            state.phase.label(),
            layout.phase,
            "heading",
            C_TEXT,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
        let detail = if state.detail.is_empty() {
            state.phase.label_ru().to_string()
        } else {
            state.detail.clone()
        };
        draw_text(
            hdc,
            &detail,
            layout.detail,
            "body",
            C_MUTED,
            DT_LEFT | DT_TOP | DT_WORDBREAK | DT_END_ELLIPSIS,
        );
        let uptime_line = if state.phase == Phase::Ready {
            uptime_text(&state.started_at)
        } else {
            format!("DB {}", state.db_port)
        };
        draw_text(
            hdc,
            &format!("PORT {}", state.app_port),
            layout.right,
            "mono",
            C_MUTED,
            DT_RIGHT | DT_TOP | DT_SINGLELINE,
        );
        let mut second = layout.right;
        second.top += s(22, scale);
        draw_text(
            hdc,
            &uptime_line,
            second,
            "mono",
            C_MUTED,
            DT_RIGHT | DT_TOP | DT_SINGLELINE,
        );

        fill_round(hdc, layout.info, s(14, scale), C_CARD2);
        stroke_round(hdc, layout.info, s(14, scale), C_BORDER);
        let db_value = if state.database_running {
            format!("PostgreSQL 17 · 127.0.0.1:{}", state.db_port)
        } else {
            "Stopped".to_string()
        };
        let backend_value = if state.backend_running {
            format!("PID {}", state.backend_pid)
        } else {
            "Stopped".to_string()
        };
        let runtime = if state.runtime_included {
            if state.runtime_configured {
                "Included · configured".to_string()
            } else {
                "Included · enrollment required".to_string()
            }
        } else {
            "Not included".to_string()
        };
        let rows = [
            ("NEXUS", if state.phase == Phase::Ready { "Running".to_string() } else { state.phase.label_ru().to_string() }),
            ("PostgreSQL", db_value),
            ("Backend", backend_value),
            ("Address", format!("http://127.0.0.1:{}", state.app_port)),
            ("AI Runtime", runtime),
            ("Admin", if state.admin_present { "Created".to_string() } else { "Not created".to_string() }),
        ];
        for (index, (label, value)) in rows.iter().enumerate() {
            let row = layout.info_rows[index];
            let mut label_rect = row;
            label_rect.right = label_rect.left + s(150, scale);
            let mut value_rect = row;
            value_rect.left = label_rect.right + s(12, scale);
            draw_text(
                hdc,
                label,
                label_rect,
                "small",
                C_MUTED,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE,
            );
            draw_text(
                hdc,
                value,
                value_rect,
                "body",
                C_TEXT,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
        }

        let message = if !state.error.is_empty() {
            Some((state.error.clone(), rgb(60, 20, 26), C_RED))
        } else if !state.warning.is_empty() {
            Some((state.warning.clone(), rgb(52, 42, 18), C_AMBER))
        } else {
            None
        };
        if let Some((text, background, color)) = message {
            fill_round(hdc, layout.message, s(10, scale), background);
            stroke_round(hdc, layout.message, s(10, scale), color);
            let mut area = layout.message;
            area.left += s(14, scale);
            area.right -= s(14, scale);
            area.top += s(8, scale);
            area.bottom -= s(6, scale);
            draw_text(
                hdc,
                &text,
                area,
                "small",
                color,
                DT_LEFT | DT_TOP | DT_WORDBREAK | DT_END_ELLIPSIS,
            );
        }

        let busy = BUSY.load(Ordering::SeqCst) || is_busy(state.phase);
        let hovered = HOVER.load(Ordering::SeqCst);
        for (index, (action, area, primary)) in layout.buttons.iter().enumerate() {
            let enabled = match action {
                Action::Open => !busy,
                Action::Stop => state.backend_running || state.database_running,
                Action::Restart => !busy,
                Action::Logs | Action::Diagnostics | Action::Exit => true,
                Action::Backup => state.database_running && !busy,
            };
            let hover = hovered == index as i32 && enabled;
            let background = if *primary {
                if hover { rgb(120, 225, 255) } else { C_ACCENT }
            } else if hover {
                rgb(30, 47, 72)
            } else {
                rgb(20, 31, 48)
            };
            let border = if *primary { C_ACCENT } else { C_BORDER };
            fill_round(hdc, *area, s(9, scale), background);
            stroke_round(hdc, *area, s(9, scale), border);
            let text_color = if !enabled {
                rgb(80, 95, 112)
            } else if *primary {
                rgb(8, 18, 28)
            } else {
                C_TEXT
            };
            let label = match action {
                Action::Open => {
                    if state.backend_running && state.phase == Phase::Ready {
                        "Открыть NEXUS"
                    } else {
                        "Запустить NEXUS"
                    }
                }
                Action::Stop => "Стоп",
                Action::Restart => "Перезапуск",
                Action::Logs => "Логи",
                Action::Diagnostics => "Диагностика",
                Action::Backup => "Резервная копия",
                Action::Exit => "Выход",
            };
            draw_text(
                hdc,
                label,
                *area,
                if *primary { "heading" } else { "body" },
                text_color,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }
        draw_text(
            hdc,
            &format!("v{}", state.version),
            layout.footer,
            "small",
            C_MUTED,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE,
        );

        EndPaint(hwnd, &ps);
    }
}

fn hit_test(hwnd: HWND, x: i32, y: i32) -> Option<(usize, Action)> {
    let scale = scale_for(hwnd);
    let layout = layout(scale);
    for (index, (action, area, _)) in layout.buttons.iter().enumerate() {
        if x >= area.left && x <= area.right && y >= area.top && y <= area.bottom {
            return Some((index, *action));
        }
    }
    None
}

fn run_action(action: Action) {
    let Some(ctx) = GLOBAL.get().cloned() else {
        return;
    };
    let state = ctx.state();
    match action {
        Action::Open => {
            if state.backend_running && state.phase == Phase::Ready {
                if !open_external(&format!("http://127.0.0.1:{}", state.app_port)) {
                    info_dialog(
                        "NEXUS",
                        &format!(
                            "Не удалось открыть браузер. Откройте вручную:\nhttp://127.0.0.1:{}",
                            state.app_port
                        ),
                    );
                }
            } else {
                spawn_start(&ctx, true);
            }
        }
        Action::Logs => {
            let _ = std::fs::create_dir_all(&ctx.paths.logs);
            if !open_external(&ctx.paths.logs.to_string_lossy()) {
                info_dialog("NEXUS", &format!("Каталог журналов: {}", ctx.paths.logs.display()));
            }
        }
        Action::Exit => request_exit(),
        Action::Stop => spawn_worker(&ctx, Worker::Stop),
        Action::Restart => spawn_worker(&ctx, Worker::Restart),
        Action::Backup => spawn_worker(&ctx, Worker::Backup),
        Action::Diagnostics => spawn_diagnostics(&ctx),
    }
}

#[derive(Clone, Copy)]
enum Worker {
    StopAndExit,
    Stop,
    Restart,
    Backup,
}

fn spawn_worker(ctx: &Ctx, worker: Worker) {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = match worker {
            Worker::StopAndExit | Worker::Stop => setup::stop_all(&ctx),
            Worker::Restart => setup::stop_all(&ctx).and_then(|_| setup::start_all(&ctx, true, true)),
            Worker::Backup => backup_with_pause(&ctx),
        };
        if let Err(error) = result {
            ctx.set_error(format!("{error:#}"));
        }
        BUSY.store(false, Ordering::SeqCst);
        if matches!(worker, Worker::StopAndExit) {
            post(hwnd_of(&ctx), WM_APP_EXIT);
        } else {
            post(hwnd_of(&ctx), WM_APP_UPDATE);
        }
    });
}

fn backup_with_pause(ctx: &Ctx) -> anyhow::Result<()> {
    let state = ctx.state();
    let was_running = state.backend_running;
    if was_running {
        setup::stop_backend(ctx)?;
    }
    let result = setup::database_backup(ctx);
    if was_running {
        let prepared = setup::load_config(ctx)?;
        setup::start_backend(ctx, &prepared.instance, &prepared.secrets)?;
    }
    result.map(|_| ())
}

fn spawn_start(ctx: &Ctx, open_browser: bool) {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        if let Err(error) = setup::start_all(&ctx, true, open_browser) {
            ctx.set_error(format!("{error:#}"));
        }
        BUSY.store(false, Ordering::SeqCst);
        post(hwnd_of(&ctx), WM_APP_UPDATE);
    });
}

fn spawn_diagnostics(ctx: &Ctx) {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        match crate::diagnostics::run(&ctx) {
            Ok(report) => {
                let _ = write_text(&ctx.paths.logs.join("diagnostics-latest.txt"), &report);
                let path = ctx.paths.logs.join("diagnostics-latest.txt");
                if !open_external(&path.to_string_lossy()) {
                    info_dialog("NEXUS — диагностика", &report);
                }
                ctx.update(|state| {
                    state.detail = "Диагностика завершена: logs/diagnostics-latest.txt".into();
                    state.phase = if state.backend_running { Phase::Ready } else { state.phase };
                });
            }
            Err(error) => ctx.update(|state| {
                state.warning = format!("Диагностика: {error:#}");
            }),
        }
        BUSY.store(false, Ordering::SeqCst);
        post(hwnd_of(&ctx), WM_APP_UPDATE);
    });
}

fn hwnd_of(ctx: &Ctx) -> HWND {
    ctx.hwnd as HWND
}

fn post(hwnd: HWND, message: u32) {
    if !hwnd.is_null() {
        unsafe {
            PostMessageW(hwnd, message, 0, 0);
        }
    }
}

fn request_exit() {
    let Some(ctx) = GLOBAL.get() else {
        std::process::exit(0);
    };
    let state = ctx.state();
    if state.backend_running || state.database_running {
        let answer = unsafe {
            MessageBoxW(
                hwnd_of(ctx),
                wide("Остановить NEXUS и выйти?").as_ptr(),
                wide("NEXUS").as_ptr(),
                MB_YESNOCANCEL | MB_ICONQUESTION,
            )
        };
        if answer == IDCANCEL {
            return;
        }
        if answer == IDYES {
            spawn_worker(ctx, Worker::StopAndExit);
            return;
        }
    }
    unsafe {
        DestroyWindow(hwnd_of(ctx));
    }
}

fn handle_requests(_hwnd: HWND) {
    let Some(ctx) = GLOBAL.get().cloned() else {
        return;
    };
    if ctx.paths.stop_request().exists() {
        let _ = std::fs::remove_file(ctx.paths.stop_request());
        spawn_worker(&ctx, Worker::StopAndExit);
    }
    if ctx.paths.restart_request().exists() {
        let _ = std::fs::remove_file(ctx.paths.restart_request());
        spawn_worker(&ctx, Worker::Restart);
    }
    if ctx.paths.open_request().exists() {
        let _ = std::fs::remove_file(ctx.paths.open_request());
        let state = ctx.state();
        if state.app_port != 0 {
            open_external(&format!("http://127.0.0.1:{}", state.app_port));
        }
    }
    if ctx.paths.backup_request().exists() {
        let _ = std::fs::remove_file(ctx.paths.backup_request());
        spawn_worker(&ctx, Worker::Backup);
    }
}

fn timer_tick(hwnd: HWND) {
    handle_requests(hwnd);
    let Some(ctx) = GLOBAL.get().cloned() else {
        return;
    };
    let tick = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    let state = ctx.state();
    if state.phase == Phase::Ready
        && state.backend_pid != 0
        && !setup::backend_process_alive(&ctx.paths, state.backend_pid)
    {
        append_log(&ctx.paths.launcher_log, "backend завершился неожиданно");
        ctx.set_error(
            "Backend неожиданно завершился. Откройте логи и нажмите «Перезапуск».",
        );
    }
    if tick % 10 == 0 {
        let db_running = setup::database_process_running(&ctx.paths);
        if db_running != state.database_running {
            ctx.update(|state| state.database_running = db_running);
        }
        if state.phase == Phase::Ready && !db_running {
            append_log(&ctx.paths.launcher_log, "PostgreSQL завершился неожиданно");
            ctx.set_error(
                "PostgreSQL неожиданно остановился. Откройте логи и нажмите «Перезапуск».",
            );
        }
    }
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CREATE => {
            init_fonts(scale_for(hwnd));
            SetTimer(hwnd, TIMER_ID, 500, None);
            0
        }
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_TIMER if wparam == TIMER_ID => {
            timer_tick(hwnd);
            0
        }
        WM_APP_UPDATE => {
            unsafe {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_APP_EXIT => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_MOUSEMOVE => {
            let x = (lparam & 0xffff) as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i16 as i32;
            let index = hit_test(hwnd, x, y)
                .map(|(index, _)| index as i32)
                .unwrap_or(-1);
            if index != HOVER.load(Ordering::SeqCst) {
                HOVER.store(index, Ordering::SeqCst);
                unsafe {
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            let cursor = if index >= 0 {
                LoadCursorW(std::ptr::null_mut(), IDC_HAND)
            } else {
                LoadCursorW(std::ptr::null_mut(), IDC_ARROW)
            };
            SetCursor(cursor);
            0
        }
        WM_LBUTTONUP => {
            let x = (lparam & 0xffff) as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i16 as i32;
            if let Some((_, action)) = hit_test(hwnd, x, y) {
                run_action(action);
            }
            0
        }
        WM_CLOSE => {
            request_exit();
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_ID);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

pub fn info_dialog(title: &str, message: &str) {
    let title = wide(title);
    let message = wide(message);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
        );
    }
}

pub fn error_dialog(title: &str, message: &str) {
    let title = wide(title);
    let message = wide(message);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

pub fn bring_to_front() {
    let class = wide(CLASS_NAME);
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
}

pub fn run_gui(ctx: Ctx) -> i32 {
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide(CLASS_NAME);
        let title = wide("NEXUS Workspace");
        let cursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        let icon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
        let window_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: icon,
            hCursor: cursor,
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&window_class);

        let scale = {
            let screen = GetDC(std::ptr::null_mut());
            let dpi = if screen.is_null() {
                96
            } else {
                let value = GetDeviceCaps(screen, LOGPIXELSX as i32);
                ReleaseDC(std::ptr::null_mut(), screen);
                value
            };
            if dpi == 0 { 96 } else { dpi }
        };
        let client_w = s(680, scale);
        let client_h = s(566, scale);
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: client_w,
            bottom: client_h,
        };
        AdjustWindowRectEx(&mut frame, WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX, 0, 0);
        let width = frame.right - frame.left;
        let height = frame.bottom - frame.top;
        let x = (screen_w - width) / 2;
        let y = (screen_h - height) / 2;
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            x.max(0),
            y.max(0),
            width,
            height,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            error_dialog("NEXUS", "Не удалось создать окно лаунчера.");
            return 1;
        }
        let _ = GLOBAL.set(ctx.with_hwnd(hwnd as isize));
        ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        spawn_start(GLOBAL.get().unwrap(), true);

        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        0
    }
}
