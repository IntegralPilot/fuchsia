// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities that prints information in a human-readable format.

use crate::{ProfileMemoryOutput, digest};
use anyhow::Result;
use digest::processed;
use humansize::{BINARY, format_size};
use processed::RetainedMemory;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::io::Write;

// Returns a sorted list of names that match non-empty vmo groups.
pub fn filter_and_order_vmo_groups_names_for_printing(
    name_to_vmo_memory: &HashMap<String, RetainedMemory>,
) -> Vec<&String> {
    let mut names: Vec<&String> = name_to_vmo_memory.keys().collect();
    // Filter out names of VMOs that don't use any memory.
    names.retain(|&name| name_to_vmo_memory.get(name).unwrap().total_populated > 0);
    // Sort the VMO names along the tuple (private, scaled, name of VMO).
    names.sort_by(|&a, &b| {
        let sa = name_to_vmo_memory.get(a).unwrap();
        let sb = name_to_vmo_memory.get(b).unwrap();
        // Sort along decreasing memory sizes and increasing lexical order for names.
        let tuple_1 = (sa.private, sa.scaled, &b);
        let tuple_2 = (sb.private, sb.scaled, &a);
        tuple_2.cmp(&tuple_1)
    });
    names
}

/// Print to `w` a human-readable presentation of `processes`.
fn print_processes_digest<W: Write>(
    w: &mut W,
    processes: Vec<processed::Process>,
    size_formatter: fn(u64) -> String,
) -> std::io::Result<()> {
    for process in processes {
        writeln!(w, "Process name:         {}", process.name)?;
        writeln!(w, "Process koid:         {}", process.koid)?;

        let vmo_names = filter_and_order_vmo_groups_names_for_printing(&process.name_to_vmo_memory);
        // Find the longest name. Use that length during formatting to align the column after
        // the name of VMOs.
        let process_name_trailing_padding =
            vmo_names.iter().map(|vmo_name| vmo_name.len()).max().unwrap_or(0);
        // The spacing between the columns containing sizes.
        // 12 was chosen to accommodate the following worst case: "1234567890 B"
        let padding_between_number_columns = 12;
        // Write the heading of the table of VMOs.
        writeln!(
            w,
            "    {:<p1$} {:^p2$} {:^p2$} {:^p2$}",
            "",
            "Private",
            "Scaled",
            "Total",
            p1 = process_name_trailing_padding,
            p2 = padding_between_number_columns * 2 + 1
        )?;
        writeln!(
            w,
            "    {:<p1$} {:>p2$} {:>p2$} {:>p2$} {:>p2$} {:>p2$} {:>p2$}",
            "",
            "Committed",
            "Populated",
            "Committed",
            "Populated",
            "Committed",
            "Populated",
            p1 = process_name_trailing_padding,
            p2 = padding_between_number_columns
        )?;
        writeln!(
            w,
            "    {:<p1$} {:>p2$} {:>p2$} {:>p2$} {:>p2$} {:>p2$} {:>p2$}",
            "Total",
            size_formatter(process.memory.private),
            size_formatter(process.memory.private_populated),
            size_formatter(process.memory.scaled),
            size_formatter(process.memory.scaled_populated),
            size_formatter(process.memory.total),
            size_formatter(process.memory.total_populated),
            p1 = process_name_trailing_padding,
            p2 = padding_between_number_columns
        )?;
        writeln!(w)?;
        // Write the actual content of the table of VMOs.
        for vmo_name in vmo_names {
            if let Some(sizes) = process.name_to_vmo_memory.get(vmo_name) {
                if sizes.total != sizes.private {
                    let extra_info = "(shared)";
                    writeln!(
                        w,
                        "    {:<p1$} {:>p2$} {:>p2$} {:>p2$} {:>p2$} {:>p2$} {:>p2$} {:>p2$}",
                        vmo_name,
                        size_formatter(sizes.private),
                        size_formatter(sizes.private_populated),
                        size_formatter(sizes.scaled),
                        size_formatter(sizes.scaled_populated),
                        size_formatter(sizes.total),
                        size_formatter(sizes.total_populated),
                        extra_info,
                        p1 = process_name_trailing_padding,
                        p2 = padding_between_number_columns
                    )?;
                } else {
                    writeln!(
                        w,
                        "    {:<p1$} {:>p2$} {:>p2$} {:>p2$} {:>p2$} {:>p2$} {:>p2$}",
                        vmo_name,
                        size_formatter(sizes.private),
                        size_formatter(sizes.private_populated),
                        size_formatter(sizes.scaled),
                        size_formatter(sizes.scaled_populated),
                        size_formatter(sizes.total),
                        size_formatter(sizes.total_populated),
                        p1 = process_name_trailing_padding,
                        p2 = padding_between_number_columns
                    )?;
                }
            }
        }
        writeln!(w)?;
    }
    Ok(())
}

