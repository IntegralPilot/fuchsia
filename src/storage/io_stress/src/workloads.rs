// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::stress::MemoryPressureStats;
use anyhow::Context as _;
use argh::FromArgs;
use fidl_fuchsia_io as fio;
use fuchsia_trace as trace;
use fuchsiaperf::{Direction, FuchsiaPerfBenchmarkResult, Unit};
use rand::RngExt as _;
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub const KB: u64 = 1024;
pub const MB: u64 = 1024 * KB;
pub const PAGE_SIZE: u64 = 4096;

pub const BURST_IO_PREALLOC_SIZE: u64 = 55 * MB;

pub const NS_PER_MS: f64 = 1_000_000.0;

pub const RANDOM_IO_WRITE_PATTERN: u8 = 0xBB;
pub const BURST_IO_WRITE_PATTERN: u8 = 0xCC;

#[derive(Default, Debug, Clone)]
pub struct Metrics {
    pub read_ops: u64,
    pub write_ops: u64,
    pub fsync_ops: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub max_read_latency_ns: u64,
    pub max_write_latency_ns: u64,
    pub max_fsync_latency_ns: u64,
    pub total_write_latency_ns: u64,
    pub total_read_latency_ns: u64,
    pub total_fsync_latency_ns: u64,
    pub op_latencies_ns: Vec<u64>,
    pub fsync_latencies_ns: Vec<u64>,
}

impl Metrics {
    pub fn record_read(&mut self, bytes_read: u64, elapsed_ns: u64) {
        self.read_ops += 1;
        self.read_bytes += bytes_read;
        self.max_read_latency_ns = std::cmp::max(self.max_read_latency_ns, elapsed_ns);
        self.total_read_latency_ns += elapsed_ns;
        self.op_latencies_ns.push(elapsed_ns);
    }

    pub fn record_write(&mut self, bytes_written: u64, elapsed_ns: u64) {
        self.write_ops += 1;
        self.write_bytes += bytes_written;
        self.max_write_latency_ns = std::cmp::max(self.max_write_latency_ns, elapsed_ns);
        self.total_write_latency_ns += elapsed_ns;
        self.op_latencies_ns.push(elapsed_ns);
    }

    pub fn record_fsync(&mut self, elapsed_ns: u64) {
        self.fsync_ops += 1;
        self.max_fsync_latency_ns = std::cmp::max(self.max_fsync_latency_ns, elapsed_ns);
        self.total_fsync_latency_ns += elapsed_ns;
        self.fsync_latencies_ns.push(elapsed_ns);
    }

    pub fn merge(&mut self, other: &Metrics) {
        self.read_ops += other.read_ops;
        self.write_ops += other.write_ops;
        self.fsync_ops += other.fsync_ops;
        self.read_bytes += other.read_bytes;
        self.write_bytes += other.write_bytes;
        self.max_read_latency_ns =
            std::cmp::max(self.max_read_latency_ns, other.max_read_latency_ns);
        self.max_write_latency_ns =
            std::cmp::max(self.max_write_latency_ns, other.max_write_latency_ns);
        self.max_fsync_latency_ns =
            std::cmp::max(self.max_fsync_latency_ns, other.max_fsync_latency_ns);
        self.total_write_latency_ns += other.total_write_latency_ns;
        self.total_read_latency_ns += other.total_read_latency_ns;
        self.total_fsync_latency_ns += other.total_fsync_latency_ns;
        self.op_latencies_ns.extend_from_slice(&other.op_latencies_ns);
        self.fsync_latencies_ns.extend_from_slice(&other.fsync_latencies_ns);
    }
}

struct Timer {
    start: zx::MonotonicInstant,
}

impl Timer {
    fn start() -> Self {
        Self { start: zx::MonotonicInstant::get() }
    }

    fn elapsed_ns(&self) -> u64 {
        (zx::MonotonicInstant::get() - self.start).into_nanos() as u64
    }
}

