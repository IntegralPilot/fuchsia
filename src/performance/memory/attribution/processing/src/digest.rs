// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{ProcessedAttributionData, ZXName};
use anyhow::Result;
use bstr::ByteSlice;
use fidl_fuchsia_kernel_common as fkernel;
use fidl_fuchsia_memory_attribution_plugin_common as fplugin;
use regex_lite::Regex;
use rustc_hash::FxHashMap;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize};
use smallvec::SmallVec;
use std::collections::hash_map::Entry::Occupied;
#[cfg(target_os = "fuchsia")]
use {crate::CATEGORY_MEMORY_CAPTURE, fuchsia_trace::duration};

const UNDIGESTED: &str = "Undigested";
const ORPHANED: &str = "Orphaned";
const KERNEL: &str = "Kernel";
const FREE: &str = "Free";
const PAGER_TOTAL: &str = "[Addl]PagerTotal";
const PAGER_NEWEST: &str = "[Addl]PagerNewest";
const PAGER_OLDEST: &str = "[Addl]PagerOldest";
const DISCARDABLE_LOCKED: &str = "[Addl]DiscardableLocked";
const DISCARDABLE_UNLOCKED: &str = "[Addl]DiscardableUnlocked";
const ZRAM_COMPRESSED_BYTES: &str = "[Addl]ZramCompressedBytes";
const POPULATED_ANONYMOUS_BYTES: &str = "[Addl]PopulatedAnonymousBytes";

/// Represents a specification for aggregating memory usage in meaningful groups.
///
/// `name` represents the meaningful name of the group; grouping is done based on process and VMO
/// names.
///
// Note: This needs to mirror `//src/lib/assembly/memory_buckets/src/memory_buckets.rs`, but cannot
// reuse it directly because it is an host-only library.
#[derive(Clone, Debug, Deserialize)]
pub struct BucketDefinition {
    pub name: String,
    #[serde(deserialize_with = "deserialize_regex")]
    pub process: Option<Regex>,
    #[serde(deserialize_with = "deserialize_regex")]
    pub vmo: Option<Regex>,
    #[serde(default, deserialize_with = "deserialize_regex")]
    pub principal: Option<Regex>,
    pub event_code: u64,
}

impl BucketDefinition {
    /// Tests whether a process matches this bucket's definition, based on its name.
    fn process_match(&self, process: &ZXName) -> bool {
        self.process.as_ref().is_none_or(|process_regex| {
            process
                .as_bstr()
                .to_str()
                .is_ok_and(|process_name| process_regex.is_match(process_name))
        })
    }

    /// Tests whether a VMO matches this bucket's definition, based on its name.
    fn vmo_match(&self, vmo: &ZXName) -> bool {
        self.vmo.as_ref().is_none_or(|vmo_regex| {
            vmo.as_bstr().to_str().is_ok_and(|vmo_name| vmo_regex.is_match(vmo_name))
        })
    }

    /// Tests whether any of the specified principal names match this bucket's definition.
    fn principals_match(&self, principals: &[&str]) -> bool {
        self.principal.as_ref().is_none_or(|a| principals.iter().any(|name| a.is_match(name)))
    }
}

// Teach serde to deserialize an optional regex.
fn deserialize_regex<'de, D>(d: D) -> Result<Option<Regex>, D::Error>
where
    D: Deserializer<'de>,
{
    // Deserialize as Option<&str>
    Option::<String>::deserialize(d)
        // If the parsing failed, return the error, otherwise transform the value
        .and_then(|os| {
            os
                // If there is a value, try to parse it as a Regex.
                .map(|s| {
                    Regex::new(&s)
                        // If the regex compilation failed, wrap the error in the error type expected
                        // by serde.
                        .map_err(D::Error::custom)
                })
                // If there was a value but it failed to compile, return an error, otherwise return
                // the potentially parsed option.
                .transpose()
        })
}

/// Aggregates bytes in categories with human readable names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Bucket {
    pub name: String,
    pub populated_size: u64,
    pub committed_size: u64,
    pub vmos: Option<Vec<NamedVmo>>,
}

/// Contains a view of the system's memory usage, aggregated in groups called buckets, which are
/// configurable.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Digest {
    pub buckets: Vec<Bucket>,
}

/// Non-owning structure to keep track of known undigested VMOs.
struct UndigestedVmo<'a> {
    populated_size: u64,
    committed_size: u64,
    name: &'a ZXName,
    principals: &'a [&'a str],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
/// Owning structure to report known VMOs.
pub struct NamedVmo {
    pub name: ZXName,
    pub populated_size: u64,
    pub committed_size: u64,
    pub principals: Vec<String>,
}

