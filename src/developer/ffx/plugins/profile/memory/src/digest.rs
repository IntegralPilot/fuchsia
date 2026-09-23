// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Capture and Digest related functionality.

/// Types and utilities related to the raw data produced by
/// `MemoryMonitor`.
pub mod raw {
    use serde::{Deserialize, Serialize};
    /// Slightly modified copy of the structure returned by the
    /// `zx_object_get_info` syscall when invoked with the
    /// `ZX_INFO_KMEM_STATS_EXTENDED` topic. Refer to this syscall's
    /// documentation (and implementation) for more details.
    ///
    /// Some notable points:
    ///   * Some memory is not covered (because it belongs to an
    ///   uncovered category).
    ///   * Some memory is counted twice in different fields
    ///   (`total_heap` counts memory in `free_heap`, for instance)
    ///   * Data collection is racy and best effort, which can lead to
    ///   some inaccuracies (a page can be counted once in allocated
    ///   and free memory, for instance, if it was allocated or
    ///   deallocated during the collection).
    ///   * The report is from a kernel centric point of view, and
    ///   distinguishes data reserved to the kernel from data reserved
    ///   to userspace.
    ///   * The report assumes expert knowledge in memory
    ///   management. This type includes every single field of the
    ///   report for completion and to facilitate deserialization, but
    ///   it is likely that only a small subset of its fields will
    ///   ever get used in this crate.
    #[derive(Serialize, Deserialize, PartialEq, Debug, Default, Clone)]
    pub struct Kernel {
        /// Total physical memory available to the system, in bytes.
        pub total: u64,
        /// Unallocated memory, in bytes.
        pub free: u64,
        /// Memory reserved by and mapped into the kernel for reasons
        /// not covered by other fields in this struct, in
        /// bytes. Typically for readonly data like the ram disk and
        /// kernel image, and for early-boot dynamic memory.
        pub wired: u64,
        /// Memory allocated to the kernel heap, in bytes.
        pub total_heap: u64,
        /// Portion of `total_heap` that is not in use, in bytes.
        pub free_heap: u64,
        /// Memory committed to (reserved for, but not necessarily
        /// used by) VMOs, both kernel and user, in bytes. A superset
        /// of all userspace memory. Does not include certain VMOs
        /// that fall under `wired`.
        pub vmo: u64,
        /// Memory committed to pager-backed VMOs, in bytes.
        pub vmo_pager_total: u64,
        /// Memory committed to pager-backed VMOs, in bytes, that has
        /// been most recently accessed, and would not be eligible for
        /// eviction by the kernel under memory pressure.
        pub vmo_pager_newest: u64,
        /// Memory committed to pager-backed VMOs, in bytes, that has
        /// been least recently accessed, and would be the first to be
        /// evicted by the kernel under memory pressure.
        pub vmo_pager_oldest: u64,
        /// Memory committed to discardable VMOs, in bytes, that is
        /// currently locked, or unreclaimable by the kernel under
        /// memory pressure.
        pub vmo_discardable_locked: u64,
        /// Memory committed to discardable VMOs, in bytes, that is
        /// currently unlocked, or reclaimable by the kernel under
        /// memory pressure.
        pub vmo_discardable_unlocked: u64,
        /// Memory used for architecture-specific MMU (Memory
        /// Management Unit) metadata like page tables, in bytes.
        pub mmu: u64,
        /// Memory in use by IPC, in bytes.
        pub ipc: u64,
        /// Non-free memory that isn't accounted for in any other
        /// field, in bytes.
        pub other: u64,
        /// On a system with zRAM, the size in bytes of all memory,
        /// including metadata, fragmentation and other overheads, of
        /// the compressed memory area.
        #[serde(default)]
        pub zram_compressed_total: Option<u64>,
        /// On a system with zRAM, the size in bytes of the content that
        /// is currently being compressed and stored.
        #[serde(default)]
        pub zram_uncompressed: Option<u64>,
        /// On a system with zRAM, the size in bytes of any fragmentation
        /// in the compressed memory area.
        #[serde(default)]
        pub zram_fragmentation: Option<u64>,
    }

    /// Placeholder to validate the JSON schema. None of those fields
    /// are ever used, but they are documented here as a reference.
    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    pub struct ProcessHeaders {
        pub koid: String,
        pub name: String,
        pub vmos: String,
    }

