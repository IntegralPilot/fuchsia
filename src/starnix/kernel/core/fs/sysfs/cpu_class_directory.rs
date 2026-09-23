// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use crate::vfs::FsNodeOps;
use crate::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use crate::vfs::pseudo::simple_file::{BytesFile, BytesFileOps, SimpleFileNode};
use crate::vfs::pseudo::stub_empty_file::StubEmptyFile;
use fidl_fuchsia_hardware_cpu_ctrl as fcpuctrl;
use fidl_fuchsia_power_cpu as fcpu;
use fuchsia_component::client::connect_to_protocol_sync;
use itertools::Itertools;
use starnix_logging::{bug_ref, log_warn};
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::{errno, error, from_status_like_fdio};
use zx;

pub fn build_cpu_class_directory(dir: &SimpleDirectoryMutator) {
    let cpu_domains = get_cpu_domains();

    let mut core_to_domain_map: Vec<(u64, &fcpu::DomainInfo)> = cpu_domains
        .iter()
        .flat_map(|domain| {
            domain
                .core_ids
                .as_ref()
                .expect("core_ids not available")
                .iter()
                .map(move |id| (*id, domain))
        })
        .collect();
    core_to_domain_map.sort_by_key(|(id, _)| *id);
    core_to_domain_map.dedup_by_key(|(id, _)| *id);

    for (core_id, domain) in &core_to_domain_map {
        let name = format!("cpu{}", core_id);
        dir.subdir(&name, 0o755, |dir| build_cpu_directory(dir, domain));
    }

    let core_count = core_to_domain_map.len();

    dir.entry(
        "online",
        BytesFile::new_node(format!("0-{}\n", core_count.saturating_sub(1)).into_bytes()),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "possible",
        BytesFile::new_node(format!("0-{}\n", core_count.saturating_sub(1)).into_bytes()),
        mode!(IFREG, 0o444),
    );
    dir.subdir("vulnerabilities", 0o755, |dir| {
        for (name, contents) in VULNERABILITIES {
            let contents = contents.to_string();
            dir.entry(name, BytesFile::new_node(contents.into_bytes()), mode!(IFREG, 0o444));
        }
    });
    dir.subdir("cpufreq", 0o755, |dir| {
        for domain in &cpu_domains {
            let min_core_id = domain
                .core_ids
                .as_ref()
                .expect("core_ids not available")
                .iter()
                .min()
                .expect("core_ids is empty");
            let name = format!("policy{}", min_core_id);
            dir.subdir(&name, 0o755, |dir| build_cpufreq_directory(dir, domain));
        }
    });
    dir.subdir("soc", 0o755, |dir| {
        dir.subdir("0", 0o755, |dir| {
            dir.entry(
                "machine",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                mode!(IFREG, 0o444),
            );
        });
    });
}

