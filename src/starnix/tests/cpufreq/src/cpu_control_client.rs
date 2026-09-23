// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::str;

const FREQUENCIES_HZ: [&'static str; 2] = ["1128000 1256000 1512000 2024000", "512000 1024000"];
const MAX_FREQUENCIES_HZ: [&'static str; 2] = ["2024000", "1024000"];

fn main() {
    println!("cpu_control_client started");
    check_cpufreq();
    println!("cpu_control_client done");
}

fn check_cpufreq() {
    assert_eq!(
        "0-5\n",
        str::from_utf8(&std::fs::read("/sys/devices/system/cpu/possible").unwrap()).unwrap()
    );
    assert_eq!(
        "0-5\n",
        str::from_utf8(&std::fs::read("/sys/devices/system/cpu/online").unwrap()).unwrap()
    );

    check_cpufreq_dir(0, 0);
    check_cpufreq_dir(1, 0);
    check_cpufreq_dir(2, 1);
    check_cpufreq_dir(3, 1);
    check_cpufreq_dir(4, 1);
    check_cpufreq_dir(5, 1);

    check_cpufreq_policy_dir(0, 0, "0 1");
    check_cpufreq_policy_dir(2, 1, "2 3 4 5");
}

fn check_cpufreq_policy_dir(policy_id: u64, cluster_id: u64, expected_related_cpus: &str) {
    assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpufreq/policy{policy_id}")).unwrap());

    let max_frequency_str = MAX_FREQUENCIES_HZ[cluster_id as usize];
    assert_eq!(
        &format!("{max_frequency_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpufreq/policy{policy_id}/cpuinfo_max_freq"
            ))
            .unwrap()
        )
        .unwrap()
    );

    let frequencies_str = FREQUENCIES_HZ[cluster_id as usize];
    assert_eq!(
        &format!("{frequencies_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpufreq/policy{policy_id}/scaling_available_frequencies"
            ))
            .unwrap()
        )
        .unwrap()
    );

    assert_eq!(
        &format!("{expected_related_cpus}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpufreq/policy{policy_id}/related_cpus"
            ))
            .unwrap()
        )
        .unwrap()
    );

    let cur_frequency_str = MAX_FREQUENCIES_HZ[cluster_id as usize];
    assert_eq!(
        &format!("{cur_frequency_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpufreq/policy{policy_id}/scaling_cur_freq"
            ))
            .unwrap()
        )
        .unwrap()
    );
}

fn check_cpufreq_dir(core_id: u64, cluster_id: u64) {
    assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpu{core_id}")).unwrap());
    assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpu{core_id}/cpufreq")).unwrap());

    let max_frequency_str = MAX_FREQUENCIES_HZ[cluster_id as usize];
    assert_eq!(
        &format!("{max_frequency_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpu{core_id}/cpufreq/cpuinfo_max_freq"
            ))
            .unwrap()
        )
        .unwrap()
    );

    let frequencies_str = FREQUENCIES_HZ[cluster_id as usize];
    assert_eq!(
        &format!("{frequencies_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpu{core_id}/cpufreq/scaling_available_frequencies"
            ))
            .unwrap()
        )
        .unwrap()
    );

    assert_eq!(
        &format!("{max_frequency_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpu{core_id}/cpufreq/scaling_cur_freq"
            ))
            .unwrap()
        )
        .unwrap()
    );

    assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpu{core_id}/topology")).unwrap());
    assert_eq!(
        &format!("{cluster_id}\n"),
        str::from_utf8(
            &std::fs::read(format!("/sys/devices/system/cpu/cpu{core_id}/topology/cluster_id"))
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        &format!("{cluster_id}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpu{core_id}/topology/physical_package_id"
            ))
            .unwrap()
        )
        .unwrap()
    );
}