    impl Default for ProcessHeaders {
        fn default() -> ProcessHeaders {
            ProcessHeaders {
                koid: "koid".to_string(),
                name: "name".to_string(),
                vmos: "vmos".to_string(),
            }
        }
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    pub struct ProcessData {
        /// Kernel Object ID. See related Fuchsia Kernel concept.
        pub koid: u64,
        pub name: String,
        pub vmos: Vec<u64>,
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    #[serde(untagged)]
    pub enum Process {
        /// Headers describing the meaning of the data.
        Headers(ProcessHeaders),
        /// The actual data.
        Data(ProcessData),
    }

    /// Placeholder to validate the JSON schema. None of those fields
    /// are ever used, but they are documented here as a reference.
    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    pub struct VmoHeaders {
        pub koid: String,
        pub name: String,
        pub parent_koid: String,
        pub committed_bytes: String,
        pub allocated_bytes: String,
        #[serde(default)]
        pub populated_bytes: Option<String>,
    }

    impl Default for VmoHeaders {
        fn default() -> Self {
            VmoHeaders {
                koid: "koid".to_string(),
                name: "name".to_string(),
                parent_koid: "parent_koid".to_string(),
                committed_bytes: "committed_bytes".to_string(),
                allocated_bytes: "allocated_bytes".to_string(),
                populated_bytes: Some("populated_bytes".to_string()),
            }
        }
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    pub struct VmoData {
        /// Kernel Object ID. See related Fuchsia Kernel concept.
        pub koid: u64,
        pub name: u64,
        pub parent_koid: u64,
        pub committed_bytes: u64,
        pub allocated_bytes: u64,
        #[serde(default)]
        pub populated_bytes: Option<u64>,
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    #[serde(untagged)]
    pub enum Vmo {
        /// Headers describing the meaning of the data.
        Headers(VmoHeaders),
        /// The actual data.
        Data(VmoData),
    }

    /// Capture exported by `MemoryMonitor`.
    /// This part of the schema that contains information on the
    /// processes and VMOs.
    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    #[serde(rename_all = "PascalCase")]
    pub struct Capture {
        /// A monotonic time (in ns).
        pub time: u64,
        pub kernel: Kernel,
        #[serde(
            rename = "kmem_stats_compression",  // Override PascalCase.
            default,
            with = "attribution_processing::fkernel_serde::MemoryStatsCompressionDef"
        )]
        pub kmem_stats_compression: fdomain_fuchsia_kernel::MemoryStatsCompression,
        pub processes: Vec<Process>,
        /// Names of the VMOs mentioned in the `Capture`.
        pub vmo_names: Vec<String>,
        pub vmos: Vec<Vmo>,
    }

    /// Criteria for VMO matching within a bucket and for publishing aggregated metrics.
    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    pub struct BucketDefinition {
        /// Cobalt event code associated with this bucket.
        pub event_code: u64,
        /// Human-readable name of the bucket.
        pub name: String,
        /// String saying which process to match. Will be interpreted as a regex.
        /// If the string is empty, will be interpreted as ".*".
        pub process: String,
        /// Regex saying which VMOs to match. Will be interpreted as a regex.
        /// If the string is empty, will be interpreted as ".*".
        pub vmo: String,
    }

    /// Digests exported by `MemoryMonitor`.
    /// This corresponds to the schema of the data that is transferred
    /// by `MemoryMonitor::WriteJsonCaptureAndBuckets`'s API.
    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    pub struct MemoryMonitorOutput {
        #[serde(rename(serialize = "Buckets", deserialize = "Buckets"))]
        pub buckets_definitions: Vec<BucketDefinition>,
        #[serde(rename(serialize = "Capture", deserialize = "Capture"))]
        pub capture: Capture,
    }
}

/// Types and utilities to produce and manipulate processed summaries
/// suitable for user-facing consumption.
pub mod processed {
    use crate::bucket::{Bucket, compute_buckets};
    use crate::digest::{processed, raw};
    use serde::Serialize;

    use std::collections::{HashMap, HashSet};
    use std::fmt::Display;

    /// Koid of a process.
    #[derive(Serialize, PartialEq, Eq, Hash, Debug, Clone, Copy, PartialOrd, Ord)]
    pub struct ProcessKoid(u64);

    impl ProcessKoid {
        pub fn new(raw_koid: u64) -> Self {
            ProcessKoid(raw_koid)
        }
    }

    impl Display for ProcessKoid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    /// Koid of a VMO.
    #[derive(Serialize, PartialEq, Eq, Hash, Debug, Clone, Copy, PartialOrd, Ord)]
    pub struct VmoKoid(u64);

    impl VmoKoid {
        pub fn new(raw_koid: u64) -> Self {
            VmoKoid(raw_koid)
        }
    }

