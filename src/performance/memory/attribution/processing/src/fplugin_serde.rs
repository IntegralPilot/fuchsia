// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fkernel_serde;
use fidl_fuchsia_kernel_common as fkernel;
use fidl_fuchsia_memory_attribution_plugin_common as fplugin;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(remote = "fplugin::PerformanceImpactMetrics")]
pub struct PerformanceImpactMetricsDef {
    pub some_memory_stalls_ns: Option<i64>,
    pub full_memory_stalls_ns: Option<i64>,
    pub page_refaults: Option<u64>,
    #[doc(hidden)]
    #[serde(skip)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

// TODO(https://github.com/serde-rs/serde/issues/723): Use remote serialization
// with fkernel::KernelStatistics when supported inside options.
#[derive(Default, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct KernelStatistics {
    #[serde(with = "fkernel_serde::MemoryStatsDef")]
    pub memory_statistics: fkernel::MemoryStats,
    #[serde(with = "fkernel_serde::MemoryStatsCompressionDef")]
    pub compression_statistics: fkernel::MemoryStatsCompression,
}

impl From<fplugin::KernelStatistics> for KernelStatistics {
    fn from(value: fplugin::KernelStatistics) -> KernelStatistics {
        KernelStatistics {
            memory_statistics: value.memory_stats.unwrap(),
            compression_statistics: value.compression_stats.unwrap(),
        }
    }
}

impl Into<fplugin::KernelStatistics> for KernelStatistics {
    fn into(self) -> fplugin::KernelStatistics {
        fplugin::KernelStatistics {
            memory_stats: Some(self.memory_statistics),
            compression_stats: Some(self.compression_statistics),
            ..Default::default()
        }
    }
}

// TODO(https://github.com/serde-rs/serde/issues/723): Use remote serialization
// with fkernel::KernelStatistics when supported inside options.
#[derive(PartialEq, Debug, Clone, Serialize)]
#[serde(remote = "fplugin::ResourceType")]
pub enum ResourceTypeDef {
    #[serde(with = "JobDef")]
    Job(fplugin::Job),
    #[serde(with = "ProcessDef")]
    Process(fplugin::Process),
    #[serde(with = "VmoDef")]
    Vmo(fplugin::Vmo),
    #[doc(hidden)]
    #[serde(skip)]
    __SourceBreaking { unknown_ordinal: u64 },
}

#[derive(PartialEq, Debug, Clone, Serialize)]
#[serde(remote = "fplugin::Job")]
pub struct JobDef {
    pub child_jobs: Option<Vec<u64>>,
    pub processes: Option<Vec<u64>>,
    #[serde(skip)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

#[derive(PartialEq, Debug, Clone, Serialize)]
#[serde(remote = "fplugin::Process")]
pub struct ProcessDef {
    pub vmos: Option<Vec<u64>>,
    #[serde(with = "option_vec_mapping_def")]
    pub mappings: Option<Vec<fplugin::Mapping>>,
    #[serde(skip)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

// As [Process::mappings] is an Option<Vec<fplugin::Mapping>> instead of a pure struct, we can't
// easily derive a serializer and need to provide a custom one.
mod option_vec_mapping_def {
    use super::{MappingDef, fplugin};
    use serde::ser::SerializeSeq;
    use serde::{Serialize, Serializer};

    pub fn serialize<S>(
        opt_vec: &Option<Vec<fplugin::Mapping>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Wrapper<'a>(#[serde(with = "MappingDef")] &'a fplugin::Mapping);

        match opt_vec {
            Some(vec) => {
                let mut seq = serializer.serialize_seq(Some(vec.len()))?;
                for element in vec {
                    seq.serialize_element(&Wrapper(element))?;
                }
                seq.end()
            }
            None => serializer.serialize_none(),
        }
    }
}

#[derive(PartialEq, Debug, Clone, Serialize)]
#[serde(remote = "fplugin::Mapping")]
pub struct MappingDef {
    pub vmo: Option<u64>,
    pub address_base: Option<u64>,
    pub size: Option<u64>,
    #[serde(skip)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

#[derive(PartialEq, Debug, Clone, Serialize)]
#[serde(remote = "fplugin::Vmo")]
pub struct VmoDef {
    pub parent: Option<u64>,
    pub private_committed_bytes: Option<u64>,
    pub private_populated_bytes: Option<u64>,
    pub scaled_committed_bytes: Option<u64>,
    pub scaled_populated_bytes: Option<u64>,
    pub total_committed_bytes: Option<u64>,
    pub total_populated_bytes: Option<u64>,
    pub flags: Option<u32>,
    #[serde(skip)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::fplugin;
    #[test]
    fn test_convert() {
        let fplugin_kernel_statistics = fplugin::KernelStatistics {
            memory_stats: Some(fkernel::MemoryStats {
                total_bytes: Some(1),
                free_bytes: Some(2),
                free_loaned_bytes: Some(3),
                wired_bytes: Some(4),
                total_heap_bytes: Some(5),
                free_heap_bytes: Some(6),
                vmo_bytes: Some(7),
                mmu_overhead_bytes: Some(8),
                ipc_bytes: Some(9),
                cache_bytes: Some(10),
                slab_bytes: Some(11),
                zram_bytes: Some(12),
                other_bytes: Some(13),
                vmo_reclaim_total_bytes: Some(14),
                vmo_reclaim_newest_bytes: Some(15),
                vmo_reclaim_oldest_bytes: Some(16),
                vmo_reclaim_disabled_bytes: Some(17),
                vmo_discardable_locked_bytes: Some(18),
                vmo_discardable_unlocked_bytes: Some(19),
                ..Default::default()
            }),
            compression_stats: Some(fkernel::MemoryStatsCompression {
                uncompressed_storage_bytes: Some(15),
                compressed_storage_bytes: Some(16),
                compressed_fragmentation_bytes: Some(17),
                compression_time: Some(18),
                decompression_time: Some(19),
                total_page_compression_attempts: Some(20),
                failed_page_compression_attempts: Some(21),
                total_page_decompressions: Some(22),
                compressed_page_evictions: Some(23),
                eager_page_compressions: Some(24),
                memory_pressure_page_compressions: Some(25),
                critical_memory_page_compressions: Some(26),
                pages_decompressed_unit_ns: Some(27),
                pages_decompressed_within_log_time: Some([0, 1, 2, 3, 4, 5, 6, 7]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let kernel_statistics: KernelStatistics = fplugin_kernel_statistics.clone().into();

        assert_eq!(kernel_statistics.memory_statistics.total_bytes, Some(1));
        assert_eq!(kernel_statistics.memory_statistics.free_bytes, Some(2));

        assert_eq!(kernel_statistics.compression_statistics.uncompressed_storage_bytes, Some(15));

        assert_eq!(fplugin_kernel_statistics, kernel_statistics.into());
    }
}
