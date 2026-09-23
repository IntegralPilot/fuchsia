// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use attribution_processing::ZXName;
use attribution_processing::digest::{self};
use attribution_processing::fplugin_serde::KernelStatistics;
use attribution_processing::summary::{
    ComponentSummaryProfileResult, PrincipalSummary, VmoSummary,
};
use fdomain_fuchsia_memory_attribution_plugin as fplugin;
use prettytable::{Table, row, table};
use std::cmp::Reverse;

pub fn write_summary(
    f: &mut dyn std::io::Write,
    csv: bool,
    profile_result: ComponentSummaryProfileResult,
) -> std::io::Result<()> {
    if csv {
        let mut csv_writer = csv::Writer::from_writer(f);
        csv_writer.write_record(&[
            "attributor",
            "principal",
            "vmo",
            "committed_private",
            "populated_private",
            "committed_scaled",
            "populated_scaled",
            "committed_total",
            "populated_total",
        ])?;
        for principal in &profile_result.principals {
            write_summary_principal_csv(&mut csv_writer, principal)?;
        }
    } else {
        write_summary_kernel_stats(f, &profile_result.kernel, &profile_result.performance)?;
        profile_result.digest.map(|d| write_digest(f, d)).transpose()?;
        for principal in &profile_result.principals {
            writeln!(f)?;
            writeln!(f)?;
            write_summary_principal(f, principal)?;
        }
    }
    Ok(())
}

fn write_digest(w: &mut dyn std::io::Write, mut digest: digest::Digest) -> std::io::Result<()> {
    if digest.buckets.len() > 0 {
        writeln!(w, "")?;
    }
    digest.buckets.sort_by_key(|bucket| Reverse(bucket.populated_size));

    for bucket in digest.buckets.iter_mut() {
        writeln!(
            w,
            "Bucket {}: {}",
            bucket.name,
            if bucket.populated_size == bucket.committed_size {
                format!("{}", format_bytes(bucket.populated_size as f64))
            } else {
                format!(
                    "{} (populated), {} (committed)",
                    format_bytes(bucket.populated_size as f64),
                    format_bytes(bucket.committed_size as f64)
                )
            }
        )?;
        for vmos in bucket.vmos.iter_mut() {
            vmos.sort_by_key(|vmo| Reverse(vmo.populated_size));
            for vmo in vmos {
                writeln!(
                    w,
                    "    Vmo {}({}): {}",
                    vmo.name,
                    vmo.principals.join(", "),
                    if vmo.populated_size == vmo.committed_size {
                        format_bytes(vmo.populated_size as f64)
                    } else {
                        format!(
                            "{} (populated), {} (committed)",
                            format_bytes(vmo.populated_size as f64),
                            format_bytes(vmo.committed_size as f64)
                        )
                    },
                )?;
            }
        }
    }
    Ok(())
}

fn write_summary_principal(
    f: &mut dyn std::io::Write,
    value: &PrincipalSummary,
) -> std::io::Result<()> {
    let format = prettytable::format::FormatBuilder::new().padding(1, 1).build();
    let mut tbl = table!(
        ["Principal name:", &value.name],
        ["Principal id:", &value.id.to_string()],
        [
            "Principal type:",
            match value.principal_type.as_str() {
                "R" => "Runnable",
                "P" => "Part",
                o => o,
            }
        ]
    );

    if let Some(parent) = &value.attributor {
        tbl.add_row(row!["Attributor:", parent]);
    }

    if !value.processes.is_empty() {
        tbl.add_row(row!["Processes:", value.processes.join(", ")]);
    }
    tbl.set_format(format);
    tbl.print(f)?;

    let mut vmos: Vec<(&ZXName, &VmoSummary)> = value.vmos.iter().collect();
    vmos.sort_by_key(|(_, v)| -(v.populated_total as i64));
    let mut tbl = Table::new();
    tbl.add_row(
        row![bc -> "VMO name", bc->"Count", bH2c->"Private", bH2c->"Scaled", bH2c->"Total"],
    );
    tbl.add_row(row![bH2 -> "", bc->"Committed", bc->"Populated", bc->"Committed", bc->"Populated", bc->"Committed", bc->"Populated"]);
    tbl.add_row(row![
        "Total",
        "",
        r->format_bytes(value.committed_private as f64),
        r->format_bytes(value.populated_private as f64),
        r->format_bytes(value.committed_scaled),
        r->format_bytes(value.populated_scaled),
        r->format_bytes(value.committed_total as f64),
        r->format_bytes(value.populated_total as f64)
    ]);
    tbl.add_row(row![]);
    for (name, vmo) in vmos {
        tbl.add_row(row![
            name,
            r->vmo.count,
            r->format_bytes(vmo.committed_private as f64),
            r->format_bytes(vmo.populated_private as f64),
            r->format_bytes(vmo.committed_scaled),
            r->format_bytes(vmo.populated_scaled),
            r->format_bytes(vmo.committed_total as f64),
            r->format_bytes(vmo.populated_total as f64)
        ]);
    }
    tbl.set_format(format);
    tbl.print(f)?;
    Ok(())
}

