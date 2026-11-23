use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sysinfo::{Disks, Pid, System};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub removable: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreLoad {
    pub name: String,
    pub usage: f32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub captured_at: DateTime<Utc>,
    pub cpu_percent: f32,
    pub cpu_cores: usize,
    pub per_core: Vec<CoreLoad>,
    pub memory_total: u64,
    pub memory_used: u64,
    pub memory_available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub process_memory: u64,
    pub network_received: u64,
    pub network_transmitted: u64,
    pub network_rx_rate: f64,
    pub network_tx_rate: f64,
    /// Скорость чтения процесса NEXUS (байт/с) — sysinfo не даёт per-disk IO.
    pub process_read_bytes_per_sec: f64,
    /// Скорость записи процесса NEXUS (байт/с).
    pub process_write_bytes_per_sec: f64,
    pub load_average: [f64; 3],
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub kernel_version: String,
    pub disks: Vec<DiskInfo>,
}

impl Default for MetricsSnapshot {
    fn default() -> Self {
        Self {
            captured_at: Utc::now(),
            cpu_percent: 0.0,
            cpu_cores: 0,
            per_core: Vec::new(),
            memory_total: 0,
            memory_used: 0,
            memory_available: 0,
            swap_total: 0,
            swap_used: 0,
            process_memory: 0,
            network_received: 0,
            network_transmitted: 0,
            network_rx_rate: 0.0,
            network_tx_rate: 0.0,
            process_read_bytes_per_sec: 0.0,
            process_write_bytes_per_sec: 0.0,
            load_average: [0.0; 3],
            hostname: "unknown".into(),
            os_name: "unknown".into(),
            os_version: String::new(),
            kernel_version: String::new(),
            disks: Vec::new(),
        }
    }
}

/// Кэш реальных системных показателей. Обновляется отдельным потоком,
/// чтобы не блокировать Tokio runtime и не опрашивать систему на каждый запрос.
#[derive(Clone)]
pub struct MetricsService {
    snapshot: Arc<RwLock<MetricsSnapshot>>,
}

impl Default for MetricsService {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsService {
    pub fn new() -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(MetricsSnapshot::default())),
        }
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        self.snapshot
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn publish(&self, snapshot: MetricsSnapshot) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = snapshot;
        }
    }
}

pub fn spawn_collector(service: MetricsService, db: sqlx::PgPool) {
    let spawn_result = std::thread::Builder::new()
        .name("nexus-metrics".into())
        .spawn(move || {
            let pid = Pid::from_u32(std::process::id());
            let mut system = System::new_all();
            let mut previous_net: Option<(u64, u64, std::time::Instant)> = None;
            let mut previous_io: Option<(u64, u64, std::time::Instant)> = None;
            let mut tick: u64 = 0;
            // Собственный однопоточный runtime для записи истории в БД.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok();
            loop {
                system.refresh_all();
                let disks = Disks::new_with_refreshed_list();
                let networks = sysinfo::Networks::new_with_refreshed_list();
                let disk_infos = disks
                    .list()
                    .iter()
                    .map(|disk| DiskInfo {
                        name: disk.name().to_string_lossy().to_string(),
                        mount_point: disk.mount_point().to_string_lossy().to_string(),
                        file_system: disk.file_system().to_string_lossy().to_string(),
                        total_bytes: disk.total_space(),
                        available_bytes: disk.available_space(),
                        removable: disk.is_removable(),
                    })
                    .collect();

                let per_core = system
                    .cpus()
                    .iter()
                    .enumerate()
                    .map(|(index, cpu)| CoreLoad {
                        name: index.to_string(),
                        usage: cpu.cpu_usage(),
                    })
                    .collect::<Vec<_>>();

                let (mut rx_total, mut tx_total) = (0u64, 0u64);
                for data in networks.list().values() {
                    rx_total += data.total_received();
                    tx_total += data.total_transmitted();
                }
                let now = std::time::Instant::now();
                let (rx_rate, tx_rate) = match previous_net {
                    Some((prev_rx, prev_tx, prev_at)) => {
                        let elapsed = now.duration_since(prev_at).as_secs_f64().max(0.001);
                        (
                            (rx_total.saturating_sub(prev_rx)) as f64 / elapsed,
                            (tx_total.saturating_sub(prev_tx)) as f64 / elapsed,
                        )
                    }
                    None => (0.0, 0.0),
                };
                previous_net = Some((rx_total, tx_total, now));

                // Ввод-вывод процесса: sysinfo даёт его только для процесса целиком.
                let (read_total, write_total) = system
                    .process(pid)
                    .map(|process| {
                        let usage = process.disk_usage();
                        (usage.read_bytes, usage.written_bytes)
                    })
                    .unwrap_or((0, 0));
                let (read_rate, write_rate) = match previous_io {
                    Some((prev_read, prev_write, prev_at)) => {
                        let elapsed = now.duration_since(prev_at).as_secs_f64().max(0.001);
                        (
                            (read_total.saturating_sub(prev_read)) as f64 / elapsed,
                            (write_total.saturating_sub(prev_write)) as f64 / elapsed,
                        )
                    }
                    None => (0.0, 0.0),
                };
                previous_io = Some((read_total, write_total, now));

                let load = System::load_average();
                let snapshot = MetricsSnapshot {
                    captured_at: Utc::now(),
                    cpu_percent: system.global_cpu_usage(),
                    cpu_cores: system.cpus().len(),
                    per_core,
                    memory_total: system.total_memory(),
                    memory_used: system.used_memory(),
                    memory_available: system.available_memory(),
                    swap_total: system.total_swap(),
                    swap_used: system.used_swap(),
                    process_memory: system.process(pid).map(|p| p.memory()).unwrap_or(0),
                    network_received: rx_total,
                    network_transmitted: tx_total,
                    network_rx_rate: rx_rate,
                    network_tx_rate: tx_rate,
                    process_read_bytes_per_sec: read_rate,
                    process_write_bytes_per_sec: write_rate,
                    load_average: [load.one, load.five, load.fifteen],
                    hostname: System::host_name().unwrap_or_default(),
                    os_name: System::name().unwrap_or_default(),
                    os_version: System::long_os_version().unwrap_or_default(),
                    kernel_version: System::kernel_version().unwrap_or_default(),
                    disks: disk_infos,
                };
                service.publish(snapshot.clone());

                // Историю пишем раз в 10 секунд, чтобы не разрастаться; хранение — 7 дней.
                tick += 1;
                if tick % 5 == 0 {
                    if let Some(runtime) = runtime.as_ref() {
                        runtime.block_on(persist_snapshot(&db, &snapshot));
                    }
                }

                std::thread::sleep(Duration::from_secs(2));
            }
        });

    if let Err(error) = spawn_result {
        tracing::error!(%error, "не удалось запустить сборщик метрик");
    }
}