impl Digest {
    /// Given means to query the system for memory usage, and a specification, this function
    /// aggregates the current memory usage into human displayable units we call buckets.
    pub fn compute(
        attribution_data: &ProcessedAttributionData,
        kmem_stats: &fkernel::MemoryStats,
        kmem_stats_compression: &fkernel::MemoryStatsCompression,
        bucket_definitions: &[BucketDefinition],
        detailed_vmos: bool,
    ) -> Result<Digest> {
        #[cfg(target_os = "fuchsia")]
        duration!(CATEGORY_MEMORY_CAPTURE, c"Digest::compute");

        // Maps resources' (VMO, Process, Job. See Resource) ids
        // to their owner, i.e. the principal they have been
        // attributed to.
        //
        // On a test run (2026-08), we found that the number of VMOs for each number of owners
        // distributes as follows:
        // # of owners : # of VMOs
        // - 01 : 62072
        // - 02 :  3112
        // - 03 :  0096
        // - 04 :  0011
        // - 05 :  0002
        // - 06 :  0001
        // - .... (each subsequent entry has 1 VMO at most)
        let owners: FxHashMap<u64, SmallVec<[&str; 1]>> = {
            // `owners` are only needed when detailed_vmos is true, or when one bucket definition
            // uses a principal matcher, which is the case on all the products where memory monitor
            // 2 is deployed. Therefore we always compute it.
            let mut owners: FxHashMap<u64, SmallVec<[&str; 1]>> = FxHashMap::default();
            for (_, p) in &attribution_data.principals {
                let p_name = p.name();
                for r in &p.resources {
                    owners.entry(*r).or_default().push(p_name);
                }
            }
            owners
        };

        let mut populated_reclaimable_bytes = 0;
        let mut undigested_vmos: FxHashMap<u64, UndigestedVmo<'_>> = attribution_data
            .resources
            .iter()
            .filter_map(|(koid, r)| match &r.resource.resource_type {
                fplugin::ResourceType::Vmo(vmo) => {
                    attribution_data.resource_names.get(r.resource.name_index).and_then(|name| {
                        let populated_size = vmo.scaled_populated_bytes?;
                        let committed_size = vmo.scaled_committed_bytes?;
                        if vmo.flags.map_or(false, |flags| {
                            flags
                                & (zx_types::ZX_INFO_VMO_PAGER_BACKED
                                    | zx_types::ZX_INFO_VMO_DISCARDABLE)
                                != 0
                        }) {
                            populated_reclaimable_bytes += populated_size;
                        }
                        Some((
                            *koid,
                            UndigestedVmo {
                                name,
                                populated_size,
                                committed_size,
                                principals: owners.get(koid).map_or(&[], |v| v.as_slice()),
                            },
                        ))
                    })
                }
                _ => None,
            })
            .collect();
        let processes: Vec<(&ZXName, &fplugin::Process)> = attribution_data
            .resources
            .values()
            .filter_map(|r| match &r.resource.resource_type {
                fplugin::ResourceType::Process(process) => attribution_data
                    .resource_names
                    .get(r.resource.name_index)
                    .map(|name| (name, process)),
                _ => None,
            })
            .collect();

        let mut buckets: Vec<Bucket> = bucket_definitions
            .iter()
            .map(|bd| {
                let mut bucket = Bucket {
                    name: bd.name.to_owned(),
                    populated_size: 0,
                    committed_size: 0,
                    vmos: None,
                };
                processes.iter().for_each(|(process_name, process)| {
                    if bd.process_match(process_name) {
                        for koid in process.vmos.iter().flatten() {
                            let (populated_size, committed_size) = match undigested_vmos
                                .entry(*koid)
                            {
                                Occupied(e) => {
                                    let UndigestedVmo { name, principals, .. } = e.get();
                                    if bd.vmo_match(&name) && bd.principals_match(principals) {
                                        let (_, vmo) = e.remove_entry();
                                        if detailed_vmos {
                                            bucket.vmos.get_or_insert_default().push(NamedVmo {
                                                name: vmo.name.clone(),
                                                populated_size: vmo.populated_size,
                                                committed_size: vmo.committed_size,
                                                principals: vmo
                                                    .principals
                                                    .iter()
                                                    .map(|&name| name.to_owned())
                                                    .collect(),
                                            });
                                        }
                                        (vmo.populated_size, vmo.committed_size)
                                    } else {
                                        (0, 0)
                                    }
                                }
                                _ => (0, 0),
                            };
                            bucket.committed_size += committed_size;
                            bucket.populated_size += populated_size;
                        }
                    };
                });
                bucket
            })
            .collect();

        // This bucket contains the total size of the known VMOs that have not been covered
        // by any other bucket.
        let undigested = {
            let (populated_size, committed_size) = undigested_vmos
                .values()
                .map(|UndigestedVmo { populated_size, committed_size, .. }| {
                    (*populated_size, *committed_size)
                })
                .fold((0, 0), |(total_populated, total_committed), (populated, committed)| {
                    (total_populated + populated, total_committed + committed)
                });

            Bucket {
                name: UNDIGESTED.to_string(),
                populated_size: populated_size,
                committed_size,
                vmos: if detailed_vmos {
                    Some(
                        undigested_vmos
                            .values()
                            .map(|vmo| NamedVmo {
                                name: vmo.name.clone(),
                                populated_size: vmo.populated_size,
                                committed_size: vmo.committed_size,
                                principals: vmo
                                    .principals
                                    .iter()
                                    .map(|&name| name.to_owned())
                                    .collect(),
                            })
                            .collect(),
                    )
                } else {
                    None
                },
            }
        };

        let total_vmo_size: u64 = undigested.committed_size
            + buckets.iter().map(|Bucket { committed_size, .. }| committed_size).sum::<u64>();

        // Extend the configured aggregation with a number of additional, occasionally useful meta
        // aggregations.
        buckets.extend([
            undigested,
            // This bucket accounts for VMO bytes that have been allocated by the kernel, but not
            // claimed by any VMO (anymore).
            {
                let size = kmem_stats.vmo_bytes.unwrap_or(0).saturating_sub(total_vmo_size);
                Bucket {
                    name: ORPHANED.to_string(),
                    populated_size: size,
                    committed_size: size,
                    vmos: None,
                }
            },
            // This bucket aggregates overall kernel memory usage.
            {
                let size = (|| {
                    Some(
                        kmem_stats.wired_bytes?
                            + kmem_stats.total_heap_bytes?
                            + kmem_stats.mmu_overhead_bytes?
                            + kmem_stats.ipc_bytes?
                            + kmem_stats.other_bytes?
                            + kmem_stats.slab_bytes?
                            + kmem_stats.cache_bytes?,
                    )
                })()
                .unwrap_or(0);
                Bucket {
                    name: KERNEL.to_string(),
                    populated_size: size,
                    committed_size: size,
                    vmos: None,
                }
            },
            // This bucket contains the amount of free memory in the system.
            {
                let size = kmem_stats.free_bytes.unwrap_or(0);
                Bucket {
                    name: FREE.to_string(),
                    populated_size: size,
                    committed_size: size,
                    vmos: None,
                }
            },
            // Those buckets contain pager related information.
            {
                let size = kmem_stats.vmo_reclaim_total_bytes.unwrap_or(0);
                Bucket {
                    name: PAGER_TOTAL.to_string(),
                    populated_size: size,
                    committed_size: size,
                    vmos: None,
                }
            },
            {
                let size = kmem_stats.vmo_reclaim_newest_bytes.unwrap_or(0);
                Bucket {
                    name: PAGER_NEWEST.to_string(),
                    populated_size: size,
                    committed_size: size,
                    vmos: None,
                }
            },
            {
                let size = kmem_stats.vmo_reclaim_oldest_bytes.unwrap_or(0);
                Bucket {
                    name: PAGER_OLDEST.to_string(),
                    populated_size: size,
                    committed_size: size,
                    vmos: None,
                }
            },
            // Those buckets account for discardable memory.
            {
                let size = kmem_stats.vmo_discardable_locked_bytes.unwrap_or(0);
                Bucket {
                    name: DISCARDABLE_LOCKED.to_string(),
                    populated_size: size,
                    committed_size: size,
                    vmos: None,
                }
            },
            {
                let size = kmem_stats.vmo_discardable_unlocked_bytes.unwrap_or(0);
                Bucket {
                    name: DISCARDABLE_UNLOCKED.to_string(),
                    populated_size: size,
                    committed_size: size,
                    vmos: None,
                }
            },
            // This bucket accounts for compressed memory.
            {
                let size = kmem_stats_compression.compressed_storage_bytes.unwrap_or(0);
                Bucket {
                    name: ZRAM_COMPRESSED_BYTES.to_string(),
                    populated_size: size,
                    committed_size: size,
                    vmos: None,
                }
            },
            // This bucket accounts for all populated anonymous memory (non-reclaimable).
            {
                let size = (kmem_stats.total_bytes.unwrap_or(0)
                    + kmem_stats_compression.uncompressed_storage_bytes.unwrap_or(0))
                .saturating_sub(kmem_stats.free_bytes.unwrap_or(0))
                .saturating_sub(kmem_stats.zram_bytes.unwrap_or(0))
                .saturating_sub(populated_reclaimable_bytes);

                Bucket {
                    name: POPULATED_ANONYMOUS_BYTES.to_string(),
                    populated_size: size,
                    committed_size: size,
                    vmos: None,
                }
            },
        ]);
        Ok(Digest { buckets })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Attribution, AttributionData, GlobalPrincipalIdentifier, Principal, PrincipalDescription,
        PrincipalType, ProcessedAttributionData, Resource, ResourceReference, attribute_vmos,
    };
    use fidl_fuchsia_memory_attribution_plugin_common as fplugin;
    use regex_lite::Regex;