    impl Display for VmoKoid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    /// Per process memory attribution.
    #[derive(Serialize, PartialEq, Debug, Default, Clone)]
    pub struct RetainedMemory {
        /// Total size, in bytes, of VMOs exclusively retained
        /// (directly, or indirectly via children VMOs) by the
        /// process.
        pub private: u64,
        /// Total uncompressed size, in bytes, of the VMOs contributing to `private`.
        pub private_populated: u64,
        /// Total size, in bytes, of VMOs retained (directly, or
        /// indirectly via children VMOs) by several processes. The
        /// cost of each VMO is shared evenly among all its retaining
        /// processes.
        pub scaled: u64,
        /// Total uncompressed size, in bytes, of the VMOs contributing to `scaled`.
        pub scaled_populated: u64,
        /// Total size, in bytes, of VMOs retained (exclusively or
        /// not, directly, or indirectly via children VMOs) by this
        /// process.
        pub total: u64,
        /// Total uncompressed size, in bytes, of the VMOs contributing to `total`.
        pub total_populated: u64,
        /// List of VMOs aggregated in this group.
        pub vmos: Vec<VmoKoid>,
    }

    impl RetainedMemory {
        fn add_vmo(&mut self, vmo: &processed::Vmo, share_count: u64) {
            self.total += vmo.committed_bytes;
            self.total_populated += vmo.populated_bytes.unwrap_or(vmo.committed_bytes);
            self.scaled += vmo.committed_bytes / share_count;
            self.scaled_populated +=
                vmo.populated_bytes.unwrap_or(vmo.committed_bytes) / share_count;
            if share_count == 1 {
                self.private += vmo.committed_bytes;
                self.private_populated += vmo.populated_bytes.unwrap_or(vmo.committed_bytes);
            }
        }
    }

    /// Summary of memory-related data for a given process.
    #[derive(Serialize, PartialEq, Debug, Clone)]
    pub struct Process {
        /// Kernel Object ID. See related Fuchsia Kernel concept.
        pub koid: ProcessKoid,
        pub name: String,
        pub memory: RetainedMemory,
        /// Mapping between VMO group names and the retained memory.
        /// VMO are grouped by name matching. See `rename` for more detail.
        pub name_to_vmo_memory: HashMap<String, RetainedMemory>,
        /// Set of vmo koids related to this process.
        pub vmos: HashSet<VmoKoid>,
    }

    /// Holds all the data relevant to a Vmo.
    #[derive(Serialize, PartialEq, Debug)]
    pub struct Vmo {
        pub koid: VmoKoid,
        pub name: String,
        pub parent_koid: VmoKoid,
        pub committed_bytes: u64,
        pub allocated_bytes: u64,
        #[serde(default)]
        pub populated_bytes: Option<u64>,
    }

    pub type Kernel = raw::Kernel;

    /// Aggregated, processed digest of memory use in a system.
    #[derive(Serialize, PartialEq, Debug)]
    pub struct Digest {
        /// A monotonic time (in ns).
        pub time: u64,
        /// The sum of all the committed bytes in all VMOs.
        pub total_committed_bytes_in_vmos: u64,
        /// Kernel data.
        pub kernel: Kernel,
        #[serde(with = "attribution_processing::fkernel_serde::MemoryStatsCompressionDef")]
        pub kmem_stats_compression: fdomain_fuchsia_kernel::MemoryStatsCompression,
        /// Process data.
        pub processes: Vec<Process>,
        /// Details about VMOs.
        pub vmos: Vec<Vmo>,
        /// Buckets
        pub buckets: Option<Vec<Bucket>>,
        /// The sum of all the committed bytes not in a bucket.
        pub total_undigested: Option<u64>,
    }

