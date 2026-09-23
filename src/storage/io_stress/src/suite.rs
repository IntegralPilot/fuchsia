// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::FromArgs;
use io_stress::stress::{self, MemoryPressureStats, watch_memory_pressure};
use io_stress::workloads::{
    RandomArgs, SequentialArgs, TransferArgs, WorkloadSubcommand, run_workloads,
};
use std::sync::Arc;
use std::time::Duration;

#[derive(FromArgs, Debug)]
/// Dynamic Storage IO Stress Suite Runner
struct SuiteArgs {
    /// duration of each workload run in seconds (default: 10)
    #[argh(option, default = "10")]
    duration_secs: u64,

    /// evaluation track to run: "fairness", "correctness", "stress", or "all" (default: "all")
    #[argh(option, default = "String::from(\"all\")")]
    track: String,

    /// output path for fuchsiaperf json results
    #[argh(option)]
    output_fuchsiaperf: Option<String>,
}

#[fuchsia::main(logging_tags = ["io_stress_suite"])]
async fn main(args: SuiteArgs) -> Result<()> {
    if let Ok(entries) = std::fs::read_dir("/data") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.starts_with("stress_target_"))
                .unwrap_or(false)
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    let mem_stats = Arc::new(MemoryPressureStats::new());

    let mem_stats_clone = mem_stats.clone();
    let _watcher_task = fuchsia_async::Task::spawn(async move {
        if let Err(e) = watch_memory_pressure(mem_stats_clone).await {
            log::error!("Memory pressure watcher task failed: {:?}", e);
        }
    });

    run_orchestrated_suite(
        args.duration_secs,
        &args.track,
        args.output_fuchsiaperf.as_deref(),
        mem_stats,
    )
    .await?;

    Ok(())
}

async fn run_orchestrated_suite(
    duration_secs: u64,
    track: &str,
    output_fuchsiaperf: Option<&str>,
    mem_stats: Arc<MemoryPressureStats>,
) -> Result<()> {
    let num_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let cpu_load_cores = std::cmp::max(1, num_cpus - 1);

    let run_fairness = track.eq_ignore_ascii_case("all") || track.eq_ignore_ascii_case("fairness");
    let run_correctness =
        track.eq_ignore_ascii_case("all") || track.eq_ignore_ascii_case("correctness");
    let run_stress = track.eq_ignore_ascii_case("all") || track.eq_ignore_ascii_case("stress");

    let mut all_perf_results = Vec::new();

    // Track 1: Fairness
    if run_fairness {
        println!("=========================================================================");
        println!("TRACK 1: Fairness");
        println!("=========================================================================");

        println!("------------------------------------------------------------------------");
        println!("Run 1.1: Fairness (Light) - Throttled (2x Database + 1x Download)");
        println!("------------------------------------------------------------------------");
        let run_1_1_subs = make_fairness_light(15, 1);
        all_perf_results.extend(
            run_workloads("fairness_light", run_1_1_subs, duration_secs, mem_stats.clone()).await?,
        );
        stress::wait_for_cooldown(&mem_stats, Duration::from_secs(60)).await?;

        println!("------------------------------------------------------------------------");
        println!(
            "Run 1.2: Fairness (Heavy) - Throttled (4x Database + Download + Media + Compress + Copy)"
        );
        println!("------------------------------------------------------------------------");
        let run_1_2_subs = make_fairness_heavy(15, 1);
        all_perf_results.extend(
            run_workloads("fairness_heavy", run_1_2_subs, duration_secs, mem_stats.clone()).await?,
        );
        stress::wait_for_cooldown(&mem_stats, Duration::from_secs(60)).await?;
    }

    // Track 2: Correctness
    if run_correctness {
        println!("=========================================================================");
        println!("TRACK 2: Correctness");
        println!("=========================================================================");

        println!("------------------------------------------------------------------------");
        println!("Run 2.1: Correctness (Light) - Unconstrained (AppLaunch + Copy + 2x Database)");
        println!("------------------------------------------------------------------------");
        let run_2_1_subs = make_correctness_light();
        all_perf_results.extend(
            run_workloads("correctness_light", run_2_1_subs, duration_secs, mem_stats.clone())
                .await?,
        );
        stress::wait_for_cooldown(&mem_stats, Duration::from_secs(60)).await?;

        println!("------------------------------------------------------------------------");
        println!(
            "Run 2.2: Correctness (Heavy) - Unconstrained (2x AppLaunch + 2x Copy + 2x Compress + 2x Database)"
        );
        println!("------------------------------------------------------------------------");
        let run_2_2_subs = make_correctness_heavy();
        all_perf_results.extend(
            run_workloads("correctness_heavy", run_2_2_subs, duration_secs, mem_stats.clone())
                .await?,
        );
        stress::wait_for_cooldown(&mem_stats, Duration::from_secs(60)).await?;
    }

    // Track 3: Stress
    if run_stress {
        println!("=========================================================================");
        println!("TRACK 3: Stress");
        println!("=========================================================================");

        println!("------------------------------------------------------------------------");
        println!("Run 3.1: Stress (Light) - CPU Loaded (Fairness Light Workload)");
        println!("------------------------------------------------------------------------");
        {
            let _cpu_stressor = stress::CpuStressor::start(cpu_load_cores);
            let run_3_1_subs = make_fairness_light(15, 1);
            all_perf_results.extend(
                run_workloads("stress_light", run_3_1_subs, duration_secs, mem_stats.clone())
                    .await?,
            );
        }
        stress::wait_for_cooldown(&mem_stats, Duration::from_secs(60)).await?;

        println!("------------------------------------------------------------------------");
        println!(
            "Run 3.2: Stress (Heavy) - CPU + Memory Pressure Loaded (Fairness Heavy Workload)"
        );
        println!("------------------------------------------------------------------------");
        {
            let _cpu_stressor = stress::CpuStressor::start(cpu_load_cores);
            let _mem_stressor =
                stress::MemoryPressureGuard::new(fidl_fuchsia_memorypressure::Level::Warning)?;
            let run_3_2_subs = make_fairness_heavy(15, 1);
            all_perf_results.extend(
                run_workloads("stress_heavy", run_3_2_subs, duration_secs, mem_stats.clone())
                    .await?,
            );
        }
        stress::wait_for_cooldown(&mem_stats, Duration::from_secs(60)).await?;
    }

    if let Some(out_path) = output_fuchsiaperf {
        let json_str = serde_json::to_string_pretty(&all_perf_results)?;
        std::fs::write(out_path, json_str)?;
        println!("Wrote FuchsiaPerf JSON export to: {}", out_path);
    }

    println!("=========================================================================");
    println!("All selected tracks completed successfully.");
    println!("=========================================================================");
    Ok(())
}