    fn get_attribution_data() -> ProcessedAttributionData {
        attribute_vmos(AttributionData {
            principals_vec: vec![
                Principal {
                    identifier: GlobalPrincipalIdentifier::new_for_test(1),
                    description: Some(PrincipalDescription::Component("principal".to_owned())),
                    principal_type: PrincipalType::Runnable,
                    parent: Some(GlobalPrincipalIdentifier::new_for_test(2)),
                },
                Principal {
                    identifier: GlobalPrincipalIdentifier::new_for_test(2),
                    description: Some(PrincipalDescription::Component("parent".to_owned())),
                    principal_type: PrincipalType::Runnable,
                    parent: None,
                },
            ],
            resources_vec: vec![
                Resource {
                    koid: 10,
                    name_index: 0,
                    resource_type: fplugin::ResourceType::Vmo(fplugin::Vmo {
                        parent: None,
                        private_committed_bytes: Some(1024),
                        private_populated_bytes: Some(2048),
                        scaled_committed_bytes: Some(512),
                        scaled_populated_bytes: Some(2048),
                        total_committed_bytes: Some(1024),
                        total_populated_bytes: Some(2048),
                        ..Default::default()
                    }),
                },
                Resource {
                    koid: 20,
                    name_index: 1,
                    resource_type: fplugin::ResourceType::Vmo(fplugin::Vmo {
                        parent: None,
                        private_committed_bytes: Some(1024),
                        private_populated_bytes: Some(2048),
                        scaled_committed_bytes: Some(512),
                        scaled_populated_bytes: Some(2048),
                        total_committed_bytes: Some(1024),
                        total_populated_bytes: Some(2048),
                        ..Default::default()
                    }),
                },
                Resource {
                    koid: 30,
                    name_index: 1,
                    resource_type: fplugin::ResourceType::Process(fplugin::Process {
                        vmos: Some(vec![10, 20]),
                        ..Default::default()
                    }),
                },
            ],
            resource_names: vec![
                ZXName::try_from_bytes(b"resource").unwrap(),
                ZXName::try_from_bytes(b"matched").unwrap(),
            ],
            attributions: vec![Attribution {
                source: GlobalPrincipalIdentifier::new_for_test(1),
                subject: GlobalPrincipalIdentifier::new_for_test(1),
                resources: vec![ResourceReference::KernelObject(20)],
            }],
        })
    }

