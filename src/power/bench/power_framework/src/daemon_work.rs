// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! common functions to be used by Criterion or integration test for the
/// Topology Test Daemon.
use anyhow::Result;
use fidl::endpoints::create_sync_proxy;
use fidl_fuchsia_power_broker as fbroker;
use fidl_fuchsia_power_topology_test as fpt;
use fuchsia_component::client::connect_to_protocol_sync;

use rand::RngExt as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[inline(always)]
fn black_box<T>(placeholder: T) -> T {
    std::hint::black_box(placeholder)
}

fn work_func(
    topology_control: &fpt::TopologyControlSynchronousProxy,
    status_channel: &Arc<fbroker::StatusSynchronousProxy>,
) -> Result<()> {
    // Acquire lease for C @ 5.

    let _ = topology_control
        .acquire_lease("C", 5, fbroker::LeaseStatus::Unknown, zx::MonotonicInstant::INFINITE)
        .unwrap();
    let level = status_channel
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("Fidl call should work")
        .expect("Result should be good");
    assert_eq!(level, 5);

    let _ = topology_control.drop_lease("C", zx::MonotonicInstant::INFINITE).unwrap();
    let level = status_channel
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("Fidl call should work")
        .expect("Result should be good");
    assert_eq!(level, 0);

    Ok(())
}

pub(crate) fn prepare_work()
-> (Arc<fpt::TopologyControlSynchronousProxy>, Arc<fbroker::StatusSynchronousProxy>) {
    // Current Criterion library doesn't support async call yet.
    let topology_control = connect_to_protocol_sync::<fpt::TopologyControlMarker>().unwrap();

    let elements: [fpt::Element; 2] = [
        fpt::Element {
            element_name: "C".to_string(),
            initial_current_level: 0,
            valid_levels: vec![0, 5],
            dependencies: vec![fpt::LevelDependency {
                dependent_level: 5,
                requires_element: "P".to_string(),
                requires_level: 50,
            }],
        },
        fpt::Element {
            element_name: "P".to_string(),
            initial_current_level: 0,
            valid_levels: vec![0, 30, 50],
            dependencies: vec![],
        },
    ];
    let _ = topology_control.create(&elements, zx::MonotonicInstant::INFINITE).unwrap();
    let (status_channel, server_channel) = create_sync_proxy::<fbroker::StatusMarker>();
    let _ =
        topology_control.open_status_channel("C", server_channel, zx::MonotonicInstant::INFINITE);

    let level = status_channel
        .watch_power_level(zx::MonotonicInstant::INFINITE)
        .expect("Fidl call should work")
        .expect("Result should be good");
    assert_eq!(level, 0);

    (Arc::new(topology_control), Arc::new(status_channel))
}

pub(crate) fn execute(
    topology_control: &fpt::TopologyControlSynchronousProxy,
    status_channel: &Arc<fbroker::StatusSynchronousProxy>,
) {
    let _ = black_box(work_func(topology_control, status_channel));
}

fn acquire_and_drop_rand_lease_work_func(
    topology_control: &fpt::TopologyControlSynchronousProxy,
    num_elements: usize,
    lease_random_element: bool,
) -> Result<()> {
    let mut rng = rand::rng();
    let i = if lease_random_element { rng.random_range(0..num_elements) } else { num_elements - 1 };

    let _ = topology_control
        .acquire_lease(
            &format!("element_{}", i),
            1,
            fbroker::LeaseStatus::Satisfied,
            zx::MonotonicInstant::INFINITE,
        )
        .unwrap();

    let _ = topology_control
        .drop_lease(&format!("element_{}", i), zx::MonotonicInstant::INFINITE)
        .unwrap();
    Ok(())
}

pub(crate) fn prepare_large_topology(
    num_elements: usize,
) -> Arc<fpt::TopologyControlSynchronousProxy> {
    let topology_control = connect_to_protocol_sync::<fpt::TopologyControlMarker>().unwrap();

    let mut elements = Vec::new();

    for i in 0..num_elements {
        let name = format!("element_{}", i);
        let mut deps = Vec::new();
        if i > 0 {
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: format!("element_{}", i - 1),
                requires_level: 1,
            });
            // Multi-dependencies to trigger exponential explosions natively
            if i > 1 {
                deps.push(fpt::LevelDependency {
                    dependent_level: 1,
                    requires_element: format!("element_{}", i - 2),
                    requires_level: 1,
                });
            }
        }
        elements.push(fpt::Element {
            element_name: name,
            initial_current_level: 0,
            valid_levels: vec![0, 1],
            dependencies: deps,
        });
    }

    let _ = topology_control.create(&elements, zx::MonotonicInstant::INFINITE).unwrap();

    Arc::new(topology_control)
}

