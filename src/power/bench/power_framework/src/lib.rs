// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Integration test that can help to check the connection to the server
//! and the fidl call.

mod daemon_work;
mod sag_work;

use anyhow::{Error, Result, format_err};
use argh::FromArgs;
use diagnostics_hierarchy::DiagnosticsHierarchy;
use diagnostics_reader::ArchiveReader;
use fidl::endpoints::create_sync_proxy;
use fidl_fuchsia_power_broker as fbroker;
use std::time::Instant;

#[derive(FromArgs, Debug)]
/// Command line argument for the tests
struct Options {
    /// the format for changing the argument on command line is `-- --repeat N`, e.g.
    /// fx test -o power-framework-bench-integration-tests --test-filter=*takewakelease -- --repeat 5000
    #[argh(option, default = "1000")]
    repeat: u32,

    /// switch used by rust test runner.
    #[argh(switch)]
    #[allow(unused)]
    nocapture: bool,

    /// timeout for the test in seconds
    #[argh(option, default = "10")]
    timeout_secs: u64,

    /// enables a quick and dirty mechanism for the host to synchronize with the test to perform
    /// memory profiling:
    ///   - The test will log "WAITING FOR MEMORY PROFILING" and sleeps indefinitely.
    ///   - The host takes a memory profile of the appropriate test component.
    ///   - The host kills the test.
    #[argh(switch)]
    wait_for_memory_profiling: bool,
}

async fn maybe_wait_for_memory_profiling(args: &Options) {
    // See flag docstring for usage.
    if args.wait_for_memory_profiling {
        println!("WAITING FOR MEMORY PROFILING");
        std::future::pending::<()>().await;
    }
}

/// Runs the given function `func` for `args.repeat` times or until `args.timeout_secs` is reached.
/// Prints the power broker inspect stats every 1000 iterations.
/// Returns the number of iterations performed.
async fn iterate_until_timeout<F>(args: &Options, mut func: F) -> u32
where
    F: FnMut(u32),
{
    let timeout = std::time::Duration::from_secs(args.timeout_secs);
    let start = Instant::now();
    let mut iterations = 0;
    while start.elapsed() < timeout && iterations < args.repeat {
        func(iterations);
        if iterations > 0 && iterations % 1000 == 0 {
            print_power_broker_inspect_stats(iterations).await;
        }
        iterations += 1;
    }
    iterations
}

#[fuchsia::test]
async fn test_sag_takewakelease() {
    let args: Options = argh::from_env::<Options>();

    let sag_arc = sag_work::obtain_sag_proxy();
    let start = Instant::now();
    let iterations = iterate_until_timeout(&args, |_| {
        sag_work::execute(&sag_arc);
    })
    .await;
    assert!(iterations > 0, "Test failed to complete at least 1 iteration");
    let duration = start.elapsed();
    println!("Total execution time: {:?}", duration);
    println!("Average time for each call is {:?}", duration / iterations);

    // Check how much PB Inspect VMO we used.
    print_power_broker_inspect_stats(iterations).await;

    maybe_wait_for_memory_profiling(&args).await;
    ()
}

#[fuchsia::test]
async fn test_topologytestdaemon_toggle() -> Result<()> {
    let args: Options = argh::from_env::<Options>();

    let (topology_control, status_channel) = daemon_work::prepare_work();
    let start = Instant::now();
    let iterations = iterate_until_timeout(&args, |_| {
        daemon_work::execute(&topology_control, &status_channel);
    })
    .await;
    assert!(iterations > 0, "Test failed to complete at least 1 iteration");
    let duration = start.elapsed();
    println!("Total execution time: {:?}", duration);
    println!("Average time for each call is {:?}", duration / iterations);

    maybe_wait_for_memory_profiling(&args).await;
    Ok(())
}

async fn get_power_broker_inspect() -> Result<DiagnosticsHierarchy, Error> {
    ArchiveReader::inspect()
        .select_all_for_component("test-power-broker")
        .snapshot()
        .await?
        .into_iter()
        .next()
        .and_then(|result| result.payload)
        .ok_or_else(|| format_err!("expected one inspect hierarchy"))
}

fn get_inspect_vmo_bytes(inspect: &DiagnosticsHierarchy) -> (u64, u64) {
    let curr = inspect
        .get_property_by_path(&vec!["fuchsia.inspect.Stats", "current_size"])
        .unwrap()
        .uint()
        .unwrap();
    let max = inspect
        .get_property_by_path(&vec!["fuchsia.inspect.Stats", "maximum_size"])
        .unwrap()
        .uint()
        .unwrap();
    return (curr, max);
}

async fn print_power_broker_inspect_stats(iteration: u32) {
    let pb_inspect = get_power_broker_inspect().await.expect("Inspect data");
    let (used, max) = get_inspect_vmo_bytes(&pb_inspect);
    println!(
        "{} - Power Broker inspect used {} / {} bytes, {:.0} % utilization",
        iteration,
        used,
        max,
        (used as f64 / max as f64) * 100.0
    );
    ()
}