/// Retrieves CPU topology and domain information with a tiered fallback approach.
///
/// 1. Get CPU domains from the `fuchsia.power.cpu.DomainController` FIDL protocol.
/// 2. If that fails, use the `fuchsia.hardware.cpu.ctrl.Service` FIDL service to connect to
///    individual CPU control devices.
/// 3. If that fails, get CPU information from the kernel directly. In this case, no CPU control
///    or topological information is available, so only a limited set of sysfs entries
///    will be populated.
fn get_cpu_domains() -> Vec<fcpu::DomainInfo> {
    // Tier 1: Try DomainController
    if let Ok(domain_controller) = connect_to_protocol_sync::<fcpu::DomainControllerMarker>() {
        if let Ok(mut domains) = domain_controller.list_domains(zx::MonotonicInstant::INFINITE) {
            // Remove any domains without an ID or empty core_ids.
            domains
                .retain(|d| d.id.is_some() && d.core_ids.as_ref().map_or(false, |c| !c.is_empty()));
            if !domains.is_empty() {
                return domains;
            }
        }
    }

    log_warn!(
        "Could not retrieve CPU domains from fuchsia.power.cpu.DomainController, using CPU control devices instead."
    );

    // Tier 2: Try cpu.ctrl devices
    if let Ok(proxies) = connect_to_cpu_devices() {
        let mut domains = Vec::new();

        for proxy in proxies {
            let cpu_count = match proxy.get_num_logical_cores(zx::MonotonicInstant::INFINITE) {
                Ok(count) => count,
                Err(e) => {
                    log_warn!("get_num_logical_cores returned error: {}", e);
                    continue;
                }
            };
            let domain_id = match proxy.get_domain_id(zx::MonotonicInstant::INFINITE) {
                Ok(id) => id as u64,
                Err(e) => {
                    log_warn!("get_domain_id returned error: {}", e);
                    continue;
                }
            };

            let mut core_ids = Vec::with_capacity(cpu_count as usize);
            let mut get_core_failed = false;
            for i in 0..cpu_count {
                let core_id = match proxy.get_logical_core_id(i, zx::MonotonicInstant::INFINITE) {
                    Ok(id) => id,
                    Err(e) => {
                        log_warn!("get_logical_core_id error in domain {}: {}", domain_id, e);
                        get_core_failed = true;
                        break;
                    }
                };
                core_ids.push(core_id);
            }
            if get_core_failed || core_ids.is_empty() {
                log_warn!("get_logical_core_id failed in domain {}, skipping", domain_id);
                continue;
            }

            let available_frequencies_hz =
                match proxy.get_operating_point_count(zx::MonotonicInstant::INFINITE) {
                    Ok(Ok(count)) => {
                        let mut freqs = Vec::with_capacity(count as usize);
                        for i in 0..count {
                            if let Ok(Ok(info)) =
                                proxy.get_operating_point_info(i, zx::MonotonicInstant::INFINITE)
                            {
                                if info.frequency_hz > 0 {
                                    freqs.push(info.frequency_hz as u64);
                                }
                            }
                        }
                        freqs.sort();
                        freqs.dedup();
                        Some(freqs)
                    }
                    _ => None,
                };

            domains.push(fcpu::DomainInfo {
                id: Some(domain_id),
                core_ids: Some(core_ids),
                available_frequencies_hz,
                ..Default::default()
            });
        }

        if !domains.is_empty() {
            return domains;
        }
    }

    log_warn!(
        "Could not connect to CPU control devices, using default domain info from kernel CPU count."
    );

    // Tier 3: Fallback to kernel CPU count
    let cpu_count = zx::system_get_num_cpus();
    vec![fcpu::DomainInfo {
        id: Some(0),
        core_ids: Some((0..cpu_count as u64).collect()),
        available_frequencies_hz: None,
        name: None,
        ..Default::default()
    }]
}

fn hz_to_khz(hz: u64) -> u64 {
    hz / 1000
}

fn get_available_frequencies(domain: &fcpu::DomainInfo) -> Vec<u64> {
    domain
        .available_frequencies_hz
        .as_deref()
        .map(|freqs| freqs.iter().map(|f| hz_to_khz(*f)).sorted().dedup().collect())
        .unwrap_or_default()
}

fn build_cpu_directory(dir: &SimpleDirectoryMutator, domain: &fcpu::DomainInfo) {
    let cluster_id = domain.id.as_ref().expect("id not available");

    dir.entry(
        "cpu_capacity",
        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
        mode!(IFREG, 0o444),
    );
    dir.subdir("cpufreq", 0o755, |dir| {
        build_cpufreq_directory(dir, domain);
    });
    dir.subdir("topology", 0o755, |dir| {
        dir.entry(
            "cluster_id",
            BytesFile::new_node(format!("{cluster_id}\n").into_bytes()),
            mode!(IFREG, 0o444),
        );
        dir.entry(
            "physical_package_id",
            BytesFile::new_node(format!("{cluster_id}\n").into_bytes()),
            mode!(IFREG, 0o444),
        );
    });
}

fn build_cpufreq_directory(dir: &SimpleDirectoryMutator, domain: &fcpu::DomainInfo) {
    let scaling_available_frequencies = get_available_frequencies(domain);
    let core_ids = domain.core_ids.as_ref().expect("core_ids not available");

    dir.subdir("stats", 0o755, |dir| {
        dir.entry("reset", CpuFreqStatsResetFile::new_node(), mode!(IFREG, 0o200));
        dir.entry(
            "time_in_state",
            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
            mode!(IFREG, 0o444),
        );
    });

    let related_cpus_str = format!("{}\n", core_ids.iter().sorted().join(" "));
    dir.entry(
        "related_cpus",
        BytesFile::new_node(related_cpus_str.into_bytes()),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "scaling_cur_freq",
        create_scaling_cur_freq_file(*domain.id.as_ref().expect("domain id missing")),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "scaling_min_freq",
        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "scaling_max_freq",
        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "scaling_available_frequencies",
        BytesFile::new_node((scaling_available_frequencies.iter().join(" ") + "\n").into_bytes()),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "scaling_available_governors",
        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "scaling_governor",
        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "cpuinfo_max_freq",
        BytesFile::new_node(
            format!(
                "{}\n",
                scaling_available_frequencies.last().map(|f| f.to_string()).unwrap_or_default()
            )
            .into_bytes(),
        ),
        mode!(IFREG, 0o444),
    );
}