/// Публикует снимок метрик в realtime-канал раз в 5 секунд.
pub fn spawn_realtime_broadcaster(state: std::sync::Arc<crate::state::AppState>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Первый тик интервала срабатывает сразу.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if state.realtime.active() == 0 {
                continue;
            }
            let snapshot = state.metrics.snapshot();
            let stats = state.requests.snapshot();
            state.realtime.publish_system(
                "system.metrics.updated",
                serde_json::json!({
                    "cpu": snapshot.cpu_percent,
                    "memoryUsed": snapshot.memory_used,
                    "memoryTotal": snapshot.memory_total,
                    "rxRate": snapshot.network_rx_rate,
                    "txRate": snapshot.network_tx_rate,
                    "requestsPerSecond": stats.requests_per_second,
                    "activeRequests": state.requests.active_connections(),
                    "activeDownloads": state.requests.active_downloads(),
                    "activeWebSockets": state.realtime.active(),
                }),
            );
        }
    });
}

/// Сохраняет точку истории и чистит записи старше 7 дней.
async fn persist_snapshot(db: &sqlx::PgPool, snapshot: &MetricsSnapshot) {
    let samples: Vec<(&str, f64)> = vec![
        ("cpu.percent", snapshot.cpu_percent as f64),
        ("memory.used", snapshot.memory_used as f64),
        ("memory.total", snapshot.memory_total as f64),
        ("process.memory", snapshot.process_memory as f64),
        ("network.rx_rate", snapshot.network_rx_rate),
        ("network.tx_rate", snapshot.network_tx_rate),
        ("network.rx_total", snapshot.network_received as f64),
        ("network.tx_total", snapshot.network_transmitted as f64),
        ("process.read_rate", snapshot.process_read_bytes_per_sec),
        ("process.write_rate", snapshot.process_write_bytes_per_sec),
        ("swap.used", snapshot.swap_used as f64),
    ];
    for (metric, value) in samples {
        let result = sqlx::query(
            "insert into system_metrics (captured_at, metric, value) values ($1, $2, $3)",
        )
        .bind(snapshot.captured_at)
        .bind(metric)
        .bind(value)
        .execute(db)
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, metric, "не удалось сохранить метрику");
            return;
        }
    }

    let cleanup =
        sqlx::query("delete from system_metrics where captured_at < now() - interval '7 days'")
            .execute(db)
            .await;
    if let Err(error) = cleanup {
        tracing::warn!(%error, "не удалось очистить старые метрики");
    }
}