    /// Conversion trait from a raw digest to a human-friendly
    /// processed digest. Data is aggregated, normalized, sorted.
    /// Arguments:
    ///   - `memory_monitor_output`: data from the device memory monitor
    ///   - `bucketize`: if true, bins the memory into memory buckets
    ///   - `undigest_view`: if true, only return the memory which isn't part of any memory bucket
    pub fn digest_from_memory_monitor_output(
        memory_monitor_output: raw::MemoryMonitorOutput,
        bucketize: bool,
        undigest_view: bool,
    ) -> Digest {
        let capture = memory_monitor_output.capture;
        // Index processes by koid.
        let koid_to_process = {
            let mut koid_to_process = HashMap::new();
            for process in capture.processes {
                if let raw::Process::Data(raw::ProcessData { koid, .. }) = process {
                    koid_to_process.insert(koid, process);
                }
            }
            koid_to_process
        };
        // Index VMOs by koid.
        let koid_to_vmo = {
            let mut koid_to_vmo = HashMap::new();
            for vmo in capture.vmos {
                if let raw::Vmo::Data(raw::VmoData {
                    koid,
                    parent_koid,
                    name,
                    committed_bytes,
                    allocated_bytes,
                    populated_bytes,
                }) = vmo
                {
                    let vmo_name_index = name as usize;
                    let vmo_name_string = capture.vmo_names[vmo_name_index].clone();
                    koid_to_vmo.insert(
                        VmoKoid::new(koid),
                        processed::Vmo {
                            koid: VmoKoid::new(koid),
                            parent_koid: VmoKoid::new(parent_koid),
                            name: vmo_name_string,
                            committed_bytes,
                            allocated_bytes,
                            populated_bytes,
                        },
                    );
                }
            }
            koid_to_vmo
        };
        let mut processes: Vec<Process> = {
            let mut processes = vec![];
            for (koid, process) in koid_to_process {
                if let raw::Process::Data(raw::ProcessData { name, vmos, .. }) = process {
                    let p = Process {
                        koid: ProcessKoid::new(koid),
                        name: name,
                        memory: RetainedMemory::default(),
                        name_to_vmo_memory: HashMap::new(),
                        vmos: HashSet::from_iter(vmos.into_iter().map(VmoKoid::new)),
                    };
                    if !p.vmos.is_empty() {
                        processes.push(p);
                    }
                }
            }
            processes
        };

        let (buckets, total_undigested) = if bucketize || undigest_view {
            let (buckets, undigested) = compute_buckets(
                &memory_monitor_output.buckets_definitions,
                &processes,
                &koid_to_vmo,
            );

            let total_undigested = Some(
                undigested
                    .iter()
                    .map(|vmo| koid_to_vmo.get(vmo).unwrap().committed_bytes)
                    .sum::<u64>(),
            );
            if undigest_view {
                for process in processes.iter_mut() {
                    // Only keep undigested VMOs when the undigested view is requested. The other VMOs are hidden and discarded.
                    process.vmos.retain(|vmo| undigested.contains(vmo));
                }
                processes.retain(|process| !process.vmos.is_empty());
            }
            if bucketize { (Some(buckets), total_undigested) } else { (None, total_undigested) }
        } else {
            (None, None)
        };

        // Mapping from each VMO koid to the set of every processes that refer
        // this VMO, either directly, or indirectly via related VMOs.
        // Also maps process to VMOs either directly, or indirectly.
        let (vmo_to_charged_processes, process_to_charged_vmos) = {
            let mut vmo_to_charged_processes: HashMap<VmoKoid, HashSet<ProcessKoid>> =
                HashMap::new();
            let mut process_to_charged_vmos: HashMap<ProcessKoid, HashSet<VmoKoid>> =
                HashMap::new();
            for process in processes.iter() {
                for mut vmo_koid in process.vmos.iter() {
                    // In case of related VMOs, follow parents until reaching
                    // the root VMO.
                    while *vmo_koid != VmoKoid::new(0) {
                        vmo_to_charged_processes.entry(*vmo_koid).or_default().insert(process.koid);
                        process_to_charged_vmos.entry(process.koid).or_default().insert(*vmo_koid);
                        if let Some(processed::Vmo { parent_koid, .. }) = koid_to_vmo.get(&vmo_koid)
                        {
                            if *vmo_koid == *parent_koid {
                                // TODO(https://fxbug.dev/439837320): IOBuffer hierarchy has cycles.
                                // Once fixed we should log an error when a cycle is detected.
                                break;
                            }
                            vmo_koid = parent_koid;
                        } else {
                            // If we reach this branch, it means that the report
                            // mentions a process that holds a handle to a VMO,
                            // and that either this VMO or one of its ascendants
                            // is absent from the VMO list.
                            // This can happen because the report producer
                            // (memory_monitor) does not generate the report
                            // using an atomic view of the system, so some
                            // inconsistencies like this are expected.
                            eprintln!(
                                "[stderr] Process {:?} refers (directly or indirectly) to unknown VMO {}",
                                process.koid, vmo_koid
                            );
                            break;
                        }
                    }
                }
            }
            (vmo_to_charged_processes, process_to_charged_vmos)
        };

        // Compute per-process, aggregated sizes.
        for process in processes.iter_mut() {
            if let Some(vmo_koids) = process_to_charged_vmos.get(&process.koid) {
                for vmo_koid in vmo_koids.iter() {
                    if let Some(vmo) = koid_to_vmo.get(&vmo_koid) {
                        let share_count = match vmo_to_charged_processes.get(&vmo_koid) {
                            Some(v) => v.len() as u64,
                            None => unreachable!(),
                        };
                        let name =
                            attribution_processing::summary::vmo_name_to_digest_name(&vmo.name)
                                .to_string();
                        let name_sizes = process.name_to_vmo_memory.entry(name).or_default();
                        name_sizes.vmos.push(*vmo_koid);
                        name_sizes.add_vmo(vmo, share_count);
                        process.memory.add_vmo(vmo, share_count);
                    }
                }
            }
        }

        processes.sort_by(|a, b| (b.memory.private, &b.name).cmp(&(a.memory.private, &a.name)));

        let total_committed_vmo = {
            let mut total = 0;
            for (_, vmo) in &koid_to_vmo {
                total += vmo.committed_bytes;
            }
            total
        };
        let kernel_vmo = capture.kernel.vmo.saturating_sub(total_committed_vmo);

        Digest {
            time: capture.time,
            total_committed_bytes_in_vmos: total_committed_vmo,
            kernel: raw::Kernel { vmo: kernel_vmo, ..capture.kernel },
            kmem_stats_compression: capture.kmem_stats_compression,
            processes,
            vmos: koid_to_vmo.into_values().collect(),
            buckets,
            total_undigested,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket::Bucket;
    use crate::raw::BucketDefinition;
    use std::collections::{HashMap, HashSet};

    /// Returns a list of references to processes sorted by koid.
    fn sorted_by_koid(processes: &Vec<processed::Process>) -> Vec<&processed::Process> {
        let mut vec_of_refs: Vec<&processed::Process> = processes.iter().collect();
        vec_of_refs.sort_by(|a, b| a.koid.cmp(&b.koid));
        vec_of_refs
    }
    /// Returns a list of references to processes sorted by koid.
    fn sorted_vmos_by_koid(processes: &Vec<processed::Vmo>) -> Vec<&processed::Vmo> {
        let mut vec_of_refs: Vec<&processed::Vmo> = processes.iter().collect();
        vec_of_refs.sort_by(|a, b| a.koid.cmp(&b.koid));
        vec_of_refs
    }

    #[test]
    fn raw_to_processed_test() {
        let kmem_stats_compression = fdomain_fuchsia_kernel::MemoryStatsCompression {
            uncompressed_storage_bytes: Some(1),
            compressed_storage_bytes: Some(2),
            compressed_fragmentation_bytes: Some(3),
            compression_time: Some(4),
            decompression_time: Some(5),
            total_page_compression_attempts: Some(6),
            failed_page_compression_attempts: Some(7),
            total_page_decompressions: Some(8),
            compressed_page_evictions: Some(9),
            eager_page_compressions: Some(10),
            memory_pressure_page_compressions: Some(11),
            critical_memory_page_compressions: Some(12),
            pages_decompressed_unit_ns: Some(13),
            pages_decompressed_within_log_time: None,
            ..Default::default()
        };
        let capture = raw::Capture {
            time: 1234567,
            kernel: raw::Kernel::default(),
            kmem_stats_compression: kmem_stats_compression.clone(),
            processes: vec![
                raw::Process::Headers(raw::ProcessHeaders::default()),
                // Process with one shared, root VMO
                raw::Process::Data(raw::ProcessData {
                    koid: 2,
                    name: "process2".to_string(),
                    vmos: vec![1],
                }),
                // Process with two VMOs
                raw::Process::Data(raw::ProcessData {
                    koid: 3,
                    name: "process3".to_string(),
                    vmos: vec![1, 2],
                }),
                // Process with one private, root VMO
                raw::Process::Data(raw::ProcessData {
                    koid: 4,
                    name: "process4".to_string(),
                    vmos: vec![3],
                }),
                // Process with one child VMO
                raw::Process::Data(raw::ProcessData {
                    koid: 5,
                    name: "process5".to_string(),
                    vmos: vec![2],
                }),
                // Process with one child VMO
                raw::Process::Data(raw::ProcessData {
                    koid: 6,
                    name: "process6".to_string(),
                    vmos: vec![4],
                }),
            ],
            vmo_names: vec![
                "vmo1".to_string(),
                "vmo2".to_string(),
                "vmo3".to_string(),
                "vmo4".to_string(),
            ],
            vmos: vec![
                raw::Vmo::Headers(raw::VmoHeaders::default()),
                raw::Vmo::Data(raw::VmoData {
                    koid: 1,
                    name: 0,
                    parent_koid: 0,
                    committed_bytes: 300,
                    allocated_bytes: 300,
                    populated_bytes: Some(300),
                }),
                raw::Vmo::Data(raw::VmoData {
                    koid: 2,
                    name: 1,
                    parent_koid: 1,
                    committed_bytes: 100,
                    allocated_bytes: 100,
                    populated_bytes: Some(100),
                }),
                raw::Vmo::Data(raw::VmoData {
                    koid: 3,
                    name: 2,
                    parent_koid: 0,
                    committed_bytes: 100,
                    allocated_bytes: 200,
                    populated_bytes: Some(200),
                }),
                raw::Vmo::Data(raw::VmoData {
                    koid: 4,
                    name: 3,
                    parent_koid: 0,
                    committed_bytes: 100,
                    allocated_bytes: 200,
                    populated_bytes: Some(200),
                }),
            ],
        };
        let expected_processes = vec![
            processed::Process {
                koid: processed::ProcessKoid::new(2),
                name: "process2".to_string(),
                memory: processed::RetainedMemory {
                    private: 0,
                    scaled: 100,
                    total: 300,
                    private_populated: 0,
                    scaled_populated: 100,
                    total_populated: 300,
                    vmos: vec![],
                },
                name_to_vmo_memory: {
                    let mut result = HashMap::new();
                    result.insert(
                        "vmo1".to_string(),
                        processed::RetainedMemory {
                            private: 0,
                            scaled: 100,
                            total: 300,
                            private_populated: 0,
                            scaled_populated: 100,
                            total_populated: 300,
                            vmos: vec![processed::VmoKoid::new(1)],
                        },
                    );
                    result
                },
                vmos: {
                    let mut result = HashSet::new();
                    result.insert(processed::VmoKoid::new(1));
                    result
                },
            },
            processed::Process {
                koid: processed::ProcessKoid::new(3),
                name: "process3".to_string(),
                memory: processed::RetainedMemory {
                    private: 0,
                    scaled: 150,
                    total: 400,
                    private_populated: 0,
                    scaled_populated: 150,
                    total_populated: 400,
                    vmos: vec![],
                },
                name_to_vmo_memory: {
                    let mut result = HashMap::new();
                    result.insert(
                        "vmo1".to_string(),
                        processed::RetainedMemory {
                            private: 0,
                            scaled: 100,
                            total: 300,
                            private_populated: 0,
                            scaled_populated: 100,
                            total_populated: 300,
                            vmos: vec![processed::VmoKoid::new(1)],
                        },
                    );
                    result.insert(
                        "vmo2".to_string(),
                        processed::RetainedMemory {
                            private: 0,
                            scaled: 50,
                            total: 100,
                            private_populated: 0,
                            scaled_populated: 50,
                            total_populated: 100,
                            vmos: vec![processed::VmoKoid::new(2)],
                        },
                    );
                    result
                },
                vmos: {
                    let mut result = HashSet::new();
                    result.insert(processed::VmoKoid::new(1));
                    result.insert(processed::VmoKoid::new(2));
                    result
                },
            },
            processed::Process {
                koid: processed::ProcessKoid::new(4),
                name: "process4".to_string(),
                memory: processed::RetainedMemory {
                    private: 100,
                    scaled: 100,
                    total: 100,
                    private_populated: 200,
                    scaled_populated: 200,
                    total_populated: 200,
                    vmos: vec![],
                },
                name_to_vmo_memory: {
                    let mut result = HashMap::new();
                    result.insert(
                        "vmo3".to_string(),
                        processed::RetainedMemory {
                            private: 100,
                            scaled: 100,
                            total: 100,
                            private_populated: 200,
                            scaled_populated: 200,
                            total_populated: 200,
                            vmos: vec![processed::VmoKoid::new(3)],
                        },
                    );
                    result
                },
                vmos: {
                    let mut result = HashSet::new();
                    result.insert(processed::VmoKoid::new(3));
                    result
                },
            },
            processed::Process {
                koid: processed::ProcessKoid::new(5),
                name: "process5".to_string(),
                memory: processed::RetainedMemory {
                    private: 0,
                    scaled: 150,
                    total: 400,
                    private_populated: 0,
                    scaled_populated: 150,
                    total_populated: 400,
                    vmos: vec![],
                },
                name_to_vmo_memory: {
                    let mut result = HashMap::new();
                    result.insert(
                        "vmo2".to_string(),
                        processed::RetainedMemory {
                            private: 0,
                            scaled: 50,
                            total: 100,
                            private_populated: 0,
                            scaled_populated: 50,
                            total_populated: 100,
                            vmos: vec![processed::VmoKoid::new(2)],
                        },
                    );
                    result.insert(
                        "vmo1".to_string(),
                        processed::RetainedMemory {
                            private: 0,
                            scaled: 100,
                            total: 300,
                            private_populated: 0,
                            scaled_populated: 100,
                            total_populated: 300,
                            vmos: vec![processed::VmoKoid::new(1)],
                        },
                    );
                    result
                },
                vmos: {
                    let mut result = HashSet::new();
                    result.insert(processed::VmoKoid::new(2));
                    result
                },
            },
            processed::Process {
                koid: processed::ProcessKoid::new(6),
                name: "process6".to_string(),
                memory: processed::RetainedMemory {
                    private: 100,
                    scaled: 100,
                    total: 100,
                    private_populated: 200,
                    scaled_populated: 200,
                    total_populated: 200,
                    vmos: vec![],
                },
                name_to_vmo_memory: {
                    let mut result = HashMap::new();
                    result.insert(
                        "vmo4".to_string(),
                        processed::RetainedMemory {
                            private: 100,
                            scaled: 100,
                            total: 100,
                            private_populated: 200,
                            scaled_populated: 200,
                            total_populated: 200,
                            vmos: vec![processed::VmoKoid::new(4)],
                        },
                    );
                    result
                },
                vmos: {
                    let mut result = HashSet::new();
                    result.insert(processed::VmoKoid::new(4));
                    result
                },
            },
        ];

        let buckets_definitions = vec![
            BucketDefinition {
                event_code: 1000,
                name: "bucket0".to_string(),
                process: "process2|process4".to_string(),
                vmo: "".to_string(),
            },
            BucketDefinition {
                event_code: 1001,
                name: "bucket1".to_string(),
                process: "process3|process5".to_string(),
                vmo: "".to_string(),
            },
        ];

        let processed = processed::digest_from_memory_monitor_output(
            raw::MemoryMonitorOutput {
                capture: capture.clone(),
                buckets_definitions: buckets_definitions.clone(),
            },
            true,
            false,
        );
        // Check that the process list is sorted
        {
            for [p1, p2] in processed.processes.array_windows() {
                assert!(
                    p1.memory.private >= p2.memory.private,
                    "Processes are not presented in sorted order: {:?} < {:?}",
                    p1.memory,
                    p2.memory
                );
            }
        }

        pretty_assertions::assert_eq!(&processed.kmem_stats_compression, &kmem_stats_compression);

        pretty_assertions::assert_eq!(
            sorted_by_koid(&processed.processes),
            sorted_by_koid(&expected_processes)
        );

        let expected_vmos = vec![
            processed::Vmo {
                koid: processed::VmoKoid::new(1),
                name: "vmo1".to_string(),
                parent_koid: processed::VmoKoid::new(0),
                committed_bytes: 300,
                allocated_bytes: 300,
                populated_bytes: Some(300),
            },
            processed::Vmo {
                koid: processed::VmoKoid::new(2),
                name: "vmo2".to_string(),
                parent_koid: processed::VmoKoid::new(1),
                committed_bytes: 100,
                allocated_bytes: 100,
                populated_bytes: Some(100),
            },
            processed::Vmo {
                koid: processed::VmoKoid::new(3),
                name: "vmo3".to_string(),
                parent_koid: processed::VmoKoid::new(0),
                committed_bytes: 100,
                allocated_bytes: 200,
                populated_bytes: Some(200),
            },
            processed::Vmo {
                koid: processed::VmoKoid::new(4),
                name: "vmo4".to_string(),
                parent_koid: processed::VmoKoid::new(0),
                committed_bytes: 100,
                allocated_bytes: 200,
                populated_bytes: Some(200),
            },
        ];

        pretty_assertions::assert_eq!(
            sorted_vmos_by_koid(&expected_vmos),
            sorted_vmos_by_koid(&processed.vmos)
        );

        // Check that the buckets have been correctly created.
        // note: `bucket.rs` contain more in-depth bucketing tests.
        let buckets = processed.buckets;
        pretty_assertions::assert_eq!(
            buckets,
            Some(vec![
                Bucket {
                    name: "bucket0".to_string(),
                    size: 400,
                    vmos: HashSet::from([processed::VmoKoid::new(1), processed::VmoKoid::new(3)])
                },
                Bucket {
                    name: "bucket1".to_string(),
                    size: 100,
                    vmos: HashSet::from([processed::VmoKoid::new(2)])
                }
            ])
        );

        // VMO 4 from process 6 is the only VMO not part of any bucket.
        pretty_assertions::assert_eq!(processed.total_undigested, Some(100));

        // When `undigest_view` is active, only undigested VMOs are returned.
        let undigested_processed = processed::digest_from_memory_monitor_output(
            raw::MemoryMonitorOutput { capture, buckets_definitions },
            true,
            true,
        );

        pretty_assertions::assert_eq!(
            sorted_by_koid(&undigested_processed.processes),
            sorted_by_koid(&vec![expected_processes[4].clone()])
        );
    }

    // Reproduce a case similar to how blobfs shares the VMOs containing the file content.
    // `blobfs.cm` shares an unmodified child VMO with `app.cm`.
    // The children VMO has 0 committed pages.
    // The test verifies that the shared memory charged to `app.cm` is 0 despite the fact
    // that it owns a VMO that has a parent with committed memory.
    #[test]
    fn code_pages_received_from_blobfs_test() {
        let capture = raw::Capture {
            time: 1234567,
            kernel: raw::Kernel::default(),
            processes: vec![
                raw::Process::Headers(raw::ProcessHeaders::default()),
                raw::Process::Data(raw::ProcessData {
                    koid: 2,
                    name: "blobfs.cm".to_string(),
                    vmos: vec![1],
                }),
                raw::Process::Data(raw::ProcessData {
                    koid: 3,
                    name: "app.cm".to_string(),
                    vmos: vec![2],
                }),
            ],
            vmo_names: vec!["blob-xxx".to_string(), "app.cm".to_string()],
            vmos: vec![
                raw::Vmo::Headers(raw::VmoHeaders::default()),
                raw::Vmo::Data(raw::VmoData {
                    koid: 1,
                    name: 0,
                    parent_koid: 0,
                    committed_bytes: 500,
                    allocated_bytes: 1000,
                    populated_bytes: Some(500),
                }),
                raw::Vmo::Data(raw::VmoData {
                    koid: 2,
                    name: 1,
                    parent_koid: 1,
                    committed_bytes: 0,
                    allocated_bytes: 1000,
                    populated_bytes: Some(50),
                }),
            ],
            kmem_stats_compression: Default::default(),
        };
        let expected_processes = vec![
            processed::Process {
                koid: processed::ProcessKoid::new(2),
                name: "blobfs.cm".to_string(),
                memory: processed::RetainedMemory {
                    private: 0,
                    scaled: 250,
                    total: 500,
                    private_populated: 0,
                    scaled_populated: 250,
                    total_populated: 500,
                    vmos: vec![],
                },
                name_to_vmo_memory: {
                    let mut result = HashMap::new();
                    result.insert(
                        "blob-xxx".to_string(),
                        processed::RetainedMemory {
                            private: 0,
                            scaled: 250,
                            total: 500,
                            private_populated: 0,
                            scaled_populated: 250,
                            total_populated: 500,
                            vmos: vec![processed::VmoKoid::new(1)],
                        },
                    );
                    result
                },
                vmos: {
                    let mut result = HashSet::new();
                    result.insert(processed::VmoKoid::new(1));
                    result
                },
            },
            processed::Process {
                koid: processed::ProcessKoid::new(3),
                name: "app.cm".to_string(),
                memory: processed::RetainedMemory {
                    private: 0,
                    scaled: 250,
                    total: 500,
                    private_populated: 50,
                    scaled_populated: 300,
                    total_populated: 550,
                    vmos: vec![],
                },
                name_to_vmo_memory: {
                    let mut result = HashMap::new();
                    result.insert(
                        "app.cm".to_string(),
                        processed::RetainedMemory {
                            private: 0,
                            scaled: 0,
                            total: 0,
                            private_populated: 50,
                            scaled_populated: 50,
                            total_populated: 50,
                            vmos: vec![processed::VmoKoid::new(2)],
                        },
                    );
                    result.insert(
                        "blob-xxx".to_string(),
                        processed::RetainedMemory {
                            private: 0,
                            scaled: 250,
                            total: 500,
                            private_populated: 0,
                            scaled_populated: 250,
                            total_populated: 500,
                            vmos: vec![processed::VmoKoid::new(1)],
                        },
                    );
                    result
                },
                vmos: {
                    let mut result = HashSet::new();
                    result.insert(processed::VmoKoid::new(2));
                    result
                },
            },
        ];
        let processed = processed::digest_from_memory_monitor_output(
            raw::MemoryMonitorOutput { capture, buckets_definitions: vec![] },
            false,
            false,
        );

        // Check that the process list is sorted
        {
            for [p1, p2] in processed.processes.array_windows() {
                assert!(
                    p1.memory.private >= p2.memory.private,
                    "Processes are not presented in sorted order: {:?} < {:?}",
                    p1.memory,
                    p2.memory
                );
            }
        }

        pretty_assertions::assert_eq!(
            sorted_by_koid(&processed.processes),
            sorted_by_koid(&expected_processes)
        );

        let expected_vmos = vec![
            processed::Vmo {
                koid: processed::VmoKoid::new(1),
                name: "blob-xxx".to_string(),
                parent_koid: processed::VmoKoid::new(0),
                committed_bytes: 500,
                allocated_bytes: 1000,
                populated_bytes: Some(500),
            },
            processed::Vmo {
                koid: processed::VmoKoid::new(2),
                name: "app.cm".to_string(),
                parent_koid: processed::VmoKoid::new(1),
                committed_bytes: 0,
                allocated_bytes: 1000,
                populated_bytes: Some(50),
            },
        ];
        pretty_assertions::assert_eq!(
            sorted_vmos_by_koid(&expected_vmos),
            sorted_vmos_by_koid(&processed.vmos)
        );
    }
}