const VULNERABILITIES: &[(&str, &str)] = &[
    ("gather_data_sampling", "Not affected\n"),
    ("itlb_multihit", "Not affected\n"),
    ("l1tf", "Not affected\n"),
    ("mds", "Not affected\n"),
    ("meltdown", "Not affected\n"),
    ("mmio_stale_data", "Not affected\n"),
    ("retbleed", "Not affected\n"),
    ("spec_rstack_overflow", "Not affected\n"),
    ("spec_store_bypass", "Not affected\n"),
    ("spectre_v1", "Not affected\n"),
    ("spectre_v2", "Not affected\n"),
    ("srbds", "Not affected\n"),
    ("tsx_async_abort", "Not affected\n"),
];

struct CpuFreqStatsResetFile {}

impl CpuFreqStatsResetFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self {})
    }
}

impl BytesFileOps for CpuFreqStatsResetFile {
    // Currently a no-op. The value written to this node does not matter.
    fn write(&self, _current_task: &CurrentTask, _data: Vec<u8>) -> Result<(), Errno> {
        Ok(())
    }
}

const CPU_DIRECTORY: &str = "/svc/fuchsia.hardware.cpu.ctrl.Service";

fn connect_to_cpu_devices() -> Result<Vec<fcpuctrl::DeviceSynchronousProxy>, Errno> {
    let dir = std::fs::read_dir(CPU_DIRECTORY).map_err(|_| errno!(EINVAL))?;

    let proxies: Vec<_> = dir
        .filter_map(|r| r.ok())
        .filter_map(|entry| {
            let path = entry.path().join("device").into_os_string().into_string().ok()?;
            let (client, server) = zx::Channel::create();
            fdio::service_connect(&path, server).ok()?;
            Some(fcpuctrl::DeviceSynchronousProxy::new(client))
        })
        .collect();

    if proxies.is_empty() { error!(ENOENT) } else { Ok(proxies) }
}

fn connect_to_cpu_device_by_domain_id(
    domain_id: u64,
) -> Result<fcpuctrl::DeviceSynchronousProxy, Errno> {
    let dir = std::fs::read_dir(CPU_DIRECTORY).map_err(|_| errno!(EINVAL))?;

    dir.filter_map(|r| r.ok())
        .find_map(|entry| {
            let path = entry.path().join("device").into_os_string().into_string().ok()?;
            let (client, server) = zx::Channel::create();
            fdio::service_connect(&path, server).ok()?;
            let proxy = fcpuctrl::DeviceSynchronousProxy::new(client);

            let dev_domain_id = proxy.get_domain_id(zx::MonotonicInstant::INFINITE).ok()?;
            if domain_id == dev_domain_id as u64 { Some(proxy) } else { None }
        })
        .ok_or_else(|| errno!(ENOENT))
}

fn create_scaling_cur_freq_file(domain_id: u64) -> impl FsNodeOps {
    let proxy_cache = starnix_sync::Mutex::new(None::<fcpuctrl::DeviceSynchronousProxy>);
    SimpleFileNode::new(move |_| {
        let mut guard = proxy_cache.lock();
        if guard.is_none() {
            let proxy = connect_to_cpu_device_by_domain_id(domain_id)?;
            *guard = Some(proxy);
        }
        let proxy = guard.as_ref().expect("must have a valid proxy");
        let opp = match proxy.get_current_operating_point(zx::MonotonicInstant::INFINITE) {
            Ok(opp) => opp,
            Err(_) => {
                *guard = None;
                return error!(EINVAL);
            }
        };
        let info = match proxy.get_operating_point_info(opp, zx::MonotonicInstant::INFINITE) {
            Ok(info) => info,
            Err(_) => {
                *guard = None;
                return error!(EINVAL);
            }
        };
        let info = info.map_err(|e| from_status_like_fdio!(zx::Status::err_from_raw(e)))?;
        if info.frequency_hz <= 0 {
            return error!(EINVAL);
        }
        let freq_khz = hz_to_khz(info.frequency_hz as u64);
        Ok(BytesFile::new(format!("{}\n", freq_khz).into_bytes()))
    })
}