    fn get_kernel_stats() -> (fkernel::MemoryStats, fkernel::MemoryStatsCompression) {
        (
            fkernel::MemoryStats {
                total_bytes: Some(20),
                free_bytes: Some(2),
                wired_bytes: Some(3),
                total_heap_bytes: Some(4),
                free_heap_bytes: Some(5),
                vmo_bytes: Some(10000),
                mmu_overhead_bytes: Some(7),
                ipc_bytes: Some(8),
                other_bytes: Some(9),
                free_loaned_bytes: Some(10),
                cache_bytes: Some(11),
                slab_bytes: Some(12),
                zram_bytes: Some(13),
                vmo_reclaim_total_bytes: Some(14),
                vmo_reclaim_newest_bytes: Some(15),
                vmo_reclaim_oldest_bytes: Some(16),
                vmo_reclaim_disabled_bytes: Some(17),
                vmo_discardable_locked_bytes: Some(18),
                vmo_discardable_unlocked_bytes: Some(19),
                ..Default::default()
            },
            fkernel::MemoryStatsCompression {
                uncompressed_storage_bytes: Some(1),
                compressed_storage_bytes: Some(21),
                compressed_fragmentation_bytes: Some(22),
                compression_time: Some(23),
                decompression_time: Some(24),
                total_page_compression_attempts: Some(25),
                failed_page_compression_attempts: Some(26),
                total_page_decompressions: Some(27),
                compressed_page_evictions: Some(28),
                eager_page_compressions: Some(29),
                memory_pressure_page_compressions: Some(30),
                critical_memory_page_compressions: Some(31),
                pages_decompressed_unit_ns: Some(32),
                pages_decompressed_within_log_time: Some([40, 41, 42, 43, 44, 45, 46, 47]),
                ..Default::default()
            },
        )
    }

    fn sort_buckets_for_assert(digest: &mut Digest) {
        for bucket in digest.buckets.iter_mut() {
            for vmos in bucket.vmos.iter_mut() {
                vmos.sort_by(|vmo1, vmo2| vmo1.name.cmp(&vmo2.name));
            }
        }
    }

    #[test]
    fn test_digest_no_definitions() {
        let (kernel_stats, kernel_stats_compression) = get_kernel_stats();
        let digest = {
            let mut digest = Digest::compute(
                &get_attribution_data(),
                &kernel_stats,
                &kernel_stats_compression,
                &vec![],
                true,
            )
            .unwrap();
            sort_buckets_for_assert(&mut digest);
            digest
        };
        let expected_buckets = vec![
            // The two VMOs are unmatched, 512 + 512
            Bucket {
                name: UNDIGESTED.to_string(),
                populated_size: 4096,
                committed_size: 1024,
                vmos: Some(vec![
                    NamedVmo {
                        name: ZXName::from_string_lossy("matched"),
                        populated_size: 2048,
                        committed_size: 512,
                        principals: vec!["principal".to_string()],
                    },
                    NamedVmo {
                        name: ZXName::from_string_lossy("resource"),
                        populated_size: 2048,
                        committed_size: 512,
                        principals: vec![],
                    },
                ]),
            },
            // No matched VMOs, one UNDIGESTED VMO => 10000 - 1024 = 8976
            Bucket {
                name: ORPHANED.to_string(),
                populated_size: 8976,
                committed_size: 8976,
                vmos: None,
            },
            // wired + heap + mmu + ipc + other + slab + cache => 3 + 4 + 7 + 8 + 9 + 12 + 11 = 54
            Bucket { name: KERNEL.to_string(), populated_size: 54, committed_size: 54, vmos: None },
            Bucket { name: FREE.to_string(), populated_size: 2, committed_size: 2, vmos: None },
            Bucket {
                name: PAGER_TOTAL.to_string(),
                populated_size: 14,
                committed_size: 14,
                vmos: None,
            },
            Bucket {
                name: PAGER_NEWEST.to_string(),
                populated_size: 15,
                committed_size: 15,
                vmos: None,
            },
            Bucket {
                name: PAGER_OLDEST.to_string(),
                populated_size: 16,
                committed_size: 16,
                vmos: None,
            },
            Bucket {
                name: DISCARDABLE_LOCKED.to_string(),
                populated_size: 18,
                committed_size: 18,
                vmos: None,
            },
            Bucket {
                name: DISCARDABLE_UNLOCKED.to_string(),
                populated_size: 19,
                committed_size: 19,
                vmos: None,
            },
            Bucket {
                name: ZRAM_COMPRESSED_BYTES.to_string(),
                populated_size: 21,
                committed_size: 21,
                vmos: None,
            },
            Bucket {
                name: POPULATED_ANONYMOUS_BYTES.to_string(),
                populated_size: 6,
                committed_size: 6,
                vmos: None,
            },
        ];

        assert_eq!(digest.buckets, expected_buckets);
    }