fn write_summary_kernel_stats(
    w: &mut dyn std::io::Write,
    kernel_statistics: &KernelStatistics,
    performance_metrics: &fplugin::PerformanceImpactMetrics,
) -> std::io::Result<()> {
    let memory_statistics = &kernel_statistics.memory_statistics;
    writeln!(w, "Total memory: {}", format_bytes(memory_statistics.total_bytes.unwrap() as f64))?;
    writeln!(w, "Free memory: {}", format_bytes(memory_statistics.free_bytes.unwrap() as f64))?;
    let kernel_total = memory_statistics.wired_bytes.unwrap()
        + memory_statistics.total_heap_bytes.unwrap()
        + memory_statistics.mmu_overhead_bytes.unwrap()
        + memory_statistics.ipc_bytes.unwrap()
        + memory_statistics.other_bytes.unwrap()
        + memory_statistics.slab_bytes.unwrap()
        + memory_statistics.cache_bytes.unwrap();
    writeln!(w, "Kernel:    {}", format_bytes(kernel_total as f64))?;
    writeln!(w, "    wired: {}", format_bytes(memory_statistics.wired_bytes.unwrap() as f64))?;
    writeln!(w, "    vmo:   {}", format_bytes(memory_statistics.vmo_bytes.unwrap() as f64))?;
    writeln!(w, "    heap:  {}", format_bytes(memory_statistics.total_heap_bytes.unwrap() as f64))?;
    writeln!(
        w,
        "    mmu:   {}",
        format_bytes(memory_statistics.mmu_overhead_bytes.unwrap() as f64)
    )?;
    writeln!(w, "    ipc:   {}", format_bytes(memory_statistics.ipc_bytes.unwrap() as f64))?;
    if let Some(zram_bytes) = memory_statistics.zram_bytes {
        writeln!(w, "    zram:  {}", format_bytes(zram_bytes as f64))?;
    }
    writeln!(w, "    slab:  {}", format_bytes(memory_statistics.slab_bytes.unwrap() as f64))?;
    writeln!(w, "    cache: {}", format_bytes(memory_statistics.cache_bytes.unwrap() as f64))?;
    writeln!(w, "    other: {}", format_bytes(memory_statistics.other_bytes.unwrap() as f64))?;
    writeln!(w, "  including:")?;
    writeln!(
        w,
        "    vmo_reclaim_total_bytes:        {}",
        format_bytes(memory_statistics.vmo_reclaim_total_bytes.unwrap() as f64)
    )?;
    writeln!(
        w,
        "    vmo_reclaim_newest_bytes:       {}",
        format_bytes(memory_statistics.vmo_reclaim_newest_bytes.unwrap() as f64)
    )?;
    writeln!(
        w,
        "    vmo_reclaim_oldest_bytes:       {}",
        format_bytes(memory_statistics.vmo_reclaim_oldest_bytes.unwrap() as f64)
    )?;
    writeln!(
        w,
        "    vmo_reclaim_disabled_bytes:     {}",
        format_bytes(memory_statistics.vmo_reclaim_disabled_bytes.unwrap() as f64)
    )?;
    writeln!(
        w,
        "    vmo_discardable_locked_bytes:   {}",
        format_bytes(memory_statistics.vmo_discardable_locked_bytes.unwrap() as f64)
    )?;
    writeln!(
        w,
        "    vmo_discardable_unlocked_bytes: {}",
        format_bytes(memory_statistics.vmo_discardable_unlocked_bytes.unwrap() as f64)
    )?;
    writeln!(
        w,
        "  Memory stalls (some): {}",
        format_nanoseconds(performance_metrics.some_memory_stalls_ns.unwrap() as f64)
    )?;
    writeln!(
        w,
        "  Memory stalls (full): {}",
        format_nanoseconds(performance_metrics.full_memory_stalls_ns.unwrap() as f64)
    )?;
    // Page refaults are the total number of page faults of user-pager backed pages, for pages that
    // were previously loaded, then unloaded due to memory pressure. This is reported by user
    // pagers directly, so may underestimate the true count on the whole system (due to some user
    // pagers not integrating with the reporting protocol).
    writeln!(w, "  Page refaults: {}", performance_metrics.page_refaults.unwrap())
}