pub(crate) fn prepare_large_topology_with_background_leases(
    num_elements: usize,
    num_background_leases: usize,
) -> Arc<fpt::TopologyControlSynchronousProxy> {
    let topology_control = prepare_large_topology(num_elements);

    // Acquire persistent background leases across lower/intermediate elements.
    for i in 0..num_background_leases.min(num_elements.saturating_sub(1)) {
        let name = format!("element_{}", i);
        let _ = topology_control
            .acquire_lease(
                &name,
                1,
                fbroker::LeaseStatus::Satisfied,
                zx::MonotonicInstant::INFINITE,
            )
            .expect("Fidl call should work")
            .expect("acquire background lease should succeed");
    }

    topology_control
}

pub(crate) fn execute_acquire_and_drop_lease(
    topology_control: &fpt::TopologyControlSynchronousProxy,
    num_elements: usize,
    randomize: bool,
) {
    let _ =
        black_box(acquire_and_drop_rand_lease_work_func(topology_control, num_elements, randomize));
}

pub const LARGE_SHARED_TOPOLOGY_TOTAL_ELEMENTS: usize = 100;

// The shared anchor element holds the background lease that keeps all 80 shared elements active.
pub const LARGE_SHARED_TOPOLOGY_SHARED_HEADS: [&str; 1] = ["shared_anchor"];

pub const LEASE_TARGETS: [&str; 7] = [
    "shared_sys_18",     // 0 elements modified (already active via shared_anchor)
    "shared_soc_24",     // 0 elements modified (already active via shared_anchor)
    "device_1_modified", // 1 element modified
    "device_2_modified", // 2 elements modified
    "device_3_modified", // 3 elements modified
    "device_4_modified", // 4 elements modified
    "device_5_modified", // 5 elements modified
];

pub fn target_affected_elements(target: &str) -> usize {
    match target {
        "shared_sys_18" | "shared_soc_24" => 0,
        "device_1_modified" => 1,
        "device_2_modified" => 2,
        "device_3_modified" => 3,
        "device_4_modified" => 4,
        "device_5_modified" => 5,
        _ => panic!("Unknown lease target: {}", target),
    }
}

static TARGET_INDEX: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn build_large_shared_topology_elements() -> Vec<fpt::Element> {
    let mut elements = Vec::with_capacity(LARGE_SHARED_TOPOLOGY_TOTAL_ELEMENTS);

    // 1. Shared Power Distribution Rails (10 elements: shared_rail_0 .. shared_rail_9)
    for i in 0..10 {
        let name = format!("shared_rail_{}", i);
        let mut deps = Vec::new();
        if i > 0 {
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: format!("shared_rail_{}", i - 1),
                requires_level: 1,
            });
        }
        elements.push(fpt::Element {
            element_name: name,
            initial_current_level: 0,
            valid_levels: vec![0, 1],
            dependencies: deps,
        });
    }

    // 2. Shared SoC domain (25 elements: shared_soc_0 .. shared_soc_24)
    for i in 0..25 {
        let name = format!("shared_soc_{}", i);
        let mut deps = Vec::new();
        if i == 0 {
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: "shared_rail_9".to_string(),
                requires_level: 1,
            });
        } else {
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: format!("shared_soc_{}", i - 1),
                requires_level: 1,
            });
        }
        elements.push(fpt::Element {
            element_name: name,
            initial_current_level: 0,
            valid_levels: vec![0, 1],
            dependencies: deps,
        });
    }

    // 3. Shared Interconnect / Bus domain (25 elements: shared_bus_0 .. shared_bus_24)
    for i in 0..25 {
        let name = format!("shared_bus_{}", i);
        let mut deps = Vec::new();
        if i == 0 {
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: "shared_rail_9".to_string(),
                requires_level: 1,
            });
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: "shared_soc_0".to_string(),
                requires_level: 1,
            });
        } else {
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: format!("shared_bus_{}", i - 1),
                requires_level: 1,
            });
        }
        elements.push(fpt::Element {
            element_name: name,
            initial_current_level: 0,
            valid_levels: vec![0, 1],
            dependencies: deps,
        });
    }

    // 4. Shared System services domain (19 elements: shared_sys_0 .. shared_sys_18)
    for i in 0..19 {
        let name = format!("shared_sys_{}", i);
        let mut deps = Vec::new();
        if i == 0 {
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: "shared_bus_24".to_string(),
                requires_level: 1,
            });
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: "shared_soc_24".to_string(),
                requires_level: 1,
            });
        } else {
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: format!("shared_sys_{}", i - 1),
                requires_level: 1,
            });
        }
        elements.push(fpt::Element {
            element_name: name,
            initial_current_level: 0,
            valid_levels: vec![0, 1],
            dependencies: deps,
        });
    }

    // 5. Shared anchor (1 element: shared_anchor)
    // Depends on shared_sys_18 and shared_soc_24, transitively holding all 80 shared elements.
    elements.push(fpt::Element {
        element_name: "shared_anchor".to_string(),
        initial_current_level: 0,
        valid_levels: vec![0, 1],
        dependencies: vec![
            fpt::LevelDependency {
                dependent_level: 1,
                requires_element: "shared_sys_18".to_string(),
                requires_level: 1,
            },
            fpt::LevelDependency {
                dependent_level: 1,
                requires_element: "shared_soc_24".to_string(),
                requires_level: 1,
            },
        ],
    });

    // 6. Peripheral devices (20 elements total)
    // 5 devices with variable dependency chain lengths [1, 2, 3, 4, 5], yielding 15 elements:
    // - device_1_modified (1 element modified)
    // - device_2_dep_0 -> device_2_modified (2 elements modified)
    // - device_3_dep_0 -> device_3_dep_1 -> device_3_modified (3 elements modified)
    // - device_4_dep_0 -> device_4_dep_1 -> device_4_dep_2 -> device_4_modified (4 elements modified)
    // - device_5_dep_0 -> ... -> device_5_dep_3 -> device_5_modified (5 elements modified)
    for n in 1..=5 {
        for k in 0..n {
            let name = if k == n - 1 {
                format!("device_{}_modified", n)
            } else {
                format!("device_{}_dep_{}", n, k)
            };
            let mut deps = Vec::new();
            if k == 0 {
                deps.push(fpt::LevelDependency {
                    dependent_level: 1,
                    requires_element: "shared_sys_18".to_string(),
                    requires_level: 1,
                });
                deps.push(fpt::LevelDependency {
                    dependent_level: 1,
                    requires_element: "shared_soc_24".to_string(),
                    requires_level: 1,
                });
            } else {
                deps.push(fpt::LevelDependency {
                    dependent_level: 1,
                    requires_element: format!("device_{}_dep_{}", n, k - 1),
                    requires_level: 1,
                });
            }
            elements.push(fpt::Element {
                element_name: name,
                initial_current_level: 0,
                valid_levels: vec![0, 1],
                dependencies: deps,
            });
        }
    }

    // 7. Idle peripheral devices (5 elements: device_idle_elem_0..4)
    // Unleased during tests, representing background hardware devices, bringing total device elements to 20.
    for k in 0..5 {
        let name = format!("device_idle_elem_{}", k);
        let mut deps = Vec::new();
        if k == 0 {
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: "shared_sys_18".to_string(),
                requires_level: 1,
            });
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: "shared_soc_24".to_string(),
                requires_level: 1,
            });
        } else {
            deps.push(fpt::LevelDependency {
                dependent_level: 1,
                requires_element: format!("device_idle_elem_{}", k - 1),
                requires_level: 1,
            });
        }
        elements.push(fpt::Element {
            element_name: name,
            initial_current_level: 0,
            valid_levels: vec![0, 1],
            dependencies: deps,
        });
    }

    assert_eq!(elements.len(), LARGE_SHARED_TOPOLOGY_TOTAL_ELEMENTS);
    // Reverse elements so that root elements are at the back of the Vec.
    // TopologyTestDaemon pops from the back, so parents/roots are created before children.
    elements.reverse();
    elements
}

