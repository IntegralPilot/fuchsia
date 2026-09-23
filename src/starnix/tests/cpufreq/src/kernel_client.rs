// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::str;

fn main() {
    println!("kernel_client started");
    check_cpufreq_kernel_fallback();
    println!("kernel_client done");
}

fn check_cpufreq_kernel_fallback() {
    let possible_bytes = std::fs::read("/sys/devices/system/cpu/possible").unwrap();
    let possible_str = str::from_utf8(&possible_bytes).unwrap().trim();
    assert!(possible_str.starts_with("0-"));
    let max_core: u64 = possible_str.strip_prefix("0-").unwrap().parse().unwrap();
    let cpu_count = max_core + 1;

    let online_bytes = std::fs::read("/sys/devices/system/cpu/online").unwrap();
    let online_str = str::from_utf8(&online_bytes).unwrap().trim();
    assert_eq!(online_str, possible_str);

    for core_id in 0..cpu_count {
        assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpu{core_id}")).unwrap());
        assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpu{core_id}/cpufreq")).unwrap());
        assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpu{core_id}/topology")).unwrap());

        assert_eq!(
            "0\n",
            str::from_utf8(
                &std::fs::read(format!("/sys/devices/system/cpu/cpu{core_id}/topology/cluster_id"))
                    .unwrap()
            )
            .unwrap()
        );
        assert_eq!(
            "0\n",
            str::from_utf8(
                &std::fs::read(format!(
                    "/sys/devices/system/cpu/cpu{core_id}/topology/physical_package_id"
                ))
                .unwrap()
            )
            .unwrap()
        );

        assert_eq!(
            "\n",
            str::from_utf8(
                &std::fs::read(format!(
                    "/sys/devices/system/cpu/cpu{core_id}/cpufreq/scaling_available_frequencies"
                ))
                .unwrap()
            )
            .unwrap()
        );
        assert_eq!(
            "\n",
            str::from_utf8(
                &std::fs::read(format!(
                    "/sys/devices/system/cpu/cpu{core_id}/cpufreq/cpuinfo_max_freq"
                ))
                .unwrap()
            )
            .unwrap()
        );

        assert!(
            std::fs::read(format!("/sys/devices/system/cpu/cpu{core_id}/cpufreq/scaling_cur_freq"))
                .is_err()
        );
    }

    assert!(std::fs::exists("/sys/devices/system/cpu/cpufreq/policy0").unwrap());
    let expected_related_cpus = (0..cpu_count).map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
    assert_eq!(
        &format!("{expected_related_cpus}\n"),
        str::from_utf8(
            &std::fs::read("/sys/devices/system/cpu/cpufreq/policy0/related_cpus").unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        "\n",
        str::from_utf8(
            &std::fs::read("/sys/devices/system/cpu/cpufreq/policy0/scaling_available_frequencies")
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        "\n",
        str::from_utf8(
            &std::fs::read("/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_max_freq").unwrap()
        )
        .unwrap()
    );
    assert!(std::fs::read("/sys/devices/system/cpu/cpufreq/policy0/scaling_cur_freq").is_err());
}