    #[test]
    fn test_digest_with_matching_vmo() -> Result<(), anyhow::Error> {
        let (kernel_stats, kernel_stats_compression) = get_kernel_stats();
        let digest = {
            let mut digest = Digest::compute(
                &get_attribution_data(),
                &kernel_stats,
                &kernel_stats_compression,
                &vec![BucketDefinition {
                    name: "matched".to_string(),
                    process: None,
                    vmo: Some(Regex::new("matched")?),
                    principal: None,
                    event_code: Default::default(),
                }],
                true,
            )
            .unwrap();
            sort_buckets_for_assert(&mut digest);
            digest
        };
        let expected_buckets = vec![
            // One VMO is matched, the other is not
            Bucket {
                name: "matched".to_string(),
                populated_size: 2048,
                committed_size: 512,
                vmos: Some(vec![NamedVmo {
                    name: ZXName::from_string_lossy("matched"),
                    populated_size: 2048,
                    committed_size: 512,
                    principals: vec!["principal".to_owned()],
                }]),
            },
            // One unmatched VMO
            Bucket {
                name: UNDIGESTED.to_string(),
                populated_size: 2048,
                committed_size: 512,
                vmos: Some(vec![NamedVmo {
                    name: ZXName::from_string_lossy("resource"),
                    populated_size: 2048,
                    committed_size: 512,
                    principals: vec![],
                }]),
            },
            // One matched VMO, one unmatched VMO //=> 10000 - 512 - 512 = 8976
            Bucket {
                name: ORPHANED.to_string(),
                populated_size: 8976,
                committed_size: 8976,
                vmos: None,
            },
            // wired + heap + mmu + ipc + other + slab + cache => 3 + 4 + 7 + 8 + 9 + 12 + 11 = 54
            Bucket { name: KERNEL.to_string(), populated_size: 54, committed_size: 54, vmos: None },
            Bucket { name: FREE.to_string(), populated_size: 2, committed_size: 2, vmos: None },
            Bucket {
                name: PAGER_TOTAL.to_string(),
                populated_size: 14,
                committed_size: 14,
                vmos: None,
            },
            Bucket {
                name: PAGER_NEWEST.to_string(),
                populated_size: 15,
                committed_size: 15,
                vmos: None,
            },
            Bucket {
                name: PAGER_OLDEST.to_string(),
                populated_size: 16,
                committed_size: 16,
                vmos: None,
            },
            Bucket {
                name: DISCARDABLE_LOCKED.to_string(),
                populated_size: 18,
                committed_size: 18,
                vmos: None,
            },
            Bucket {
                name: DISCARDABLE_UNLOCKED.to_string(),
                populated_size: 19,
                committed_size: 19,
                vmos: None,
            },
            Bucket {
                name: ZRAM_COMPRESSED_BYTES.to_string(),
                populated_size: 21,
                committed_size: 21,
                vmos: None,
            },
            Bucket {
                name: POPULATED_ANONYMOUS_BYTES.to_string(),
                populated_size: 6,
                committed_size: 6,
                vmos: None,
            },
        ];

        assert_eq!(digest.buckets, expected_buckets);
        Ok(())
    }

    #[test]
    fn test_digest_with_matching_process() -> Result<(), anyhow::Error> {
        let (kernel_stats, kernel_stats_compression) = get_kernel_stats();
        let digest = {
            let mut digest = Digest::compute(
                &get_attribution_data(),
                &kernel_stats,
                &kernel_stats_compression,
                &vec![BucketDefinition {
                    name: "matched".to_string(),
                    process: Some(Regex::new("matched")?),
                    vmo: None,
                    principal: None,
                    event_code: Default::default(),
                }],
                true,
            )
            .unwrap();
            sort_buckets_for_assert(&mut digest);
            digest
        };
        let expected_buckets = vec![
            // Both VMOs are matched => 512 + 512 = 1024
            Bucket {
                name: "matched".to_string(),
                populated_size: 4096,
                committed_size: 1024,
                vmos: Some(vec![
                    NamedVmo {
                        name: ZXName::from_string_lossy("matched"),
                        populated_size: 2048,
                        committed_size: 512,
                        principals: vec!["principal".to_owned()],
                    },
                    NamedVmo {
                        name: ZXName::from_string_lossy("resource"),
                        populated_size: 2048,
                        committed_size: 512,
                        principals: vec![],
                    },
                ]),
            },
            // No unmatched VMO
            Bucket {
                name: UNDIGESTED.to_string(),
                populated_size: 0,
                committed_size: 0,
                vmos: Some(vec![]),
            },
            // Two matched VMO => 10000 - 512 - 512 = 8976
            Bucket {
                name: ORPHANED.to_string(),
                populated_size: 8976,
                committed_size: 8976,
                vmos: None,
            },
            // wired + heap + mmu + ipc + other + slab + cache => 3 + 4 + 7 + 8 + 9 + 12 + 11 = 54
            Bucket { name: KERNEL.to_string(), populated_size: 54, committed_size: 54, vmos: None },
            Bucket { name: FREE.to_string(), populated_size: 2, committed_size: 2, vmos: None },
            Bucket {
                name: PAGER_TOTAL.to_string(),
                populated_size: 14,
                committed_size: 14,
                vmos: None,
            },
            Bucket {
                name: PAGER_NEWEST.to_string(),
                populated_size: 15,
                committed_size: 15,
                vmos: None,
            },
            Bucket {
                name: PAGER_OLDEST.to_string(),
                populated_size: 16,
                committed_size: 16,
                vmos: None,
            },
            Bucket {
                name: DISCARDABLE_LOCKED.to_string(),
                populated_size: 18,
                committed_size: 18,
                vmos: None,
            },
            Bucket {
                name: DISCARDABLE_UNLOCKED.to_string(),
                populated_size: 19,
                committed_size: 19,
                vmos: None,
            },
            Bucket {
                name: ZRAM_COMPRESSED_BYTES.to_string(),
                populated_size: 21,
                committed_size: 21,
                vmos: None,
            },
            Bucket {
                name: POPULATED_ANONYMOUS_BYTES.to_string(),
                populated_size: 6,
                committed_size: 6,
                vmos: None,
            },
        ];

        assert_eq!(digest.buckets, expected_buckets);
        Ok(())
    }

