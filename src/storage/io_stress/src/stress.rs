// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result};
pub use fidl_fuchsia_memorypressure::Level as MemoryLevel;
use fidl_fuchsia_memorypressure::{ProviderMarker, WatcherMarker, WatcherRequest};
use fuchsia_component::client::connect_to_protocol;
use futures::StreamExt as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

pub struct CpuStressor {
    stop_signal: Arc<AtomicBool>,
    threads: Vec<thread::JoinHandle<()>>,
}

impl CpuStressor {
    pub fn start(num_cores: usize) -> Self {
        log::info!("Spawning {} background CPU burn threads...", num_cores);
        let stop_signal = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();

        for i in 0..num_cores {
            let stop = stop_signal.clone();
            threads.push(thread::spawn(move || {
                log::debug!("CPU burn thread {} started.", i);
                while !stop.load(Ordering::Relaxed) {
                    let mut _x = 0;
                    for _ in 0..1000 {
                        _x = std::hint::black_box(_x + 1);
                    }
                }
                log::debug!("CPU burn thread {} stopped.", i);
            }));
        }

        Self { stop_signal, threads }
    }
}

impl Drop for CpuStressor {
    fn drop(&mut self) {
        log::info!("Stopping background CPU burn threads...");
        self.stop_signal.store(true, Ordering::Relaxed);
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
        log::info!("All CPU burn threads stopped.");
    }
}

pub fn set_memory_pressure(level: fidl_fuchsia_memorypressure::Level) -> Result<()> {
    log::info!("Signaling memory pressure level to target: {:?}", level);
    let debug_pressure = match connect_to_protocol::<fidl_fuchsia_memory_debug::MemoryPressureMarker>(
    ) {
        Ok(p) => p,
        Err(e) => {
            log::warn!(
                "Could not connect to fuchsia.memory.debug.MemoryPressure (routing might be missing): {:?}",
                e
            );
            return Ok(());
        }
    };
    if let Err(e) = debug_pressure.signal(level) {
        log::warn!("Failed to signal memory pressure (PEER_CLOSED?): {:?}", e);
    }
    Ok(())
}

pub struct MemoryPressureGuard;

impl MemoryPressureGuard {
    pub fn new(level: fidl_fuchsia_memorypressure::Level) -> Result<Self> {
        set_memory_pressure(level)?;
        Ok(Self)
    }
}

impl Drop for MemoryPressureGuard {
    fn drop(&mut self) {
        if let Err(e) = set_memory_pressure(fidl_fuchsia_memorypressure::Level::Normal) {
            log::error!("Failed to reset memory pressure to Normal on drop: {:?}", e);
        }
    }
}

pub async fn get_cpu_load() -> Result<f32> {
    let stats = connect_to_protocol::<fidl_fuchsia_kernel::StatsMarker>()
        .context("Failed to connect to fuchsia.kernel.Stats")?;
    let duration = zx::MonotonicDuration::from_millis(200).into_nanos();
    let load_vec = stats.get_cpu_load(duration).await.context("FIDL call to GetCpuLoad failed")?;
    if load_vec.is_empty() {
        return Ok(0.0);
    }
    let total_load: f32 = load_vec.iter().sum();
    Ok(total_load / load_vec.len() as f32)
}

pub async fn wait_for_cooldown(
    stats: &MemoryPressureStats,
    max_wait: std::time::Duration,
) -> Result<()> {
    log::info!("Starting dynamic cooldown... max wait: {:?}", max_wait);
    let start = std::time::Instant::now();
    let check_interval = std::time::Duration::from_secs(1);
    let mut last_cpu_load = 0.0;

    while start.elapsed() < max_wait {
        let mem_level = stats.current_level();
        let cpu_load = match get_cpu_load().await {
            Ok(load) => load,
            Err(e) => {
                log::warn!("Failed to query CPU load: {:?}", e);
                0.0
            }
        };
        last_cpu_load = cpu_load;

        log::info!(
            "Cooldown check: mem_level = {:?}, cpu_load = {:.2}% (elapsed: {:?})",
            mem_level,
            cpu_load,
            start.elapsed()
        );

        if mem_level == fidl_fuchsia_memorypressure::Level::Normal && cpu_load < 40.0 {
            log::info!("System settled after {:?}", start.elapsed());
            return Ok(());
        }

        fuchsia_async::Timer::new(check_interval).await;
    }

    anyhow::bail!(
        "System failed to settle within {:?}. Final state: mem_level={:?}, cpu_load={:.2}%",
        max_wait,
        stats.current_level(),
        last_cpu_load
    );
}

use std::sync::Mutex;

pub struct MemoryPressureStats {
    inner: Mutex<MemoryPressureStatsInner>,
}

struct MemoryPressureStatsInner {
    max_level: MemoryLevel,
    current_level: MemoryLevel,
}

impl MemoryPressureStats {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MemoryPressureStatsInner {
                max_level: MemoryLevel::Normal,
                current_level: MemoryLevel::Normal,
            }),
        }
    }

    pub fn update(&self, new_level: MemoryLevel) {
        let mut inner = self.inner.lock().unwrap();
        inner.max_level = std::cmp::max(inner.max_level, new_level);
        inner.current_level = new_level;
    }

    pub fn max_level(&self) -> MemoryLevel {
        self.inner.lock().unwrap().max_level
    }

    pub fn current_level(&self) -> MemoryLevel {
        self.inner.lock().unwrap().current_level
    }

    pub fn reset(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.max_level = MemoryLevel::Normal;
        inner.current_level = MemoryLevel::Normal;
    }
}

pub async fn watch_memory_pressure(stats: Arc<MemoryPressureStats>) -> anyhow::Result<()> {
    let provider = connect_to_protocol::<ProviderMarker>()
        .context("Failed to connect to fuchsia.memorypressure.Provider")?;

    let (watcher_client, mut watcher_requests) =
        fidl::endpoints::create_request_stream::<WatcherMarker>();
    provider
        .register_watcher(watcher_client)
        .context("Failed to register watcher with MemoryPressure Provider")?;

    log::info!("Registered as a fuchsia.memorypressure/Watcher");

    while let Some(request) = watcher_requests.next().await {
        match request {
            Ok(WatcherRequest::OnLevelChanged { level, responder }) => {
                log::info!("Memory pressure level changed to: {:?}", level);
                stats.update(level);
                if let Err(e) = responder.send() {
                    log::error!("Failed to respond to memory pressure event: {:?}", e);
                    break;
                }
            }
            Err(e) => {
                log::error!("Error reading memory pressure watcher stream: {:?}", e);
                break;
            }
        }
    }
    Ok(())
}