/// Print to `w` a human-readable presentation of `digest`.
fn print_complete_digest<W: Write>(
    w: &mut W,
    digest: processed::Digest,
    size_formatter: fn(u64) -> String,
) -> std::io::Result<()> {
    writeln!(w, "Time:  {} ns", digest.time)?;
    writeln!(w, "VMO:   {}", size_formatter(digest.total_committed_bytes_in_vmos))?;
    writeln!(w, "Free:  {}", size_formatter(digest.kernel.free))?;
    writeln!(w)?;
    writeln!(w, "Task:      kernel")?;
    writeln!(w, "PID:       1")?;
    let kernel_total = digest.kernel.wired
        + digest.kernel.vmo
        + digest.kernel.total_heap
        + digest.kernel.mmu
        + digest.kernel.ipc;
    writeln!(w, "Total:     {}", size_formatter(kernel_total))?;
    writeln!(w, "    wired: {}", size_formatter(digest.kernel.wired))?;
    writeln!(w, "    vmo:   {}", size_formatter(digest.kernel.vmo))?;
    writeln!(w, "    heap:  {}", size_formatter(digest.kernel.total_heap))?;
    writeln!(w, "    mmu:   {}", size_formatter(digest.kernel.mmu))?;
    writeln!(w, "    ipc:   {}", size_formatter(digest.kernel.ipc))?;
    if let Some(zram) = digest.kernel.zram_compressed_total {
        writeln!(w, "    zram:  {}", size_formatter(zram))?;
        writeln!(w, "    other: {}", size_formatter(digest.kernel.other - zram))?;
    } else {
        writeln!(w, "    other: {}", size_formatter(digest.kernel.other))?;
    }
    writeln!(w)?;

    let sorted_buckets = if let Some(buckets) = digest.buckets {
        let mut sorted_buckets = buckets;
        sorted_buckets.sort_by_key(|bucket| Reverse(bucket.size));
        sorted_buckets
    } else {
        vec![]
    };

    for bucket in sorted_buckets {
        writeln!(w, "Bucket {}: {}", bucket.name, size_formatter(bucket.size))?;
    }
    if let Some(total_undigested) = digest.total_undigested {
        writeln!(w, "Undigested: {}", size_formatter(total_undigested))?;
    }
    print_processes_digest(w, digest.processes, size_formatter)?;
    writeln!(w)?;
    Ok(())
}