fn do_fsync(file_proxy: &fio::FileSynchronousProxy, metrics: &mut Metrics) -> anyhow::Result<()> {
    let timer = Timer::start();
    trace::duration!("benchmark", "fsync");
    let deadline = zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(30));
    file_proxy.sync(deadline).context("FIDL error on fsync")?.map_err(|status| {
        anyhow::anyhow!("fsync status: {:?}", zx::Status::err_from_raw(status))
    })?;
    metrics.record_fsync(timer.elapsed_ns());
    Ok(())
}

struct RateLimiter {
    start_run: Instant,
    rate_mibs: u64,
}

impl RateLimiter {
    fn new(rate_mibs: u64) -> Self {
        Self { start_run: Instant::now(), rate_mibs }
    }

    fn sleep_if_needed(&mut self, total_bytes_processed: u64) {
        if self.rate_mibs > 0 {
            let target_duration = Duration::from_secs_f64(
                total_bytes_processed as f64 / (self.rate_mibs * 1024 * 1024) as f64,
            );
            let elapsed = self.start_run.elapsed();
            if elapsed < target_duration {
                thread::sleep(target_duration - elapsed);
            } else if elapsed > target_duration + Duration::from_millis(50) {
                self.start_run = Instant::now() - target_duration;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_random(
    vmo: zx::Vmo,
    file_proxy: &fio::FileSynchronousProxy,
    op_size_bytes: usize,
    file_size_bytes: u64,
    read_percentage: u32,
    fsync_every_n_ops: u64,
    rate_mibs: u64,
    seed: u64,
    stop_signal: Arc<AtomicBool>,
    metrics: &mut Metrics,
) -> anyhow::Result<()> {
    use rand::SeedableRng as _;
    let mut rng = if seed == 0 {
        rand::rngs::StdRng::from_rng(&mut rand::rng())
    } else {
        rand::rngs::StdRng::seed_from_u64(seed)
    };
    let mut op_count = 0;
    let mut rate_limiter = RateLimiter::new(rate_mibs);
    let mut total_bytes_processed = 0u64;
    let mut io_buffer = vec![0u8; op_size_bytes];

    while !stop_signal.load(Ordering::Relaxed) {
        let range = file_size_bytes.saturating_sub(op_size_bytes as u64);
        let max_page_index = range / PAGE_SIZE;
        let mut offset =
            if max_page_index == 0 { 0 } else { rng.random_range(0..=max_page_index) * PAGE_SIZE };
        let is_read = rng.random_range(0..100) < read_percentage;
        let buf_slice = &mut io_buffer[..op_size_bytes];

        if offset + op_size_bytes as u64 > file_size_bytes {
            offset = 0;
        }

        let timer = Timer::start();
        if is_read {
            trace::duration!(
                "benchmark",
                "vmo_read",
                "bytes" => op_size_bytes as u64,
                "offset" => offset
            );
            vmo.read(buf_slice, offset).context("VMO read failed")?;
            metrics.record_read(op_size_bytes as u64, timer.elapsed_ns());
        } else {
            buf_slice.fill(RANDOM_IO_WRITE_PATTERN);
            trace::duration!(
                "benchmark",
                "vmo_write",
                "bytes" => op_size_bytes as u64,
                "offset" => offset
            );
            vmo.write(buf_slice, offset).context("VMO write failed")?;
            metrics.record_write(op_size_bytes as u64, timer.elapsed_ns());
        }

        let is_fsync = !is_read && fsync_every_n_ops > 0 && op_count % fsync_every_n_ops == 0;
        if is_fsync {
            do_fsync(file_proxy, metrics)?;
        }

        if !is_read {
            op_count += 1;
        }

        total_bytes_processed += op_size_bytes as u64;
        rate_limiter.sleep_if_needed(total_bytes_processed);
    }

    if fsync_every_n_ops > 0 {
        do_fsync(file_proxy, metrics)?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_sequential(
    zx_vmo: zx::Vmo,
    file_proxy: &fio::FileSynchronousProxy,
    op_size_bytes: usize,
    file_size_bytes: u64,
    rate_mibs: u64,
    fsync_every_n_ops: u64,
    read: bool,
    stop_signal: Arc<AtomicBool>,
    metrics: &mut Metrics,
) -> anyhow::Result<()> {
    let mut io_buffer = vec![0u8; op_size_bytes];
    if !read {
        io_buffer.fill(0x99);
    }

    let mut offset = 0;
    let mut op_count = 0;
    let mut rate_limiter = RateLimiter::new(rate_mibs);
    let mut total_bytes_processed = 0u64;

    while !stop_signal.load(Ordering::Relaxed) {
        if offset + op_size_bytes as u64 > file_size_bytes {
            if !read && fsync_every_n_ops == 0 {
                do_fsync(file_proxy, metrics)?;
            }
            offset = 0;
        }

        let timer = Timer::start();
        if read {
            trace::duration!(
                "benchmark",
                "vmo_read",
                "bytes" => op_size_bytes as u64,
                "offset" => offset
            );
            zx_vmo.read(&mut io_buffer, offset).context("Sequential VMO read failed")?;
            metrics.record_read(op_size_bytes as u64, timer.elapsed_ns());
        } else {
            trace::duration!(
                "benchmark",
                "vmo_write",
                "bytes" => op_size_bytes as u64,
                "offset" => offset
            );
            zx_vmo.write(&io_buffer, offset).context("Sequential VMO write failed")?;
            metrics.record_write(op_size_bytes as u64, timer.elapsed_ns());
        }

        let is_fsync = !read && fsync_every_n_ops > 0 && op_count % fsync_every_n_ops == 0;
        if is_fsync {
            do_fsync(file_proxy, metrics)?;
        }

        if !read {
            op_count += 1;
        }

        offset += op_size_bytes as u64;
        total_bytes_processed += op_size_bytes as u64;
        rate_limiter.sleep_if_needed(total_bytes_processed);
    }

    if !read {
        do_fsync(file_proxy, metrics)?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_burst(
    vmo: zx::Vmo,
    file_proxy: &fio::FileSynchronousProxy,
    op_size_bytes: usize,
    burst_ops_count: usize,
    sleep_between_bursts_ms: u64,
    periodic_fsync_ms: u64,
    read: bool,
    rate_mibs: u64,
    stop_signal: Arc<AtomicBool>,
    metrics: &mut Metrics,
) -> anyhow::Result<()> {
    let mut io_buffer = vec![0u8; op_size_bytes];
    let buf_slice = &mut io_buffer[..op_size_bytes];
    if !read {
        buf_slice.fill(BURST_IO_WRITE_PATTERN);
    }
    let mut offset = 0;
    let mut last_fsync = Instant::now();
    let mut rate_limiter = RateLimiter::new(rate_mibs);
    let mut total_bytes_processed = 0u64;

    while !stop_signal.load(Ordering::Relaxed) {
        for _ in 0..burst_ops_count {
            if offset + op_size_bytes as u64 > BURST_IO_PREALLOC_SIZE {
                offset = 0;
            }
            let timer = Timer::start();
            if read {
                trace::duration!(
                    "benchmark",
                    "vmo_read",
                    "bytes" => op_size_bytes as u64,
                    "offset" => offset
                );
                vmo.read(buf_slice, offset).context("VMO burst read failed")?;
                metrics.record_read(op_size_bytes as u64, timer.elapsed_ns());
            } else {
                trace::duration!(
                    "benchmark",
                    "vmo_write",
                    "bytes" => op_size_bytes as u64,
                    "offset" => offset
                );
                vmo.write(buf_slice, offset).context("VMO burst write failed")?;
                metrics.record_write(op_size_bytes as u64, timer.elapsed_ns());
            }
            offset += op_size_bytes as u64;
            total_bytes_processed += op_size_bytes as u64;
        }

        if !read {
            let is_fsync = periodic_fsync_ms > 0
                && last_fsync.elapsed().as_millis() >= periodic_fsync_ms as u128;
            if is_fsync {
                do_fsync(file_proxy, metrics)?;
                last_fsync = Instant::now();
            }
        }

        rate_limiter.sleep_if_needed(total_bytes_processed);
        thread::sleep(Duration::from_millis(sleep_between_bursts_ms));
    }

    if !read {
        do_fsync(file_proxy, metrics)?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_transfer(
    source_vmo: zx::Vmo,
    dest_vmo: zx::Vmo,
    dest_proxy: &fio::FileSynchronousProxy,
    op_size_bytes: usize,
    file_size_bytes: u64,
    xor_transform: bool,
    rate_mibs: u64,
    fsync_every_n_ops: u64,
    stop_signal: Arc<AtomicBool>,
    metrics: &mut Metrics,
) -> anyhow::Result<()> {
    let mut op_count = 0;
    let mut rate_limiter = RateLimiter::new(rate_mibs);
    let mut total_bytes_processed = 0u64;
    let mut io_buffer = vec![0u8; op_size_bytes];

    while !stop_signal.load(Ordering::Relaxed) {
        let offset = (op_count as u64 * op_size_bytes as u64) % file_size_bytes;

        let read_timer = Timer::start();
        source_vmo.read(&mut io_buffer, offset).context("Failed to read from source VMO")?;
        metrics.record_read(op_size_bytes as u64, read_timer.elapsed_ns());

        if xor_transform {
            for b in &mut io_buffer {
                *b ^= 0x55;
            }
        }

        let write_timer = Timer::start();
        dest_vmo.write(&io_buffer, offset).context("Failed to write to dest VMO")?;
        metrics.record_write(op_size_bytes as u64, write_timer.elapsed_ns());

        op_count += 1;
        total_bytes_processed += op_size_bytes as u64;
        rate_limiter.sleep_if_needed(total_bytes_processed);

        if fsync_every_n_ops > 0 && op_count % fsync_every_n_ops == 0 {
            do_fsync(dest_proxy, metrics)?;
        }
    }

    if fsync_every_n_ops > 0 {
        do_fsync(dest_proxy, metrics)?;
    }

    Ok(())
}

#[derive(FromArgs, Debug, Clone)]
#[argh(subcommand, name = "random")]
/// Run a random write/read workload.
pub struct RandomArgs {
    /// block size in bytes (default: 4096)
    #[argh(option, default = "4096")]
    pub op_size_bytes: u64,

    /// total backing file size in bytes (default: 67108864)
    #[argh(option, default = "67108864")]
    pub file_size_bytes: u64,

    /// percentage of read operations (0 to 100)
    #[argh(option, default = "50")]
    pub read_percentage: u32,

    /// trigger fsync every N operations (default: 0)
    #[argh(option, default = "0")]
    pub fsync_every_n_ops: u64,

    /// rate limit operations in MiB/s (default: 0)
    #[argh(option, default = "0")]
    pub rate_mibs: u64,

    /// RNG seed for reproducibility (default: 0)
    #[argh(option, default = "0")]
    pub seed: u64,
}

#[derive(FromArgs, Debug, Clone)]
#[argh(subcommand, name = "sequential")]
/// Run a sequential write workload.
pub struct SequentialArgs {
    /// block size in bytes (default: 131072)
    #[argh(option, default = "131072")]
    pub op_size_bytes: u64,

    /// total backing file size in bytes (default: 1073741824)
    #[argh(option, default = "1073741824")]
    pub file_size_bytes: u64,

    /// perform sequential read instead of write
    #[argh(switch)]
    pub read: bool,

    /// trigger fsync every N operations (default: 0)
    #[argh(option, default = "0")]
    pub fsync_every_n_ops: u64,

    /// rate limit operations in MiB/s (default: 0)
    #[argh(option, default = "0")]
    pub rate_mibs: u64,
}

#[derive(FromArgs, Debug, Clone)]
#[argh(subcommand, name = "burst")]
/// Run a burst workload.
pub struct BurstArgs {
    /// block size in bytes (default: 4096)
    #[argh(option, default = "4096")]
    pub op_size_bytes: u64,

    /// number of operations in a single burst (default: 1000)
    #[argh(option, default = "1000")]
    pub burst_ops_count: u64,

    /// sleep duration between bursts in ms (default: 100)
    #[argh(option, default = "100")]
    pub sleep_between_bursts_ms: u64,

    /// periodic fsync interval in ms (default: 1000)
    #[argh(option, default = "1000")]
    pub periodic_fsync_ms: u64,

    /// perform burst read instead of write
    #[argh(switch)]
    pub read: bool,

    /// rate limit operations in MiB/s (default: 0)
    #[argh(option, default = "0")]
    pub rate_mibs: u64,
}

#[derive(FromArgs, Debug, Clone)]
#[argh(subcommand, name = "transfer")]
/// Run a dual-file transfer (copy or compress).
pub struct TransferArgs {
    /// block size in bytes (default: 131072)
    #[argh(option, default = "131072")]
    pub op_size_bytes: u64,

    /// total backing file size in bytes (default: 67108864)
    #[argh(option, default = "67108864")]
    pub file_size_bytes: u64,

    /// perform CPU XOR transformation on data before writing
    #[argh(switch)]
    pub xor_transform: bool,

    /// trigger fsync every N operations (default: 0)
    #[argh(option, default = "0")]
    pub fsync_every_n_ops: u64,

    /// rate limit operations in MiB/s (default: 0)
    #[argh(option, default = "0")]
    pub rate_mibs: u64,
}

#[derive(FromArgs, Debug, Clone)]
#[argh(subcommand)]
pub enum WorkloadSubcommand {
    Random(RandomArgs),
    Sequential(SequentialArgs),
    Burst(BurstArgs),
    Transfer(TransferArgs),
}

impl WorkloadSubcommand {
    pub fn name(&self) -> &'static str {
        match self {
            WorkloadSubcommand::Random(_) => "random",
            WorkloadSubcommand::Sequential(_) => "sequential",
            WorkloadSubcommand::Burst(_) => "burst",
            WorkloadSubcommand::Transfer(_) => "transfer",
        }
    }

    pub fn persona(&self) -> &'static str {
        match self {
            WorkloadSubcommand::Random(args) => {
                if args.rate_mibs == 0 {
                    "AppLaunch"
                } else {
                    "Database"
                }
            }
            WorkloadSubcommand::Sequential(args) => {
                if args.fsync_every_n_ops > 0 {
                    "Media"
                } else {
                    "Download"
                }
            }
            WorkloadSubcommand::Transfer(args) => {
                if args.xor_transform {
                    "Compress"
                } else {
                    "Copy"
                }
            }
            WorkloadSubcommand::Burst(_) => "Media",
        }
    }
}

pub fn get_effective_op_size(op_size_bytes: u64) -> u64 {
    if op_size_bytes == 0 { PAGE_SIZE } else { op_size_bytes }
}

pub fn setup_backing_vmo(
    file_path: &Path,
    size_bytes: u64,
) -> anyhow::Result<(fio::FileSynchronousProxy, zx::Vmo)> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(file_path)
        .context("Failed to open or create target file under /data")?;
    if size_bytes > 0 {
        file.set_len(size_bytes).context("Failed to set file size limit")?;
    }

    let zx_channel = fdio::clone_channel(&file).context("Failed to clone fdio channel")?;
    let file_proxy = fio::FileSynchronousProxy::new(zx_channel);
    let vmo_flags = fio::VmoFlags::READ | fio::VmoFlags::WRITE | fio::VmoFlags::SHARED_BUFFER;
    let deadline = zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(30));
    let vmo_handle = file_proxy
        .get_backing_memory(vmo_flags, deadline)
        .context("Failed to get backing memory via FIDL")?
        .map_err(|status| {
            anyhow::anyhow!("get_backing_memory status: {:?}", zx::Status::err_from_raw(status))
        })?;
    Ok((file_proxy, zx::Vmo::from(vmo_handle)))
}

pub struct StopSignalGuard(pub Arc<AtomicBool>);
impl Drop for StopSignalGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub struct FileCleanupGuard {
    pub paths: Vec<String>,
}
impl Drop for FileCleanupGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn calc_percentile(mut vals: Vec<u64>, pct: f64) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    vals.sort_unstable();
    let idx = ((vals.len() as f64 - 1.0) * pct).round() as usize;
    vals[std::cmp::min(idx, vals.len() - 1)] as f64
}

pub async fn run_workloads(
    run_name: &str,
    subcommands: Vec<WorkloadSubcommand>,
    duration_secs: u64,
    mem_stats: Arc<MemoryPressureStats>,
) -> anyhow::Result<Vec<FuchsiaPerfBenchmarkResult>> {
    let stop_signal = Arc::new(AtomicBool::new(false));
    let _stop_guard = StopSignalGuard(stop_signal.clone());
    let mut created_file_paths = Vec::new();

    let _timer_task = if duration_secs > 0 {
        let stop_signal_clone = stop_signal.clone();
        Some(fuchsia_async::Task::spawn(async move {
            fuchsia_async::Timer::new(Duration::from_secs(duration_secs)).await;
            log::info!("Configured global duration reached, signalling stop to all threads.");
            stop_signal_clone.store(true, Ordering::Relaxed);
        }))
    } else {
        None
    };

    let mut thread_handles = Vec::new();
    let mut persona_counters = std::collections::HashMap::new();

    for sub in subcommands {
        let sub_clone = sub.clone();
        let stop_signal = stop_signal.clone();
        let persona = sub.persona();

        let entry = persona_counters.entry(persona).or_insert(0);
        *entry += 1;
        let display_name = format!("{}_{}", persona, entry);

        let handle = match &sub_clone {
            WorkloadSubcommand::Transfer(args) => {
                let src_path_str = format!("/data/stress_target_{}_src", display_name);
                let dest_path_str = format!("/data/stress_target_{}_dest", display_name);
                created_file_paths.push(src_path_str.clone());
                created_file_paths.push(dest_path_str.clone());

                let args_clone = args.clone();
                thread::Builder::new()
                    .name(display_name.clone())
                    .spawn(move || -> anyhow::Result<Metrics> {
                        let mut metrics = Metrics::default();
                        let op_size = get_effective_op_size(args_clone.op_size_bytes);
                        let (_src_proxy, src_vmo) = setup_backing_vmo(
                            Path::new(&src_path_str),
                            args_clone.file_size_bytes,
                        )?;
                        let (dest_proxy, dest_vmo) = setup_backing_vmo(
                            Path::new(&dest_path_str),
                            args_clone.file_size_bytes,
                        )?;

                        run_transfer(
                            src_vmo,
                            dest_vmo,
                            &dest_proxy,
                            op_size as usize,
                            args_clone.file_size_bytes,
                            args_clone.xor_transform,
                            args_clone.rate_mibs,
                            args_clone.fsync_every_n_ops,
                            stop_signal,
                            &mut metrics,
                        )?;
                        Ok(metrics)
                    })
                    .context("Failed to spawn thread")?
            }
            WorkloadSubcommand::Random(args) => {
                let file_path_str = format!("/data/stress_target_{}", display_name);
                created_file_paths.push(file_path_str.clone());

                let args_clone = args.clone();
                thread::Builder::new()
                    .name(display_name.clone())
                    .spawn(move || -> anyhow::Result<Metrics> {
                        let mut metrics = Metrics::default();
                        let file_path = Path::new(&file_path_str);
                        let op_size = get_effective_op_size(args_clone.op_size_bytes);
                        let (file_proxy, zx_vmo) =
                            setup_backing_vmo(&file_path, args_clone.file_size_bytes)?;
                        run_random(
                            zx_vmo,
                            &file_proxy,
                            op_size as usize,
                            args_clone.file_size_bytes,
                            args_clone.read_percentage,
                            args_clone.fsync_every_n_ops,
                            args_clone.rate_mibs,
                            args_clone.seed,
                            stop_signal,
                            &mut metrics,
                        )?;
                        Ok(metrics)
                    })
                    .context("Failed to spawn thread")?
            }
            WorkloadSubcommand::Sequential(args) => {
                let file_path_str = format!("/data/stress_target_{}", display_name);
                created_file_paths.push(file_path_str.clone());

                let args_clone = args.clone();
                thread::Builder::new()
                    .name(display_name.clone())
                    .spawn(move || -> anyhow::Result<Metrics> {
                        let mut metrics = Metrics::default();
                        let file_path = Path::new(&file_path_str);
                        let op_size = get_effective_op_size(args_clone.op_size_bytes);
                        let (file_proxy, zx_vmo) =
                            setup_backing_vmo(&file_path, args_clone.file_size_bytes)?;
                        run_sequential(
                            zx_vmo,
                            &file_proxy,
                            op_size as usize,
                            args_clone.file_size_bytes,
                            args_clone.rate_mibs,
                            args_clone.fsync_every_n_ops,
                            args_clone.read,
                            stop_signal,
                            &mut metrics,
                        )?;
                        Ok(metrics)
                    })
                    .context("Failed to spawn thread")?
            }
            WorkloadSubcommand::Burst(args) => {
                let file_path_str = format!("/data/stress_target_{}", display_name);
                created_file_paths.push(file_path_str.clone());

                let args_clone = args.clone();
                thread::Builder::new()
                    .name(display_name.clone())
                    .spawn(move || -> anyhow::Result<Metrics> {
                        let mut metrics = Metrics::default();
                        let file_path = Path::new(&file_path_str);
                        let op_size = get_effective_op_size(args_clone.op_size_bytes);
                        let (file_proxy, zx_vmo) =
                            setup_backing_vmo(&file_path, BURST_IO_PREALLOC_SIZE)?;
                        run_burst(
                            zx_vmo,
                            &file_proxy,
                            op_size as usize,
                            args_clone.burst_ops_count as usize,
                            args_clone.sleep_between_bursts_ms,
                            args_clone.periodic_fsync_ms,
                            args_clone.read,
                            args_clone.rate_mibs,
                            stop_signal,
                            &mut metrics,
                        )?;
                        Ok(metrics)
                    })
                    .context("Failed to spawn thread")?
            }
        };

        thread_handles.push((persona, handle));
    }

    let _file_guard = FileCleanupGuard { paths: created_file_paths };

    let start_time = Instant::now();
    let mut results: Vec<(&'static str, Metrics)> = Vec::new();
    let mut first_error = None;

    for (persona, handle) in thread_handles {
        let join_result = fuchsia_async::unblock(move || handle.join()).await;
        match join_result {
            Ok(Ok(metrics)) => results.push((persona, metrics)),
            Ok(Err(e)) => {
                stop_signal.store(true, Ordering::Relaxed);
                if first_error.is_none() {
                    first_error = Some(e.context(format!("Workload '{}' returned error", persona)));
                }
            }
            Err(_) => {
                stop_signal.store(true, Ordering::Relaxed);
                if first_error.is_none() {
                    first_error = Some(anyhow::anyhow!("Workload '{}' panicked", persona));
                }
            }
        }
    }
    if let Some(err) = first_error {
        return Err(err);
    }

    let elapsed_secs = start_time.elapsed().as_secs_f64();

    let mut persona_map: std::collections::BTreeMap<&'static str, Metrics> =
        std::collections::BTreeMap::new();
    for (persona, m) in results {
        let entry = persona_map.entry(persona).or_default();
        entry.merge(&m);
    }

    let mut total_bytes = 0;
    let mut perf_results = Vec::new();

    for (persona, m) in &persona_map {
        total_bytes += m.write_bytes + m.read_bytes;
        let p95_op = calc_percentile(m.op_latencies_ns.clone(), 0.95);
        let p99_op = calc_percentile(m.op_latencies_ns.clone(), 0.99);
        let throughput = if elapsed_secs > 0.0 {
            (m.write_bytes + m.read_bytes) as f64 / elapsed_secs
        } else {
            0.0
        };

        perf_results.push(FuchsiaPerfBenchmarkResult {
            label: format!("{}/{}/p95_op_latency", run_name, persona),
            test_suite: "fuchsia.io_stress".to_string(),
            unit: Unit::Nanoseconds,
            direction: Direction::SmallerBetter,
            values: vec![p95_op],
        });
        perf_results.push(FuchsiaPerfBenchmarkResult {
            label: format!("{}/{}/p99_op_latency", run_name, persona),
            test_suite: "fuchsia.io_stress".to_string(),
            unit: Unit::Nanoseconds,
            direction: Direction::SmallerBetter,
            values: vec![p99_op],
        });
        perf_results.push(FuchsiaPerfBenchmarkResult {
            label: format!("{}/{}/throughput", run_name, persona),
            test_suite: "fuchsia.io_stress".to_string(),
            unit: Unit::BytesPerSecond,
            direction: Direction::BiggerBetter,
            values: vec![throughput],
        });
        if m.fsync_ops > 0 {
            let p95_fsync = calc_percentile(m.fsync_latencies_ns.clone(), 0.95);
            let p99_fsync = calc_percentile(m.fsync_latencies_ns.clone(), 0.99);
            perf_results.push(FuchsiaPerfBenchmarkResult {
                label: format!("{}/{}/p95_fsync_latency", run_name, persona),
                test_suite: "fuchsia.io_stress".to_string(),
                unit: Unit::Nanoseconds,
                direction: Direction::SmallerBetter,
                values: vec![p95_fsync],
            });
            perf_results.push(FuchsiaPerfBenchmarkResult {
                label: format!("{}/{}/p99_fsync_latency", run_name, persona),
                test_suite: "fuchsia.io_stress".to_string(),
                unit: Unit::Nanoseconds,
                direction: Direction::SmallerBetter,
                values: vec![p99_fsync],
            });
        }
    }

    let total_throughput_bps =
        if elapsed_secs > 0.0 { total_bytes as f64 / elapsed_secs } else { 0.0 };
    let mem_score = match mem_stats.max_level() {
        fidl_fuchsia_memorypressure::Level::Normal => 0.0,
        fidl_fuchsia_memorypressure::Level::Warning => 1.0,
        fidl_fuchsia_memorypressure::Level::Critical => 2.0,
    };

    perf_results.push(FuchsiaPerfBenchmarkResult {
        label: format!("{}/total_throughput", run_name),
        test_suite: "fuchsia.io_stress".to_string(),
        unit: Unit::BytesPerSecond,
        direction: Direction::BiggerBetter,
        values: vec![total_throughput_bps],
    });
    perf_results.push(FuchsiaPerfBenchmarkResult {
        label: format!("{}/max_memory_pressure_score", run_name),
        test_suite: "fuchsia.io_stress".to_string(),
        unit: Unit::Count,
        direction: Direction::SmallerBetter,
        values: vec![mem_score],
    });

    perf_results.sort_by(|a, b| a.label.cmp(&b.label));

    Ok(perf_results)
}