fn write_summary_principal_csv<W: std::io::Write>(
    csv_writer: &mut csv::Writer<W>,
    value: &PrincipalSummary,
) -> std::io::Result<()> {
    let mut vmos: Vec<(&ZXName, &VmoSummary)> = value.vmos.iter().collect();
    vmos.sort_by_key(|(_, v)| -(v.populated_total as i64));
    for (name, vmo) in vmos {
        csv_writer.write_record(&[
            value.attributor.clone().unwrap_or_default(),
            value.name.to_string(),
            name.to_string(),
            vmo.committed_private.to_string(),
            vmo.populated_private.to_string(),
            vmo.committed_scaled.to_string(),
            vmo.populated_scaled.to_string(),
            vmo.committed_total.to_string(),
            vmo.populated_total.to_string(),
        ])?;
    }

    csv_writer.write_record(&[
        value.attributor.clone().unwrap_or_default(),
        value.name.to_string(),
        "Total".to_string(),
        value.committed_private.to_string(),
        value.populated_private.to_string(),
        value.committed_scaled.to_string(),
        value.populated_scaled.to_string(),
        value.committed_total.to_string(),
        value.populated_total.to_string(),
    ])?;
    Ok(())
}

pub fn format_bytes(bytes: f64) -> String {
    if bytes < 1024.0 {
        format!("{:0.2} B", bytes)
    } else if bytes / 1024.0 < 1024.0 {
        format!("{:0.2} KiB", bytes / 1024.0)
    } else {
        format!("{:0.2} MiB", bytes / (1024.0 * 1024.0))
    }
}