    #[test]
    fn test_digest_with_matching_principal() -> Result<(), anyhow::Error> {
        let (kernel_stats, kernel_stats_compression) = get_kernel_stats();
        let digest = {
            let mut digest = Digest::compute(
                &get_attribution_data(),
                &kernel_stats,
                &kernel_stats_compression,
                &vec![BucketDefinition {
                    name: "matched".to_string(),
                    process: None,
                    vmo: None,
                    principal: Some(Regex::new("principal")?),
                    event_code: Default::default(),
                }],
                true,
            )
            .unwrap();
            sort_buckets_for_assert(&mut digest);
            digest
        };
        let expected_buckets = vec![
            // One VMO is matched, the other is not
            Bucket {
                name: "matched".to_string(),
                populated_size: 2048,
                committed_size: 512,
                vmos: Some(vec![NamedVmo {
                    name: ZXName::from_string_lossy("matched"),
                    populated_size: 2048,
                    committed_size: 512,
                    principals: vec!["principal".to_owned()],
                }]),
            },
            // One unmatched VMO
            Bucket {
                name: UNDIGESTED.to_string(),
                populated_size: 2048,
                committed_size: 512,
                vmos: Some(vec![NamedVmo {
                    name: ZXName::from_string_lossy("resource"),
                    populated_size: 2048,
                    committed_size: 512,
                    principals: vec![],
                }]),
            },
            // One matched VMO, one unmatched VMO //=> 10000 - 512 - 512 = 8976
            Bucket {
                name: ORPHANED.to_string(),
                populated_size: 8976,
                committed_size: 8976,
                vmos: None,
            },
            // wired + heap + mmu + ipc + other + slab + cache => 3 + 4 + 7 + 8 + 9 + 12 + 11 = 54
            Bucket { name: KERNEL.to_string(), populated_size: 54, committed_size: 54, vmos: None },
            Bucket { name: FREE.to_string(), populated_size: 2, committed_size: 2, vmos: None },
            Bucket {
                name: PAGER_TOTAL.to_string(),
                populated_size: 14,
                committed_size: 14,
                vmos: None,
            },
            Bucket {
                name: PAGER_NEWEST.to_string(),
                populated_size: 15,
                committed_size: 15,
                vmos: None,
            },
            Bucket {
                name: PAGER_OLDEST.to_string(),
                populated_size: 16,
                committed_size: 16,
                vmos: None,
            },
            Bucket {
                name: DISCARDABLE_LOCKED.to_string(),
                populated_size: 18,
                committed_size: 18,
                vmos: None,
            },
            Bucket {
                name: DISCARDABLE_UNLOCKED.to_string(),
                populated_size: 19,
                committed_size: 19,
                vmos: None,
            },
            Bucket {
                name: ZRAM_COMPRESSED_BYTES.to_string(),
                populated_size: 21,
                committed_size: 21,
                vmos: None,
            },
            Bucket {
                name: POPULATED_ANONYMOUS_BYTES.to_string(),
                populated_size: 6,
                committed_size: 6,
                vmos: None,
            },
        ];

        assert_eq!(digest.buckets, expected_buckets);
        Ok(())
    }