/// Print to `w` a human-readable presentation of `output`.
pub fn write_human_readable_output<'a, W: Write>(
    w: &mut W,
    output: ProfileMemoryOutput,
    exact_sizes: bool,
) -> Result<()> {
    let size_to_string_formatter = if exact_sizes {
        |size: u64| size.to_string() + " B"
    } else {
        |size: u64| format_size(size, BINARY)
    };

    match output {
        ProfileMemoryOutput::CompleteDigest(digest) => {
            print_complete_digest(w, *digest, size_to_string_formatter).map_err(|err| {
                match err.kind() {
                    std::io::ErrorKind::BrokenPipe => fho::Error::ExitWithCode(141).into(),
                    _ => fho::Error::Unexpected(err.into()).into(),
                }
            })
        }
        ProfileMemoryOutput::ProcessDigest(processes_digest) => {
            print_processes_digest(w, processes_digest.process_data, size_to_string_formatter)
                .map_err(|err| match err.kind() {
                    std::io::ErrorKind::BrokenPipe => fho::Error::ExitWithCode(141).into(),
                    _ => fho::Error::Unexpected(err.into()).into(),
                })
        }
        ProfileMemoryOutput::ComponentDigest(_) => errors::ffx_bail!("Not supported"),
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::ProfileMemoryOutput::{CompleteDigest, ProcessDigest};
    use crate::bucket::Bucket;
    use crate::plugin_output::ProcessesMemoryUsage;
    use std::collections::HashSet;

    #[test]
    fn filter_and_order_vmo_groups_names_for_printing_test() {
        let map = {
            let mut map = HashMap::new();
            map.insert(
                "xx".to_string(),
                processed::RetainedMemory {
                    private: 0,
                    scaled: 0,
                    total: 0,
                    private_populated: 0,
                    scaled_populated: 0,
                    total_populated: 0,
                    vmos: vec![],
                },
            );
            map.insert(
                "zb".to_string(),
                processed::RetainedMemory {
                    private: 0,
                    scaled: 10,
                    total: 10,
                    private_populated: 0,
                    scaled_populated: 10,
                    total_populated: 10,
                    vmos: vec![],
                },
            );
            map.insert(
                "ab".to_string(),
                processed::RetainedMemory {
                    private: 0,
                    scaled: 10,
                    total: 10,
                    private_populated: 0,
                    scaled_populated: 10,
                    total_populated: 10,
                    vmos: vec![],
                },
            );
            map.insert(
                "zz".to_string(),
                processed::RetainedMemory {
                    private: 0,
                    scaled: 20,
                    total: 20,
                    private_populated: 0,
                    scaled_populated: 20,
                    total_populated: 20,
                    vmos: vec![],
                },
            );
            map
        };
        let output = filter_and_order_vmo_groups_names_for_printing(&map);
        // "xx" gets filtered out because it is empty.
        // "zz" is the biggest, so it's first.
        // "zb" and "ab" are the same size, so they are ordered alphabetically.
        pretty_assertions::assert_eq!(output, vec!["zz", "ab", "zb"]);
    }

    fn data_for_test() -> crate::ProfileMemoryOutput {
        ProcessDigest(Box::new(ProcessesMemoryUsage {
            capture_time: 123,
            process_data: vec![processed::Process {
                koid: processed::ProcessKoid::new(4),
                name: "P".to_string(),
                memory: RetainedMemory {
                    private: 11,
                    scaled: 22,
                    total: 33,
                    private_populated: 11,
                    scaled_populated: 22,
                    total_populated: 33,
                    vmos: vec![],
                },
                name_to_vmo_memory: {
                    let mut result = HashMap::new();
                    result.insert(
                        "vmoC".to_string(),
                        processed::RetainedMemory {
                            private: 4444,
                            scaled: 55555,
                            total: 666666,
                            private_populated: 4444,
                            scaled_populated: 55555,
                            total_populated: 666666,
                            vmos: vec![],
                        },
                    );
                    result.insert(
                        "vmoB".to_string(),
                        processed::RetainedMemory {
                            private: 4444,
                            scaled: 55555,
                            total: 666666,
                            private_populated: 4444,
                            scaled_populated: 55555,
                            total_populated: 666666,
                            vmos: vec![],
                        },
                    );
                    result.insert(
                        "vmoA".to_string(),
                        processed::RetainedMemory {
                            private: 44444,
                            scaled: 555555,
                            total: 6666666,
                            private_populated: 4444,
                            scaled_populated: 55555,
                            total_populated: 666666,
                            vmos: vec![],
                        },
                    );
                    result
                },
                vmos: HashSet::new(),
            }],
        }))
    }

    fn data_for_test_with_compression() -> crate::ProfileMemoryOutput {
        ProcessDigest(Box::new(ProcessesMemoryUsage {
            capture_time: 123,
            process_data: vec![processed::Process {
                koid: processed::ProcessKoid::new(4),
                name: "P".to_string(),
                memory: RetainedMemory {
                    private: 11,
                    scaled: 22,
                    total: 33,
                    private_populated: 1100,
                    scaled_populated: 2200,
                    total_populated: 3300,
                    vmos: vec![],
                },
                name_to_vmo_memory: {
                    let mut result = HashMap::new();
                    result.insert(
                        "shown".to_string(),
                        processed::RetainedMemory {
                            private: 4444,
                            scaled: 55555,
                            total: 0,
                            private_populated: 44440,
                            scaled_populated: 555550,
                            total_populated: 6666660,
                            vmos: vec![],
                        },
                    );
                    result.insert(
                        "hidden".to_string(),
                        processed::RetainedMemory {
                            private: 4444,
                            scaled: 55555,
                            total: 666666,
                            private_populated: 44440,
                            scaled_populated: 555550,
                            total_populated: 0,
                            vmos: vec![],
                        },
                    );
                    result.insert(
                        "vmoC".to_string(),
                        processed::RetainedMemory {
                            private: 4444,
                            scaled: 55555,
                            total: 666666,
                            private_populated: 44440,
                            scaled_populated: 555550,
                            total_populated: 6666660,
                            vmos: vec![],
                        },
                    );
                    result.insert(
                        "vmoB".to_string(),
                        processed::RetainedMemory {
                            private: 4444,
                            scaled: 55555,
                            total: 666666,
                            private_populated: 44440,
                            scaled_populated: 555550,
                            total_populated: 6666660,
                            vmos: vec![],
                        },
                    );
                    result.insert(
                        "vmoA".to_string(),
                        processed::RetainedMemory {
                            private: 44444,
                            scaled: 555555,
                            total: 6666666,
                            private_populated: 44440,
                            scaled_populated: 555550,
                            total_populated: 6666660,
                            vmos: vec![],
                        },
                    );
                    result
                },
                vmos: HashSet::new(),
            }],
        }))
    }

    fn data_for_bucket_test() -> crate::ProfileMemoryOutput {
        CompleteDigest(Box::new(processed::Digest {
            time: 1,
            total_committed_bytes_in_vmos: 1000,
            kernel: processed::Kernel {
                total: 1500,
                free: 100,
                wired: 200,
                total_heap: 100,
                free_heap: 100,
                vmo: 1000,
                vmo_pager_total: 0,
                vmo_pager_newest: 0,
                vmo_pager_oldest: 0,
                vmo_discardable_locked: 0,
                vmo_discardable_unlocked: 0,
                mmu: 0,
                ipc: 0,
                other: 0,
                zram_compressed_total: Some(0),
                zram_uncompressed: Some(0),
                zram_fragmentation: Some(0),
            },
            kmem_stats_compression: Default::default(),
            processes: vec![processed::Process {
                koid: processed::ProcessKoid::new(1),
                name: "process1".to_owned(),
                memory: RetainedMemory {
                    private: 100,
                    private_populated: 100,
                    scaled: 100,
                    scaled_populated: 100,
                    total: 100,
                    total_populated: 100,
                    vmos: vec![processed::VmoKoid::new(10)],
                },
                name_to_vmo_memory: HashMap::from_iter(vec![(
                    "vmo1".to_owned(),
                    processed::RetainedMemory {
                        private: 100,
                        private_populated: 100,
                        scaled: 100,
                        scaled_populated: 100,
                        total: 100,
                        total_populated: 100,
                        vmos: vec![processed::VmoKoid::new(10)],
                    },
                )]),
                vmos: HashSet::from_iter(vec![processed::VmoKoid::new(10)]),
            }],
            vmos: vec![processed::Vmo {
                koid: processed::VmoKoid::new(10),
                name: "vmo1".to_owned(),
                parent_koid: processed::VmoKoid::new(0),
                committed_bytes: 100,
                allocated_bytes: 100,
                populated_bytes: None,
            }],
            buckets: Some(vec![Bucket {
                name: "some_bucket".to_owned(),
                size: 300,
                vmos: HashSet::from_iter(vec![processed::VmoKoid::new(20)]),
            }]),
            total_undigested: Some(200),
        }))
    }

    #[test]
    fn write_human_readable_output_exact_sizes_test() {
        let mut writer = Vec::new();
        let _ = write_human_readable_output(&mut writer, data_for_test(), true);
        let actual_output = std::str::from_utf8(&writer).unwrap();
        let expected_output = r#"Process name:         P|
Process koid:         4|
                  Private                   Scaled                     Total          |
            Committed    Populated    Committed    Populated    Committed    Populated|
    Total         11 B         11 B         22 B         22 B         33 B         33 B|
|
    vmoA      44444 B       4444 B     555555 B      55555 B    6666666 B     666666 B     (shared)|
    vmoB       4444 B       4444 B      55555 B      55555 B     666666 B     666666 B     (shared)|
    vmoC       4444 B       4444 B      55555 B      55555 B     666666 B     666666 B     (shared)|
|
"#;
        pretty_assertions::assert_eq!(actual_output, expected_output.replace("|", ""));
    }

    #[test]
    fn write_human_readable_output_human_friendly_sizes_test() {
        let mut writer = Vec::new();
        let _ = write_human_readable_output(&mut writer, data_for_test(), false);
        let actual_output = std::str::from_utf8(&writer).unwrap();
        let expected_output = r#"Process name:         P|
Process koid:         4|
                  Private                   Scaled                     Total          |
            Committed    Populated    Committed    Populated    Committed    Populated|
    Total         11 B         11 B         22 B         22 B         33 B         33 B|
|
    vmoA    43.40 KiB     4.34 KiB   542.53 KiB    54.25 KiB     6.36 MiB   651.04 KiB     (shared)|
    vmoB     4.34 KiB     4.34 KiB    54.25 KiB    54.25 KiB   651.04 KiB   651.04 KiB     (shared)|
    vmoC     4.34 KiB     4.34 KiB    54.25 KiB    54.25 KiB   651.04 KiB   651.04 KiB     (shared)|
|
"#;
        pretty_assertions::assert_eq!(actual_output, expected_output.replace("|", ""));
    }

    #[test]
    fn write_human_readable_output_human_friendly_sizes_with_compression_test() {
        let mut writer = Vec::new();
        let _ = write_human_readable_output(&mut writer, data_for_test_with_compression(), false);
        let actual_output = std::str::from_utf8(&writer).unwrap();
        let expected_output = r#"Process name:         P|
Process koid:         4|
                   Private                   Scaled                     Total          |
             Committed    Populated    Committed    Populated    Committed    Populated|
    Total         11 B     1.07 KiB         22 B     2.15 KiB         33 B     3.22 KiB|
|
    vmoA     43.40 KiB    43.40 KiB   542.53 KiB   542.53 KiB     6.36 MiB     6.36 MiB     (shared)|
    shown     4.34 KiB    43.40 KiB    54.25 KiB   542.53 KiB          0 B     6.36 MiB     (shared)|
    vmoB      4.34 KiB    43.40 KiB    54.25 KiB   542.53 KiB   651.04 KiB     6.36 MiB     (shared)|
    vmoC      4.34 KiB    43.40 KiB    54.25 KiB   542.53 KiB   651.04 KiB     6.36 MiB     (shared)|
|
"#;
        pretty_assertions::assert_eq!(actual_output, expected_output.replace("|", ""));
    }

    #[test]
    fn write_human_readable_output_buckets_test() {
        let mut writer = Vec::new();
        let _ = write_human_readable_output(&mut writer, data_for_bucket_test(), false);
        let actual_output = std::str::from_utf8(&writer).unwrap();
        let expected_output = r#"Time:  1 ns|
VMO:   1000 B|
Free:  100 B|
|
Task:      kernel|
PID:       1|
Total:     1.27 KiB|
    wired: 200 B|
    vmo:   1000 B|
    heap:  100 B|
    mmu:   0 B|
    ipc:   0 B|
    zram:  0 B|
    other: 0 B|
|
Bucket some_bucket: 300 B|
Undigested: 200 B|
Process name:         process1|
Process koid:         1|
                  Private                   Scaled                     Total          |
            Committed    Populated    Committed    Populated    Committed    Populated|
    Total        100 B        100 B        100 B        100 B        100 B        100 B|
|
    vmo1        100 B        100 B        100 B        100 B        100 B        100 B|
|
|
"#;
        pretty_assertions::assert_eq!(actual_output, expected_output.replace("|", ""));
    }
}