pub fn format_nanoseconds(time: f64) -> String {
    if time < 1000.0 {
        format!("{:0} ns", time)
    } else if time / 1000.0 < 1000.0 {
        format!("{:0.2} µs", time / 1000.0)
    } else if time / (1000.0 * 1000.0) < 1000.0 {
        format!("{:0.2} ms", time / (1000.0 * 1000.0))
    } else {
        format!("{:0.3} s", time / (1000.0 * 1000.0 * 1000.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::BufWriter;

    #[test]
    fn principal_output_string() {
        let po = PrincipalSummary {
            id: 42,
            name: String::from("test_name"),
            principal_type: String::from("R"),
            committed_private: 100,
            committed_scaled: 200.0,
            committed_total: 300,
            populated_private: 400,
            populated_scaled: 500.0,
            populated_total: 600,
            attributor: None,
            processes: vec![String::from("proc_a"), String::from("proc_b")],
            vmos: HashMap::from([(
                ZXName::from_string_lossy("[scudo]"),
                VmoSummary {
                    count: 42,
                    committed_private: 10,
                    committed_scaled: 20.0,
                    committed_total: 30,
                    populated_private: 40,
                    populated_scaled: 50.0,
                    populated_total: 60,
                },
            )]),
        };

        let actual_output = {
            let mut buf = BufWriter::new(Vec::new());
            write_summary_principal(&mut buf, &po).unwrap();

            let bytes = buf.into_inner().unwrap();
            String::from_utf8(bytes).unwrap()
        };

        let expected_output = r#" Principal name:  test_name |
 Principal id:    42 |
 Principal type:  Runnable |
 Processes:       proc_a, proc_b |
 VMO name  Count        Private                Scaled                Total         |
                  Committed  Populated  Committed  Populated  Committed  Populated |
 Total             100.00 B   400.00 B   200.00 B   500.00 B   300.00 B   600.00 B |
                                                                          |
 [scudo]      42    10.00 B    40.00 B    20.00 B    50.00 B    30.00 B    60.00 B |
"#;
        pretty_assertions::assert_eq!(actual_output, expected_output.replace("|", ""));
    }

    #[test]
    fn test_write_summary() {
        let principal = PrincipalSummary {
            id: 42,
            name: String::from("test_name"),
            principal_type: String::from("R"),
            committed_private: 1,
            committed_scaled: 2.0,
            committed_total: 3,
            populated_private: 4,
            populated_scaled: 5.0,
            populated_total: 6,
            attributor: Some(String::from("mr,freeze")),
            processes: vec![],
            vmos: HashMap::from([(
                ZXName::from_string_lossy("[scudo]"),
                VmoSummary {
                    count: 42,
                    committed_private: 10,
                    committed_scaled: 20.0,
                    committed_total: 30,
                    populated_private: 40,
                    populated_scaled: 50.0,
                    populated_total: 60,
                },
            )]),
        };

        let actual_output = {
            let mut buf = BufWriter::new(Vec::new());
            write_summary(
                &mut buf,
                true,
                ComponentSummaryProfileResult {
                    principals: vec![principal],
                    unclaimed: 1,
                    ..Default::default()
                },
            )
            .unwrap();
            let bytes = buf.into_inner().unwrap();
            String::from_utf8(bytes).unwrap()
        };
        let expected_output = r#"attributor,principal,vmo,committed_private,populated_private,committed_scaled,populated_scaled,committed_total,populated_total
"mr,freeze",test_name,[scudo],10,40,20,50,30,60
"mr,freeze",test_name,Total,1,4,2,5,3,6
"#;
        pretty_assertions::assert_eq!(actual_output, expected_output);
    }

    #[test]
    fn test_write_summary_scarce_info() {
        let principal = PrincipalSummary {
            id: 42,
            name: String::from("test_name"),
            principal_type: String::from("R"),
            committed_private: 1,
            committed_scaled: 2.0,
            committed_total: 3,
            populated_private: 4,
            populated_scaled: 5.0,
            populated_total: 6,
            attributor: None,
            processes: vec![],
            vmos: HashMap::from([]),
        };

        let actual_output = {
            let mut buf = BufWriter::new(Vec::new());
            write_summary(
                &mut buf,
                true,
                ComponentSummaryProfileResult {
                    principals: vec![principal],
                    unclaimed: 1,
                    ..Default::default()
                },
            )
            .unwrap();
            let bytes = buf.into_inner().unwrap();
            String::from_utf8(bytes).unwrap()
        };
        let expected_output = r#"attributor,principal,vmo,committed_private,populated_private,committed_scaled,populated_scaled,committed_total,populated_total
,test_name,Total,1,4,2,5,3,6
"#;
        pretty_assertions::assert_eq!(actual_output, expected_output);
    }

    #[test]
    fn kernel_stats_output_string() {
        let kernel_statistics: KernelStatistics = KernelStatistics {
            memory_statistics: fdomain_fuchsia_kernel::MemoryStats {
                total_bytes: Some(42 * 1024 * 1024),
                free_bytes: Some(1024),
                wired_bytes: Some(2 * 1024),
                total_heap_bytes: Some(3 * 1024),
                free_heap_bytes: Some(4 * 1024),
                vmo_bytes: Some(5 * 1024),
                mmu_overhead_bytes: Some(6 * 1024),
                ipc_bytes: Some(7 * 1024),
                other_bytes: Some(8 * 1024),
                free_loaned_bytes: Some(9 * 1024),
                cache_bytes: Some(10 * 1024),
                slab_bytes: Some(11 * 1024),
                zram_bytes: Some(12 * 1024),
                vmo_reclaim_total_bytes: Some(13 * 1024),
                vmo_reclaim_newest_bytes: Some(14 * 1024),
                vmo_reclaim_oldest_bytes: Some(15 * 1024),
                vmo_reclaim_disabled_bytes: Some(16 * 1024),
                vmo_discardable_locked_bytes: Some(17 * 1024),
                vmo_discardable_unlocked_bytes: Some(18 * 1024),
                ..Default::default()
            },
            compression_statistics: fdomain_fuchsia_kernel::MemoryStatsCompression::default(),
            ..Default::default()
        };
        let performance_metrics = fplugin::PerformanceImpactMetrics {
            some_memory_stalls_ns: Some(1000 * 1000 * 2048),
            full_memory_stalls_ns: Some(1000 * 1000 * 512),
            page_refaults: Some(256),
            ..Default::default()
        };

        let actual_output = {
            let mut buf = BufWriter::new(Vec::new());
            write_summary_kernel_stats(&mut buf, &(kernel_statistics.into()), &performance_metrics)
                .unwrap();

            let bytes = buf.into_inner().unwrap();
            String::from_utf8(bytes).unwrap()
        };

        let expected_output = r#"Total memory: 42.00 MiB
Free memory: 1.00 KiB
Kernel:    47.00 KiB
    wired: 2.00 KiB
    vmo:   5.00 KiB
    heap:  3.00 KiB
    mmu:   6.00 KiB
    ipc:   7.00 KiB
    zram:  12.00 KiB
    slab:  11.00 KiB
    cache: 10.00 KiB
    other: 8.00 KiB
  including:
    vmo_reclaim_total_bytes:        13.00 KiB
    vmo_reclaim_newest_bytes:       14.00 KiB
    vmo_reclaim_oldest_bytes:       15.00 KiB
    vmo_reclaim_disabled_bytes:     16.00 KiB
    vmo_discardable_locked_bytes:   17.00 KiB
    vmo_discardable_unlocked_bytes: 18.00 KiB
  Memory stalls (some): 2.048 s
  Memory stalls (full): 512.00 ms
  Page refaults: 256
"#;
        pretty_assertions::assert_eq!(actual_output, expected_output);
    }
}