    #[test]
    fn test_digest_with_matching_principal_process_and_vmo() -> Result<(), anyhow::Error> {
        let (kernel_stats, kernel_stats_compression) = get_kernel_stats();
        let digest = {
            let mut digest = Digest::compute(
                &get_attribution_data(),
                &kernel_stats,
                &kernel_stats_compression,
                &vec![BucketDefinition {
                    name: "matched".to_string(),
                    process: Some(Regex::new("matched")?),
                    vmo: Some(Regex::new("matched")?),
                    principal: Some(Regex::new("principal")?),
                    event_code: Default::default(),
                }],
                true,
            )
            .unwrap();
            sort_buckets_for_assert(&mut digest);
            digest
        };
        let expected_buckets = vec![
            // One VMO is matched, the other is not
            Bucket {
                name: "matched".to_string(),
                populated_size: 2048,
                committed_size: 512,
                vmos: Some(vec![NamedVmo {
                    name: ZXName::from_string_lossy("matched"),
                    populated_size: 2048,
                    committed_size: 512,
                    principals: vec!["principal".to_owned()],
                }]),
            },
            // One unmatched VMO
            Bucket {
                name: UNDIGESTED.to_string(),
                populated_size: 2048,
                committed_size: 512,
                vmos: Some(vec![NamedVmo {
                    name: ZXName::from_string_lossy("resource"),
                    populated_size: 2048,
                    committed_size: 512,
                    principals: vec![],
                }]),
            },
            // One matched VMO, one unmatched VMO => 10000 - 512 - 512 = 8976
            Bucket {
                name: ORPHANED.to_string(),
                populated_size: 8976,
                committed_size: 8976,
                vmos: None,
            },
            // wired + heap + mmu + ipc + other + slab + cache => 3 + 4 + 7 + 8 + 9 + 12 + 11 = 54
            Bucket { name: KERNEL.to_string(), populated_size: 54, committed_size: 54, vmos: None },
            Bucket { name: FREE.to_string(), populated_size: 2, committed_size: 2, vmos: None },
            Bucket {
                name: PAGER_TOTAL.to_string(),
                populated_size: 14,
                committed_size: 14,
                vmos: None,
            },
            Bucket {
                name: PAGER_NEWEST.to_string(),
                populated_size: 15,
                committed_size: 15,
                vmos: None,
            },
            Bucket {
                name: PAGER_OLDEST.to_string(),
                populated_size: 16,
                committed_size: 16,
                vmos: None,
            },
            Bucket {
                name: DISCARDABLE_LOCKED.to_string(),
                populated_size: 18,
                committed_size: 18,
                vmos: None,
            },
            Bucket {
                name: DISCARDABLE_UNLOCKED.to_string(),
                populated_size: 19,
                committed_size: 19,
                vmos: None,
            },
            Bucket {
                name: ZRAM_COMPRESSED_BYTES.to_string(),
                populated_size: 21,
                committed_size: 21,
                vmos: None,
            },
            Bucket {
                name: POPULATED_ANONYMOUS_BYTES.to_string(),
                populated_size: 6,
                committed_size: 6,
                vmos: None,
            },
        ];

        assert_eq!(digest.buckets, expected_buckets);
        Ok(())
    }

    /// What is tested: `Digest::compute` with `detailed_vmos: false` (the production periodic
    /// monitoring path) and skipping of VMO list population.
    ///
    /// Expectations verified:
    /// - When `detailed_vmos == false`, every bucket in `digest.buckets` (including matched buckets
    ///   and `UNDIGESTED`) has `bucket.vmos == None`.
    /// - Verifies that `committed_size` and `populated_size` totals match expected values
    ///   identically to when `detailed_vmos == true`.
    #[test]
    fn test_digest_compute_undetailed_vmos_fast_path() -> Result<(), anyhow::Error> {
        let (kernel_stats, kernel_stats_compression) = get_kernel_stats();
        let digest = Digest::compute(
            &get_attribution_data(),
            &kernel_stats,
            &kernel_stats_compression,
            &vec![BucketDefinition {
                name: "matched".to_string(),
                process: None,
                vmo: Some(Regex::new("matched")?),
                principal: None,
                event_code: Default::default(),
            }],
            false, // detailed_vmos = false
        )?;

        for bucket in &digest.buckets {
            assert!(bucket.vmos.is_none(), "Bucket '{}' should have vmos == None", bucket.name);
        }
        let matched_bucket = digest.buckets.iter().find(|b| b.name == "matched").unwrap();
        assert_eq!(matched_bucket.committed_size, 512);
        assert_eq!(matched_bucket.populated_size, 2048);
        let undigested_bucket = digest.buckets.iter().find(|b| b.name == UNDIGESTED).unwrap();
        assert_eq!(undigested_bucket.committed_size, 512);
        assert_eq!(undigested_bucket.populated_size, 2048);
        Ok(())
    }

    /// What is tested: First-match-wins bucket priority ordering and deduplication across multiple
    /// overlapping `BucketDefinition`s in `Digest::compute`.
    ///
    /// Expectations verified:
    /// - When two bucket definitions both match the same VMO (`first_bucket` matches
    ///   `vmo="matched"`, and `second_bucket` matches `vmo=".*"`), the earlier bucket claims the
    ///   VMO.
    /// - Verifies that `second_bucket` does not double-count VMO 20 (`committed_size == 0` for VMO
    ///   20), and only claims the remaining unconsumed VMO 10 (`resource`).
    #[test]
    fn test_digest_bucket_priority_and_deduplication() -> Result<(), anyhow::Error> {
        let (kernel_stats, kernel_stats_compression) = get_kernel_stats();
        let digest = Digest::compute(
            &get_attribution_data(),
            &kernel_stats,
            &kernel_stats_compression,
            &vec![
                BucketDefinition {
                    name: "first_bucket".to_string(),
                    process: None,
                    vmo: Some(Regex::new("matched")?),
                    principal: None,
                    event_code: Default::default(),
                },
                BucketDefinition {
                    name: "second_bucket".to_string(),
                    process: None,
                    vmo: Some(Regex::new(".*")?),
                    principal: None,
                    event_code: Default::default(),
                },
            ],
            true,
        )?;

        let b1 = digest.buckets.iter().find(|b| b.name == "first_bucket").unwrap();
        assert_eq!(b1.committed_size, 512); // VMO 20 ("matched")
        let b2 = digest.buckets.iter().find(|b| b.name == "second_bucket").unwrap();
        assert_eq!(b2.committed_size, 512); // VMO 10 ("resource")
        let undigested = digest.buckets.iter().find(|b| b.name == UNDIGESTED).unwrap();
        assert_eq!(undigested.committed_size, 0);
        Ok(())
    }