pub(crate) fn prepare_large_shared_topology() -> Arc<fpt::TopologyControlSynchronousProxy> {
    let topology_control = connect_to_protocol_sync::<fpt::TopologyControlMarker>().unwrap();
    let elements = build_large_shared_topology_elements();

    let _ = topology_control
        .create(&elements, zx::MonotonicInstant::INFINITE)
        .expect("FIDL create call should succeed")
        .expect("Topology creation should succeed");

    // Acquire persistent background lease on shared_anchor to hold all 80 shared elements ON.
    for &head in &LARGE_SHARED_TOPOLOGY_SHARED_HEADS {
        let _ = topology_control
            .acquire_lease(head, 1, fbroker::LeaseStatus::Satisfied, zx::MonotonicInstant::INFINITE)
            .expect("FIDL acquire_lease call should succeed")
            .expect("acquire shared background lease should succeed");
    }

    Arc::new(topology_control)
}

fn large_shared_topology_lease_work_func(
    topology_control: &fpt::TopologyControlSynchronousProxy,
    randomize: bool,
) -> Result<usize> {
    let target = if randomize {
        let mut rng = rand::rng();
        let idx = rng.random_range(0..LEASE_TARGETS.len());
        LEASE_TARGETS[idx]
    } else {
        let idx = TARGET_INDEX.fetch_add(1, Ordering::Relaxed) % LEASE_TARGETS.len();
        LEASE_TARGETS[idx]
    };

    let _ = topology_control
        .acquire_lease(target, 1, fbroker::LeaseStatus::Satisfied, zx::MonotonicInstant::INFINITE)
        .expect("FIDL acquire_lease call should succeed")
        .expect("acquire lease should succeed");

    let _ = topology_control
        .drop_lease(target, zx::MonotonicInstant::INFINITE)
        .expect("FIDL drop_lease call should succeed")
        .expect("drop lease should succeed");
    Ok(target_affected_elements(target))
}

pub(crate) fn execute_large_shared_topology_lease(
    topology_control: &fpt::TopologyControlSynchronousProxy,
    randomize: bool,
) -> usize {
    black_box(large_shared_topology_lease_work_func(topology_control, randomize).unwrap())
}