fn get_file_sizes() -> (u64, u64) {
    let total_physmem = zx::system_get_physmem();
    if total_physmem <= 4_500_000_000 {
        (268_435_456, 16_777_216)
    } else {
        (1_073_741_824, 67_108_864)
    }
}

fn db_worker(size: u64, rate: u64, read_pct: u32) -> WorkloadSubcommand {
    WorkloadSubcommand::Random(RandomArgs {
        op_size_bytes: 4096,
        file_size_bytes: size,
        read_percentage: read_pct,
        fsync_every_n_ops: 16,
        rate_mibs: rate,
        seed: 0,
    })
}

fn download_worker(size: u64, rate: u64) -> WorkloadSubcommand {
    WorkloadSubcommand::Sequential(SequentialArgs {
        op_size_bytes: 131072,
        file_size_bytes: size,
        rate_mibs: rate,
        fsync_every_n_ops: 0,
        read: false,
    })
}

fn media_worker(size: u64, rate: u64) -> WorkloadSubcommand {
    WorkloadSubcommand::Sequential(SequentialArgs {
        op_size_bytes: 131072,
        file_size_bytes: size,
        rate_mibs: rate,
        fsync_every_n_ops: 512,
        read: false,
    })
}

fn copy_worker(size: u64, rate: u64) -> WorkloadSubcommand {
    WorkloadSubcommand::Transfer(TransferArgs {
        op_size_bytes: 131072,
        file_size_bytes: size,
        xor_transform: false,
        fsync_every_n_ops: 0,
        rate_mibs: rate,
    })
}

fn compress_worker(size: u64, rate: u64) -> WorkloadSubcommand {
    WorkloadSubcommand::Transfer(TransferArgs {
        op_size_bytes: 131072,
        file_size_bytes: size,
        xor_transform: true,
        fsync_every_n_ops: 0,
        rate_mibs: rate,
    })
}

fn app_launch_worker(size: u64) -> WorkloadSubcommand {
    WorkloadSubcommand::Random(RandomArgs {
        op_size_bytes: 4096,
        file_size_bytes: size,
        read_percentage: 50,
        fsync_every_n_ops: 0,
        rate_mibs: 0,
        seed: 0,
    })
}

fn make_fairness_light(seq_rate: u64, rand_rate: u64) -> Vec<WorkloadSubcommand> {
    let (seq_size, rand_size) = get_file_sizes();
    vec![
        download_worker(seq_size, seq_rate),
        db_worker(rand_size, rand_rate, 0),
        db_worker(rand_size, rand_rate, 100),
    ]
}

fn make_fairness_heavy(seq_rate: u64, rand_rate: u64) -> Vec<WorkloadSubcommand> {
    let (seq_size, rand_size) = get_file_sizes();
    vec![
        db_worker(rand_size, rand_rate, 0),
        db_worker(rand_size, rand_rate, 0),
        db_worker(rand_size, rand_rate, 100),
        db_worker(rand_size, rand_rate, 100),
        download_worker(seq_size, seq_rate),
        media_worker(seq_size, seq_rate),
        compress_worker(seq_size, 10),
        copy_worker(seq_size, 10),
    ]
}

fn make_correctness_light() -> Vec<WorkloadSubcommand> {
    let (seq_size, rand_size) = get_file_sizes();
    vec![
        app_launch_worker(rand_size),
        copy_worker(seq_size, 0),
        db_worker(rand_size, 1, 0),
        db_worker(rand_size, 1, 100),
    ]
}

fn make_correctness_heavy() -> Vec<WorkloadSubcommand> {
    let (seq_size, rand_size) = get_file_sizes();
    vec![
        app_launch_worker(rand_size),
        app_launch_worker(rand_size),
        copy_worker(seq_size, 0),
        copy_worker(seq_size, 0),
        compress_worker(seq_size, 0),
        compress_worker(seq_size, 0),
        db_worker(rand_size, 1, 0),
        db_worker(rand_size, 1, 100),
    ]
}
