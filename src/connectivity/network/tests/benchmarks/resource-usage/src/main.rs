// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use fidl_fuchsia_net_debug as fnet_debug;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fuchsiaperf::{Direction, Unit};
use humansize::{BINARY, format_size};
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack, ProdNetstack3, TestSandboxExt as _,
};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

mod interfaces;
mod paced_traffic;
mod sockets;

#[derive(FromArgs)]
/// Benchmark the resource usage of the netstack.
struct Args {
    /// whether to run in perftest mode instead of unit test mode
    #[argh(switch, short = 'p')]
    perftest_mode: bool,

    /// path to which the fuchsiaperf.json results will be written to.
    #[argh(positional, default = "String::from(\"/custom_artifacts/results.fuchsiaperf.json\")")]
    output_path: String,
}

const UNIT_TEST_MODE_RUNS: NonZeroUsize = NonZeroUsize::new(1).unwrap();
const PERF_TEST_MODE_RUNS: NonZeroUsize = NonZeroUsize::new(5).unwrap();

#[fuchsia::main]
async fn main() {
    let Args { perftest_mode, output_path } = argh::from_env();

    const BENCHMARK_NAME: &str = "fuchsia.netstack.resource_usage.netstack3";
    let metrics = [
        run_benchmark::<sockets::UdpSockets, ProdNetstack3>(BENCHMARK_NAME, perftest_mode).await,
        run_benchmark::<sockets::TcpSockets, ProdNetstack3>(BENCHMARK_NAME, perftest_mode).await,
        run_benchmark::<interfaces::Interfaces, ProdNetstack3>(BENCHMARK_NAME, perftest_mode).await,
        run_benchmark::<paced_traffic::PacedTraffic, ProdNetstack3>(BENCHMARK_NAME, perftest_mode)
            .await,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    if perftest_mode {
        let metrics_json =
            serde_json::to_string_pretty(&metrics).expect("serialize metrics as JSON");
        std::fs::write(output_path, metrics_json).expect("write metrics as custom artifact");
    }
}

trait Workload {
    const NAME: &'static str;

    async fn create(netstack: &netemul::TestRealm<'_>) -> Self;

    /// Run a self-contained, repeatable workload against the provided hermetic
    /// netstack.
    async fn run(&self, netstack: &netemul::TestRealm<'_>, perftest_mode: bool);
}

async fn run_benchmark<W: Workload, N: Netstack>(
    suite_name: &str,
    perftest_mode: bool,
) -> Vec<fuchsiaperf::FuchsiaPerfBenchmarkResult> {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let netstack = sandbox
        .create_netstack_realm_with::<N, _, _>(
            W::NAME.replace("/", "-"),
            &[KnownServiceProvider::SecureStash],
        )
        .expect("create netstack");

    // Wait for the netstack to start up and initialize the loopback interface.
    let interfaces = {
        let interfaces_state = netstack
            .connect_to_protocol::<fnet_interfaces::StateMarker>()
            .expect("connect to protocol");
        fnet_interfaces_ext::existing(
            fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
                &interfaces_state,
                Default::default(),
            )
            .expect("get interface event stream"),
            HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new(),
        )
        .await
        .expect("collect existing interfaces")
    };
    assert_eq!(interfaces.len(), 1);
    let fnet_interfaces_ext::PropertiesAndState {
        properties: fnet_interfaces_ext::Properties { port_class, online, .. },
        state: _,
    } = interfaces.values().into_iter().next().unwrap();
    assert_eq!(port_class, &fnet_interfaces_ext::PortClass::Loopback);
    assert!(online);

    let diagnostics = netstack
        .connect_to_protocol::<fnet_debug::DiagnosticsMarker>()
        .expect("connect to protocol");
    let process = diagnostics
        .get_process_handle_for_inspection()
        .await
        .expect("get handle to netstack process");
    let process = Arc::new(process);

    // Record baseline resource (memory + handle) usage.
    let baseline = ResourceUsage::record(&process);

    let workload = W::create(&netstack).await;

    let (done, rx) = std::sync::mpsc::channel();
    let process_clone = process.clone();
    let measure_peak = std::thread::spawn(move || measure_peak_usage(rx, process_clone));

    let start_time = std::time::Instant::now();

    workload.run(&netstack, perftest_mode).await;
    let initial_increase = ResourceUsage::record(&process) - &baseline;
    let runs = if perftest_mode { PERF_TEST_MODE_RUNS.get() } else { UNIT_TEST_MODE_RUNS.get() };
    for _ in 0..runs - 1 {
        workload.run(&netstack, perftest_mode).await;
    }

    let runtime = start_time.elapsed();
    done.send(()).expect("peak usage task should be running");
    let peak = measure_peak.join().expect("should successfully join peak usage thread");

    // Record the increase in resource usage after we are done exercising the
    // netstack.
    //
    // This is more likely to indicate a real resource leak than any amount of
    // initial increase measured after just one run; there is often an initial
    // amount of allocation that must occur to service a large workload but
    // which does not actually leak in the sense that it can be reused if the
    // same workload is re-run.
    let increase = ResourceUsage::record(&process) - &baseline;

    // We have finished measuring, release all the resources used by the
    // workload.
    drop(workload);

    eprintln!("================== workload: {} ==================\n", W::NAME);
    eprintln!("Running workload {runs} times took {runtime:?}\n");
    eprintln!("Baseline resource usage:\n{baseline}");
    eprintln!("Peak resource usage:\n{peak}");
    eprintln!("Increase in resource usage from baseline:\n{increase}");

    [
        baseline.generate_fuchsiaperf(suite_name, &format!("{}/Baseline", W::NAME)),
        initial_increase.generate_fuchsiaperf(suite_name, &format!("{}/InitialIncrease", W::NAME)),
        increase.generate_fuchsiaperf(suite_name, &format!("{}/Increase", W::NAME)),
        peak.generate_fuchsiaperf(suite_name, &format!("{}/Peak", W::NAME)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[derive(Debug, Default)]
struct ResourceUsage {
    handles: HandleUsage,
    memory: MemoryUsage,
}

#[derive(Debug)]
struct HandleUsage {
    total: usize,
    counts: [usize; zx::sys::ZX_OBJ_TYPE_UPPER_BOUND],
}

impl Default for HandleUsage {
    fn default() -> Self {
        Self { total: 0, counts: [0; zx::sys::ZX_OBJ_TYPE_UPPER_BOUND] }
    }
}

impl From<zx::ProcessHandleStats> for HandleUsage {
    fn from(zx::ProcessHandleStats { handle_count }: zx::ProcessHandleStats) -> Self {
        Self {
            total: handle_count.iter().copied().map(|n| n as usize).sum(),
            counts: handle_count.map(|n| n.try_into().unwrap()),
        }
    }
}

#[derive(Debug, Default)]
struct MemoryUsage {
    private_bytes: usize,
    shared_bytes: usize,
}

impl From<zx::TaskStatsInfo> for MemoryUsage {
    fn from(info: zx::TaskStatsInfo) -> Self {
        let zx::TaskStatsInfo {
            mem_mapped_bytes: _,
            mem_private_bytes,
            mem_shared_bytes,
            mem_scaled_shared_bytes: _,
            mem_fractional_scaled_shared_bytes: _,
        } = info;
        Self { private_bytes: mem_private_bytes, shared_bytes: mem_shared_bytes }
    }
}

impl ResourceUsage {
    fn record(process: &Arc<zx::Process>) -> Self {
        Self {
            handles: process.handle_stats().expect("get netstack handle stats").into(),
            memory: process.task_stats().expect("get netstack task stats").into(),
        }
    }

    fn generate_fuchsiaperf(
        self,
        suite: &str,
        prefix: &str,
    ) -> Vec<fuchsiaperf::FuchsiaPerfBenchmarkResult> {
        let ResourceUsage {
            handles: HandleUsage { total, counts },
            memory: MemoryUsage { private_bytes, shared_bytes },
        } = self;

        let mut results = vec![
            fuchsiaperf::FuchsiaPerfBenchmarkResult {
                test_suite: suite.to_string(),
                label: format!("{prefix}/Memory/Private"),
                values: vec![private_bytes as f64],
                unit: Unit::Bytes,
                direction: Direction::SmallerBetter,
            },
            fuchsiaperf::FuchsiaPerfBenchmarkResult {
                test_suite: suite.to_string(),
                label: format!("{prefix}/Memory/Shared"),
                values: vec![shared_bytes as f64],
                unit: Unit::Bytes,
                direction: Direction::SmallerBetter,
            },
            fuchsiaperf::FuchsiaPerfBenchmarkResult {
                test_suite: suite.to_string(),
                label: format!("{prefix}/Handles/Total"),
                values: vec![total as f64],
                unit: Unit::Count,
                direction: Direction::SmallerBetter,
            },
        ];
        for (object_type, n) in counts.into_iter().enumerate() {
            let name = match zx::ObjectType::from_raw(
                object_type.try_into().expect("object type from index"),
            ) {
                zx::ObjectType::VMO => "Vmo",
                zx::ObjectType::CHANNEL => "Channel",
                zx::ObjectType::SOCKET => "Socket",
                zx::ObjectType::EVENTPAIR => "Eventpair",
                zx::ObjectType::FIFO => "Fifo",
                // Do not report counts for these handle types as separate metrics in order to
                // avoid producing an inordinate amount of total metrics. None of these handle
                // types is currently used frequently by the netstack, and if that changes, it
                // will be reflected in the total handle count metric.
                zx::ObjectType::PROCESS
                | zx::ObjectType::THREAD
                | zx::ObjectType::EVENT
                | zx::ObjectType::PORT
                | zx::ObjectType::INTERRUPT
                | zx::ObjectType::PCI_DEVICE
                | zx::ObjectType::DEBUGLOG
                | zx::ObjectType::RESOURCE
                | zx::ObjectType::JOB
                | zx::ObjectType::VMAR
                | zx::ObjectType::GUEST
                | zx::ObjectType::VCPU
                | zx::ObjectType::TIMER
                | zx::ObjectType::IOMMU
                | zx::ObjectType::BTI
                | zx::ObjectType::PROFILE
                | zx::ObjectType::PMT
                | zx::ObjectType::SUSPEND_TOKEN
                | zx::ObjectType::PAGER
                | zx::ObjectType::EXCEPTION
                | zx::ObjectType::CLOCK
                | zx::ObjectType::STREAM
                | zx::ObjectType::MSI
                | zx::ObjectType::NONE => continue,
                _ if n == 0 => continue,
                other => {
                    eprintln!("unexpected zircon object type: {:?} n={}", other, n);
                    continue;
                }
            };
            results.push(fuchsiaperf::FuchsiaPerfBenchmarkResult {
                test_suite: suite.to_string(),
                label: format!("{prefix}/Handles/{name}"),
                values: vec![n as f64],
                unit: Unit::Count,
                direction: Direction::SmallerBetter,
            });
        }
        results
    }
}

impl std::ops::Sub<&ResourceUsage> for ResourceUsage {
    type Output = Self;

    fn sub(self, rhs: &Self) -> Self::Output {
        let mut result = ResourceUsage {
            handles: HandleUsage {
                total: self.handles.total.saturating_sub(rhs.handles.total),
                counts: self.handles.counts,
            },
            memory: MemoryUsage {
                private_bytes: self.memory.private_bytes.saturating_sub(rhs.memory.private_bytes),
                shared_bytes: self.memory.shared_bytes.saturating_sub(rhs.memory.shared_bytes),
            },
        };
        for (object_type, n) in result.handles.counts.iter_mut().enumerate() {
            *n = n.saturating_sub(rhs.handles.counts[object_type]);
        }
        result
    }
}

impl std::fmt::Display for ResourceUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { handles, memory } = self;
        writeln!(f, "Total handles: {}", handles.total)?;
        for (object_type, n) in handles.counts.iter().enumerate() {
            if *n != 0 {
                let object_type = zx::ObjectType::from_raw(
                    object_type.try_into().expect("object type from index"),
                );
                writeln!(f, "\t{object_type:?} handles: {n}")?;
            }
        }
        writeln!(f, "Memory usage:")?;
        writeln!(f, "\tPrivate: {}", format_size(memory.private_bytes, BINARY))?;
        writeln!(f, "\tShared: {}", format_size(memory.shared_bytes, BINARY))?;

        Ok(())
    }
}

fn measure_peak_usage(
    done: std::sync::mpsc::Receiver<()>,
    process: Arc<zx::Process>,
) -> ResourceUsage {
    let mut max = ResourceUsage::default();
    loop {
        let current = ResourceUsage::record(&process);
        max.handles.total = usize::max(max.handles.total, current.handles.total);
        for (object_type, n) in max.handles.counts.iter_mut().enumerate() {
            *n = usize::max(*n, current.handles.counts[object_type]);
        }
        max.memory.private_bytes =
            usize::max(max.memory.private_bytes, current.memory.private_bytes);
        max.memory.shared_bytes = usize::max(max.memory.shared_bytes, current.memory.shared_bytes);
        match done.recv_timeout(std::time::Duration::from_millis(10)) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("should receive message when benchmark is complete")
            }
            Ok(()) => break,
        }
    }
    max
}