    /// What is tested: Multi-attribute `BucketDefinition` matching where all non-None attributes
    /// (`process`, `vmo`, `principal`) must match simultaneously, and partial mismatch fallthrough.
    ///
    /// Expectations verified:
    /// - A bucket with matching `process` ("matched") but non-matching `vmo` ("nonexistent_vmo")
    ///   fails to claim the VMO.
    /// - Verifies that partial mismatches leave the VMO unclaimed so it is assigned to
    ///   `UNDIGESTED`.
    #[test]
    fn test_digest_multi_attribute_matching_and_partial_mismatch() -> Result<(), anyhow::Error> {
        let (kernel_stats, kernel_stats_compression) = get_kernel_stats();
        let digest = Digest::compute(
            &get_attribution_data(),
            &kernel_stats,
            &kernel_stats_compression,
            &vec![BucketDefinition {
                name: "partial_mismatch".to_string(),
                process: Some(Regex::new("matched")?),
                vmo: Some(Regex::new("nonexistent_vmo")?),
                principal: Some(Regex::new("principal")?),
                event_code: Default::default(),
            }],
            true,
        )?;

        let b = digest.buckets.iter().find(|b| b.name == "partial_mismatch").unwrap();
        assert_eq!(b.committed_size, 0);
        let undigested = digest.buckets.iter().find(|b| b.name == UNDIGESTED).unwrap();
        assert_eq!(undigested.committed_size, 1024); // Both VMO 10 and 20 remain undigested
        Ok(())
    }

    /// What is tested: VMO pager-backed / discardable flags (`ZX_INFO_VMO_PAGER_BACKED` and
    /// `ZX_INFO_VMO_DISCARDABLE`) and their impact on `POPULATED_ANONYMOUS_BYTES`.
    ///
    /// Expectations verified:
    /// - VMOs with pager-backed or discardable flags set have their `scaled_populated_bytes`
    ///   accumulated in `populated_reclaimable_bytes`.
    /// - Verifies that `POPULATED_ANONYMOUS_BYTES` is reduced by the populated reclaimable amount.
    #[test]
    fn test_digest_compute_reclaimable_vmo_flags_and_anonymous_bytes() -> Result<(), anyhow::Error>
    {
        use zx_types::{ZX_INFO_VMO_DISCARDABLE, ZX_INFO_VMO_PAGER_BACKED};
        let mut attr_data = get_attribution_data();
        let vmo_res = attr_data.resources.get_mut(&20).unwrap();
        if let fplugin::ResourceType::Vmo(vmo) = &mut vmo_res.resource.resource_type {
            vmo.flags = Some(ZX_INFO_VMO_PAGER_BACKED | ZX_INFO_VMO_DISCARDABLE);
        }

        let (kernel_stats, kernel_stats_compression) = get_kernel_stats();
        let digest =
            Digest::compute(&attr_data, &kernel_stats, &kernel_stats_compression, &vec![], true)?;

        let anon_bucket =
            digest.buckets.iter().find(|b| b.name == POPULATED_ANONYMOUS_BYTES).unwrap();
        assert_eq!(anon_bucket.populated_size, 0);
        Ok(())
    }

    /// What is tested: `Digest::compute` handling of default/missing (`None`) kernel memory stats
    /// and `saturating_sub` underflow protection for the `ORPHANED` bucket.
    ///
    /// Expectations verified:
    /// - Verifies that `Digest::compute` succeeds without panicking when `fkernel::MemoryStats` and
    ///   `fkernel::MemoryStatsCompression` have all fields set to `None` (`Default::default()`).
    /// - Verifies that `ORPHANED`, `KERNEL`, `FREE`, and all pager/discardable/zram/anonymous
    ///   buckets cleanly default to `0` without underflowing.
    #[test]
    fn test_digest_missing_kernel_stats_and_saturating_orphaned() -> Result<(), anyhow::Error> {
        let digest = Digest::compute(
            &get_attribution_data(),
            &fkernel::MemoryStats::default(),
            &fkernel::MemoryStatsCompression::default(),
            &vec![],
            true,
        )?;

        let orphaned = digest.buckets.iter().find(|b| b.name == ORPHANED).unwrap();
        assert_eq!(orphaned.committed_size, 0);
        let kernel = digest.buckets.iter().find(|b| b.name == KERNEL).unwrap();
        assert_eq!(kernel.committed_size, 0);
        let free = digest.buckets.iter().find(|b| b.name == FREE).unwrap();
        assert_eq!(free.committed_size, 0);
        let anon = digest.buckets.iter().find(|b| b.name == POPULATED_ANONYMOUS_BYTES).unwrap();
        assert_eq!(anon.committed_size, 0);
        let zram = digest.buckets.iter().find(|b| b.name == ZRAM_COMPRESSED_BYTES).unwrap();
        assert_eq!(zram.committed_size, 0);
        Ok(())
    }
}