#[fuchsia::test]
async fn test_large_topology_lease_benchmark() -> Result<()> {
    // TODO(b/491223927): I'd like to get this to at least 100, but starting
    // here, and we'll bump it up as we make improvements.
    let num_elements = 20;
    let args: Options = argh::from_env::<Options>();

    println!("Building large topology with {} elements...", num_elements);
    let topology_control = daemon_work::prepare_large_topology(num_elements);
    println!("Topology created.");

    let start = Instant::now();
    let randomize = false;
    let iterations = iterate_until_timeout(&args, |_| {
        daemon_work::execute_acquire_and_drop_lease(&topology_control, num_elements, randomize);
    })
    .await;
    assert!(iterations > 0, "Test failed to complete at least 1 iteration");
    let duration = start.elapsed();
    println!("Total execution time over {} iterations: {:?}", iterations, duration);
    println!(
        "Average time for each execution ({} leases acquire/drop) is {:?}",
        iterations,
        duration / iterations
    );

    print_power_broker_inspect_stats(iterations).await;

    maybe_wait_for_memory_profiling(&args).await;
    Ok(())
}

#[fuchsia::test]
async fn test_large_topology_with_background_leases_benchmark() -> Result<()> {
    let num_elements = 20;
    let num_background_leases = 15;
    let args: Options = argh::from_env::<Options>();

    println!(
        "Building large topology with {} elements and {} background leases...",
        num_elements, num_background_leases
    );
    let topology_control = daemon_work::prepare_large_topology_with_background_leases(
        num_elements,
        num_background_leases,
    );
    println!("Topology and background leases created.");

    let start = Instant::now();
    let randomize = false;
    let iterations = iterate_until_timeout(&args, |_| {
        daemon_work::execute_acquire_and_drop_lease(&topology_control, num_elements, randomize);
    })
    .await;
    assert!(iterations > 0, "Test failed to complete at least 1 iteration");
    let duration = start.elapsed();
    println!("Total execution time over {} iterations: {:?}", iterations, duration);
    println!(
        "Average time for each execution (1 lease acquire/drop with {} background leases) is {:?}",
        num_background_leases,
        duration / iterations
    );

    print_power_broker_inspect_stats(iterations).await;

    maybe_wait_for_memory_profiling(&args).await;
    Ok(())
}

#[fuchsia::test]
async fn test_large_shared_topology() -> Result<()> {
    let args: Options = argh::from_env::<Options>();

    const SHARED_ELEMENTS: usize = 80;
    const DEVICE_ELEMENTS: usize = 20;

    println!(
        "Building large shared topology with {} elements ({} shared, {} device elements)...",
        daemon_work::LARGE_SHARED_TOPOLOGY_TOTAL_ELEMENTS,
        SHARED_ELEMENTS,
        DEVICE_ELEMENTS
    );
    let topology_control = daemon_work::prepare_large_shared_topology();
    println!("Large shared topology and shared background leases created.");

    // Map from number of affected elements to (total_duration, iteration_count)
    let mut stats_by_affected: std::collections::BTreeMap<usize, (std::time::Duration, u32)> =
        std::collections::BTreeMap::new();

    let start = Instant::now();
    let randomize = false;
    let iterations = iterate_until_timeout(&args, |_| {
        let iter_start = Instant::now();
        let affected =
            daemon_work::execute_large_shared_topology_lease(&topology_control, randomize);
        let iter_duration = iter_start.elapsed();

        let entry = stats_by_affected.entry(affected).or_insert((std::time::Duration::ZERO, 0));
        entry.0 += iter_duration;
        entry.1 += 1;
    })
    .await;
    assert!(iterations > 0, "Test failed to complete at least 1 iteration");
    let duration = start.elapsed();
    println!("Total execution time over {} iterations: {:?}", iterations, duration);
    println!(
        "Overall average time for each execution (1 lease acquire/drop across 100 elements) is {:?}",
        duration / iterations
    );

    println!("Average time breakdown by number of affected elements:");
    for (affected, (total_time, count)) in &stats_by_affected {
        if *count > 0 {
            println!(
                "  {} affected element(s): {:>8.2?} avg across {:>4} iterations (total: {:?})",
                affected,
                *total_time / *count,
                count,
                total_time
            );
        }
    }

    print_power_broker_inspect_stats(iterations).await;

    maybe_wait_for_memory_profiling(&args).await;
    Ok(())
}

#[fuchsia::test]
async fn test_large_shared_topology_correctness() -> Result<()> {
    println!("Building large shared topology and checking correctness...");
    let topology_control = daemon_work::prepare_large_shared_topology();

    // 1. Verify shared elements are ON (level 1) due to the initial background lease on shared_anchor.
    let (status_shared_sys, server_sys) = create_sync_proxy::<fbroker::StatusMarker>();
    topology_control
        .open_status_channel("shared_sys_18", server_sys, zx::MonotonicInstant::INFINITE)
        .expect("open status channel for shared_sys_18")
        .expect("open status channel ok");
    let level = status_shared_sys
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("FIDL watch_power_level")
        .expect("watch_power_level ok");
    assert_eq!(level, 1, "shared_sys_18 should be ON (level 1)");

    let (status_shared_rail, server_rail) = create_sync_proxy::<fbroker::StatusMarker>();
    topology_control
        .open_status_channel("shared_rail_0", server_rail, zx::MonotonicInstant::INFINITE)
        .expect("open status channel for shared_rail_0")
        .expect("open status channel ok");
    let level = status_shared_rail
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("FIDL watch_power_level")
        .expect("watch_power_level ok");
    assert_eq!(level, 1, "shared_rail_0 should be ON (level 1)");

    // 2. Verify device elements are initially OFF (level 0).
    let (status_dev2_modified, server_dev2_modified) = create_sync_proxy::<fbroker::StatusMarker>();
    topology_control
        .open_status_channel(
            "device_2_modified",
            server_dev2_modified,
            zx::MonotonicInstant::INFINITE,
        )
        .expect("open status channel for device_2_modified")
        .expect("open status channel ok");
    let level = status_dev2_modified
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("FIDL watch_power_level")
        .expect("watch_power_level ok");
    assert_eq!(level, 0, "device_2_modified should initially be OFF (level 0)");

    let (status_dev2_dep0, server_dev2_dep0) = create_sync_proxy::<fbroker::StatusMarker>();
    topology_control
        .open_status_channel("device_2_dep_0", server_dev2_dep0, zx::MonotonicInstant::INFINITE)
        .expect("open status channel for device_2_dep_0")
        .expect("open status channel ok");
    let level = status_dev2_dep0
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("FIDL watch_power_level")
        .expect("watch_power_level ok");
    assert_eq!(level, 0, "device_2_dep_0 should initially be OFF (level 0)");

    let (status_dev1_modified, server_dev1_modified) = create_sync_proxy::<fbroker::StatusMarker>();
    topology_control
        .open_status_channel(
            "device_1_modified",
            server_dev1_modified,
            zx::MonotonicInstant::INFINITE,
        )
        .expect("open status channel for device_1_modified")
        .expect("open status channel ok");
    let level = status_dev1_modified
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("FIDL watch_power_level")
        .expect("watch_power_level ok");
    assert_eq!(level, 0, "device_1_modified should initially be OFF (level 0)");

    // 3. Acquire and drop lease on shared_sys_18. Zero elements should change power levels!
    topology_control
        .acquire_lease(
            "shared_sys_18",
            1,
            fbroker::LeaseStatus::Satisfied,
            zx::MonotonicInstant::INFINITE,
        )
        .expect("FIDL acquire_lease")
        .expect("acquire lease on shared_sys_18");
    // Shared sys remains at 1, device remains at 0.
    topology_control
        .drop_lease("shared_sys_18", zx::MonotonicInstant::INFINITE)
        .expect("FIDL drop_lease")
        .expect("drop lease on shared_sys_18");

    // 4. Acquire lease on device_2_modified. This should raise device_2_dep_0 and device_2_modified to level 1,
    // while device_1_modified remains at level 0 and shared elements remain at level 1.
    topology_control
        .acquire_lease(
            "device_2_modified",
            1,
            fbroker::LeaseStatus::Satisfied,
            zx::MonotonicInstant::INFINITE,
        )
        .expect("FIDL acquire_lease")
        .expect("acquire lease on device_2_modified");

    let level = status_dev2_modified
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("FIDL watch_power_level")
        .expect("watch_power_level ok");
    assert_eq!(level, 1, "device_2_modified should now be ON (level 1)");

    let level = status_dev2_dep0
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("FIDL watch_power_level")
        .expect("watch_power_level ok");
    assert_eq!(level, 1, "device_2_dep_0 should now be ON (level 1)");

    // 5. Drop lease on device_2_modified. Device 2 elements should return to level 0.
    topology_control
        .drop_lease("device_2_modified", zx::MonotonicInstant::INFINITE)
        .expect("FIDL drop_lease")
        .expect("drop lease on device_2_modified");

    let level = status_dev2_modified
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("FIDL watch_power_level")
        .expect("watch_power_level ok");
    assert_eq!(level, 0, "device_2_modified should be OFF (level 0) after drop");

    let level = status_dev2_dep0
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("FIDL watch_power_level")
        .expect("watch_power_level ok");
    assert_eq!(level, 0, "device_2_dep_0 should be OFF (level 0) after drop");

    println!("Correctness verification passed!");
    Ok(())
}
