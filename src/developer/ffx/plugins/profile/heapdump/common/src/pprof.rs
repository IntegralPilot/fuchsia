// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use ffx_config::EnvironmentContext;
use flate2::Compression;
use heapdump_snapshot_fdomain::{ExecutableRegion, Snapshot};
use itertools::Itertools;
use prost::Message;
use std::collections::HashSet;
use std::collections::hash_map::{Entry, HashMap};
use std::io::Write;

// NOTE(nathaniel): it would be nice if this were available on `HashMap`
// itself; that would spare callers the keystrokes of ignoring the values
// in the comparison function.
fn iter_sorted_by_key<K: Ord, V>(
    map: impl IntoIterator<Item = (K, V)>,
) -> impl Iterator<Item = (K, V)> {
    Itertools::sorted_by(map.into_iter(), |(castor, _), (pollux, _)| Ord::cmp(castor, pollux))
}

// Tries to create a `Symbolizer` to resolve addresses in the given snapshot.
fn instantiate_symbolizer(
    context: &EnvironmentContext,
    snapshot: &Snapshot,
) -> Result<ffx_symbolize::Symbolizer> {
    let mut sorted_regions: Vec<(u64, &ExecutableRegion)> =
        snapshot.executable_regions.iter().map(|(k, v)| (*k, v)).collect();
    sorted_regions.sort_by_key(|(address, _)| *address);

    let mut referenced_regions = HashMap::with_capacity(sorted_regions.len());
    let mut searched_program_addresses = HashSet::new();
    for allocation in &snapshot.allocations {
        for program_address in &allocation.stack_trace.program_addresses {
            if searched_program_addresses.insert(*program_address) {
                let region_search_outcome =
                    sorted_regions.binary_search_by(|(region_starting_address, region)| {
                        if *region_starting_address + region.size <= *program_address {
                            std::cmp::Ordering::Less
                        } else if *program_address < *region_starting_address {
                            std::cmp::Ordering::Greater
                        } else {
                            referenced_regions.insert(*region_starting_address, region);
                            std::cmp::Ordering::Equal
                        }
                    });
                if region_search_outcome.is_err() {
                    // NOTE(https://fxrev.dev/1684875): this can occur following a
                    // believed-to-be-unlikely race condition during on-target data collection.
                    eprintln!(
                        "WARNING: No executable region found for program address {program_address:#016x}"
                    );
                }
            }
        }
    }

    let mut symbolizer = ffx_symbolize::Symbolizer::with_context(context)?;
    let mut symbolizer_module_ids = HashMap::new();
    for (region_starting_address, region) in referenced_regions {
        let base = region_starting_address.wrapping_sub(region.vaddr);
        let module_id = symbolizer_module_ids
            .entry((region.build_id.clone(), base))
            .or_insert_with(|| symbolizer.add_module(&region.name, &region.build_id));
        symbolizer.add_mapping(
            *module_id,
            ffx_symbolize::MappingDetails {
                start_addr: region_starting_address,
                size: region.size,
                vaddr: region.vaddr,
                flags: ffx_symbolize::MappingFlags::EXECUTE,
            },
        )?;
    }
    Ok(symbolizer)
}

pub enum LabelValue<'a> {
    String(&'a str),
    Number(i64),
}

pub struct PProfProfileBuilder<'c> {
    ctx: &'c EnvironmentContext,
    with_tags: bool,
    symbolize: bool,

    st: pprof::StringTableBuilder,
    interned_functions: HashMap<(i64, i64), u64>, // (function_name, file_name) -> id
    resolved_mapping_ids: HashSet<u64>,
    unresolved_mapping_ids: HashSet<u64>,
    pprof: pprof::Profile,
}

impl<'c> PProfProfileBuilder<'c> {
    pub fn new(
        ctx: &'c EnvironmentContext,
        with_tags: bool,
        symbolize: bool,
    ) -> PProfProfileBuilder<'c> {
        let mut st = pprof::StringTableBuilder::default();
        let pprof = pprof::Profile {
            sample_type: vec![
                pprof::ValueType { r#type: st.intern("objects"), unit: st.intern("count") },
                pprof::ValueType { r#type: st.intern("allocated"), unit: st.intern("bytes") },
            ],
            ..Default::default()
        };

        PProfProfileBuilder {
            ctx,
            with_tags,
            symbolize,
            st,
            interned_functions: HashMap::new(),
            resolved_mapping_ids: HashSet::new(),
            unresolved_mapping_ids: HashSet::new(),
            pprof,
        }
    }

    pub fn add<'a>(
        &mut self,
        snapshot: &Snapshot,
        extra_labels: &[(&str, LabelValue<'a>)],
    ) -> Result<()> {
        // Convert the extra labels into a vector in the destination format, which will be used as a
        // template in all the generated samples.
        let label_template = extra_labels
            .iter()
            .map(|(key, value)| {
                let mut converted =
                    pprof::Label { key: self.st.intern(&key), ..Default::default() };
                match value {
                    LabelValue::String(value) => converted.str = self.st.intern(value),
                    LabelValue::Number(value) => converted.num = *value,
                }
                converted
            })
            .collect_vec();

        // Build the Mappings with all the executable regions listed in the snapshot and obtain an
        // object that resolves arbitrary program addresses into the corresponding module IDs.
        let module_map = {
            let next_id = (self.pprof.mapping.len() + 1) as u64;
            let mut builder = pprof::ModuleMapBuilder::new(next_id);

            for (address, region) in iter_sorted_by_key(&snapshot.executable_regions) {
                let limit = *address + region.size;
                let filename_string_index = self.st.intern(&region.name);
                let build_id_string_index = self.st.intern_build_id(&region.build_id);
                builder.add_mapping(
                    *address..limit,
                    region.file_offset,
                    filename_string_index,
                    build_id_string_index,
                )?;
            }

            let (mappings, resolver) = builder.build();
            self.pprof.mapping.extend(mappings.into_iter());
            resolver
        };

        // If symbolization was requested, populate its own view of the mappings too.
        let symbolizer =
            if self.symbolize { Some(instantiate_symbolizer(self.ctx, &snapshot)?) } else { None };

        // Fill the Locations with all the program addresses referenced in the snapshot and store
        // their assigned IDs.
        let mut address_to_location_id = HashMap::new();
        for info in &snapshot.allocations {
            for address in &info.stack_trace.program_addresses {
                if let Entry::Vacant(e) = address_to_location_id.entry(*address) {
                    let mapping_id = module_map.resolve(*address);

                    // Translate the address into symbolized stack frames. While doing it, also
                    // keep track of resolved and unresolved mappings.
                    let symbolized_lines = if mapping_id != 0
                        && let Some(symbolizer) = &symbolizer
                    {
                        match symbolizer.resolve_addr(*address) {
                            Ok(resolved_locations) => {
                                let mut result = Vec::with_capacity(resolved_locations.len());
                                for resolved_location in resolved_locations {
                                    let function_name = &resolved_location.function;
                                    let (file_name, line) =
                                        match resolved_location.file_and_line.as_ref() {
                                            Some((file_name, line)) => {
                                                (file_name.as_str(), i64::from(*line))
                                            }
                                            None => ("", 0),
                                        };

                                    let function_id =
                                        self.intern_function(function_name, &file_name);
                                    result.push(pprof::Line { function_id, line });
                                }

                                // We managed to symbolize an address from this mapping, which proves that
                                // it was resolved.
                                self.resolved_mapping_ids.insert(mapping_id);

                                Some(result)
                            }
                            Err(ffx_symbolize::ResolveError::SymbolNotFound) => {
                                // Even if this specific address could not be symbolized, the mapping as a
                                // whole was resolved.
                                self.resolved_mapping_ids.insert(mapping_id);

                                None
                            }
                            Err(ffx_symbolize::ResolveError::NoOverlappingModule) => {
                                unreachable!("the address belongs to mapping {}", mapping_id)
                            }
                            Err(ffx_symbolize::ResolveError::SymbolFileUnavailable) => {
                                // Do not mark this mapping as resolved.
                                self.unresolved_mapping_ids.insert(mapping_id);

                                None
                            }
                        }
                    } else {
                        None
                    };

                    e.insert(self.insert_location(
                        mapping_id,
                        *address,
                        symbolized_lines.unwrap_or_else(Vec::new),
                    ));
                }
            }
        }

        // Helper function that translates program addresses to location IDs.
        let addresses_to_location_ids = |program_addresses: &[u64]| -> Vec<u64> {
            program_addresses.iter().map(|addr| address_to_location_id[addr]).collect()
        };

        // Fill the Samples.
        if self.with_tags {
            for info in &snapshot.allocations {
                // Cast into a pprof-friendly type.
                let size = info.size as i64;

                let location_ids = addresses_to_location_ids(&info.stack_trace.program_addresses);
                let mut label = label_template.clone();
                if let Some(address) = info.address {
                    label.push(pprof::Label {
                        key: self.st.intern("address"),
                        str: self.st.intern(&format!("0x{:x}", address)),
                        ..Default::default()
                    })
                }
                label.push(pprof::Label {
                    key: self.st.intern("bytes"),
                    num: size,
                    num_unit: self.st.intern("bytes"),
                    ..Default::default()
                });
                if let Some(timestamp) = &info.timestamp {
                    label.push(pprof::Label {
                        key: self.st.intern("timestamp"),
                        num: timestamp.into_nanos(),
                        num_unit: self.st.intern("nanoseconds"),
                        ..Default::default()
                    });
                }
                if let Some(thread_info) = &info.thread_info {
                    label.push(pprof::Label {
                        key: self.st.intern("thread"),
                        str: self.st.intern(&format!("{}[{}]", thread_info.name, thread_info.koid)),
                        ..Default::default()
                    });
                }

                self.pprof.sample.push(pprof::Sample {
                    location_id: location_ids,
                    value: vec![info.count as i64, size],
                    label,
                    ..Default::default()
                });
            }
        } else {
            // Group allocations with the same stack trace (to make the resulting profile smaller).
            let grouped_allocations =
                snapshot.allocations.iter().into_group_map_by(|allocation_info| {
                    addresses_to_location_ids(&allocation_info.stack_trace.program_addresses)
                });

            for (location_ids, allocations_info) in iter_sorted_by_key(grouped_allocations) {
                // Compute totals and cast into pprof-friendly types.
                let size = allocations_info.iter().map(|alloc| alloc.size).sum::<u64>() as i64;
                let count = allocations_info.iter().map(|alloc| alloc.count).sum::<u64>() as i64;

                self.pprof.sample.push(pprof::Sample {
                    location_id: location_ids,
                    value: vec![count, size],
                    label: label_template.clone(),
                    ..Default::default()
                });
            }
        }

        Ok(())
    }

    /// Finalizes the memory profile and returns it as a protobuf message.
    pub fn write_to_message(mut self) -> pprof::Profile {
        self.pprof.string_table = self.st.build();

        // Mark resolved mappings.
        for mapping in &mut self.pprof.mapping {
            if self.resolved_mapping_ids.contains(&mapping.id) {
                mapping.has_functions = true;
                mapping.has_filenames = true;
                mapping.has_line_numbers = true;
                mapping.has_inline_frames = true;
            }
        }

        // Print warnings about unresolved mappings, deduplicating unresolved
        // build IDs that appeared in more then one mapping.
        let unresolved_build_ids = self
            .pprof
            .mapping
            .iter()
            .filter_map(|mapping| {
                if self.unresolved_mapping_ids.contains(&mapping.id) {
                    Some(self.pprof.string_table[mapping.build_id as usize].clone())
                } else {
                    None
                }
            })
            .unique();
        for build_id in unresolved_build_ids {
            eprintln!("WARNING: Unresolved build ID \"{build_id}\".");
        }

        self.pprof
    }

    /// Finalizes the memory profile and serializes it as a pprof-compatible `.pb.gz` file.
    pub fn write_to_file(self, dest: &mut impl Write) -> Result<()> {
        let buf = self.write_to_message().encode_to_vec();
        let mut encoder = flate2::write::GzEncoder::new(dest, Compression::default());
        encoder.write_all(&buf)?;
        encoder.finish()?;
        Ok(())
    }

    fn intern_function(&mut self, function_name: &str, file_name: &str) -> u64 {
        let function_name = self.st.intern(function_name);
        let file_name = self.st.intern(file_name);
        let next_id = (self.interned_functions.len() + 1) as u64;
        *self.interned_functions.entry((function_name, file_name)).or_insert_with(|| {
            self.pprof.function.push(pprof::Function {
                id: next_id,
                name: function_name,
                filename: file_name,
                ..Default::default()
            });
            next_id
        })
    }

    fn insert_location(
        &mut self,
        mapping_id: u64,
        address: u64,
        symbolized_lines: Vec<pprof::Line>,
    ) -> u64 {
        let next_id = (self.pprof.location.len() + 1) as u64;
        self.pprof.location.push(pprof::Location {
            id: next_id,
            mapping_id,
            address,
            line: symbolized_lines,
            ..Default::default()
        });
        next_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::MonotonicInstant;
    use heapdump_snapshot_fdomain::{Allocation, ExecutableRegion, StackTrace, ThreadInfo};
    use maplit::hashmap;
    use std::io::{Read, Seek};
    use std::rc::Rc;
    use test_case::test_case;

    // Placeholder mappings for the fake profile:
    const MAP_1_ADDRESS: u64 = 0x2000;
    const MAP_1_NAME: &str = "map-1";
    const MAP_1_SIZE: u64 = 0x1000;
    const MAP_1_FILE_OFFSET: u64 = 0x1000;
    const MAP_1_VADDR: u64 = 0x2000;
    const MAP_1_BUILD_ID: &str = "112233441122334411223344";
    const MAP_2_ADDRESS: u64 = 0x8000;
    const MAP_2_NAME: &str = "map-2";
    const MAP_2_SIZE: u64 = 0x2000;
    const MAP_2_FILE_OFFSET: u64 = 0;
    const MAP_2_VADDR: u64 = 0x3000;
    const MAP_2_BUILD_ID: &str = "556677885566778855667788";

    // Placeholder code locations (program addresses) for the fake profile:
    const LOC_1_ADDRESS: u64 = 0x2500; // within mapping 1
    const LOC_2_ADDRESS: u64 = 0x8900; // within mapping 2
    const LOC_3_ADDRESS: u64 = 0x9100; // within mapping 2
    const LOC_4_ADDRESS: u64 = 0; // outside of any known mapping
    const LOC_5_ADDRESS: u64 = 0x5123; // outside of any known mapping

    // Placeholder stack traces for the fake profile:
    const STACK_TRACE_A: &[u64] = &[LOC_1_ADDRESS, LOC_2_ADDRESS, LOC_3_ADDRESS];
    const STACK_TRACE_B: &[u64] = &[LOC_1_ADDRESS, LOC_4_ADDRESS, LOC_5_ADDRESS];
    const STACK_TRACE_C: &[u64] = &[LOC_2_ADDRESS, LOC_3_ADDRESS];

    // Placeholder allocations for the fake profile:
    const ALLOC_1_ADDRESS: u64 = 0x611000;
    const ALLOC_1_SIZE: i64 = 0x1800;
    const ALLOC_1_COUNT: u64 = 1;
    const ALLOC_1_THREAD_KOID: u64 = 1234;
    const ALLOC_1_THREAD_NAME: &str = "thread-1";
    const ALLOC_1_TIMESTAMP: MonotonicInstant = MonotonicInstant::from_nanos(8777777777778);
    const ALLOC_2_ADDRESS: u64 = 0x624000;
    const ALLOC_2_SIZE: i64 = 0x30;
    const ALLOC_2_COUNT: u64 = 1;
    const ALLOC_2_THREAD_KOID: u64 = 5678;
    const ALLOC_2_THREAD_NAME: &str = "thread-2";
    const ALLOC_2_TIMESTAMP: MonotonicInstant = MonotonicInstant::from_nanos(9333333333333);
    const ALLOC_3_SIZE: i64 = 0xC000;
    const ALLOC_3_COUNT: u64 = 3;
    const ALLOC_3_THREAD_KOID: u64 = 9999;
    const ALLOC_3_THREAD_NAME: &str = "thread-3";
    const ALLOC_3_TIMESTAMP: MonotonicInstant = MonotonicInstant::from_nanos(9876543211111);

    const EXTRA_LABEL_KEY: &str = "extralabel";
    const EXTRA_LABEL_VALUE_1: i64 = 123;
    const EXTRA_LABEL_VALUE_2: &str = "testvalue";

    fn generate_fake_snapshot() -> Snapshot {
        let stack_trace_a = Rc::new(StackTrace { program_addresses: STACK_TRACE_A.to_vec() });
        let stack_trace_b = Rc::new(StackTrace { program_addresses: STACK_TRACE_B.to_vec() });

        Snapshot {
            allocations: vec![
                Allocation {
                    address: Some(ALLOC_1_ADDRESS),
                    size: ALLOC_1_SIZE.try_into().unwrap(),
                    count: ALLOC_1_COUNT,
                    thread_info: Some(Rc::new(ThreadInfo {
                        koid: ALLOC_1_THREAD_KOID,
                        name: ALLOC_1_THREAD_NAME.to_string(),
                    })),
                    stack_trace: stack_trace_a.clone(),
                    timestamp: Some(ALLOC_1_TIMESTAMP),
                    contents: None,
                },
                Allocation {
                    address: Some(ALLOC_2_ADDRESS),
                    size: ALLOC_2_SIZE.try_into().unwrap(),
                    count: ALLOC_2_COUNT,
                    thread_info: Some(Rc::new(ThreadInfo {
                        koid: ALLOC_2_THREAD_KOID,
                        name: ALLOC_2_THREAD_NAME.to_string(),
                    })),
                    stack_trace: stack_trace_b.clone(),
                    timestamp: Some(ALLOC_2_TIMESTAMP),
                    contents: None,
                },
                Allocation {
                    address: None,
                    size: ALLOC_3_SIZE.try_into().unwrap(),
                    count: ALLOC_3_COUNT,
                    thread_info: Some(Rc::new(ThreadInfo {
                        koid: ALLOC_3_THREAD_KOID,
                        name: ALLOC_3_THREAD_NAME.to_string(),
                    })),
                    stack_trace: stack_trace_b.clone(),
                    timestamp: Some(ALLOC_3_TIMESTAMP),
                    contents: None,
                },
            ],
            executable_regions: hashmap![
                MAP_1_ADDRESS => ExecutableRegion {
                    name: MAP_1_NAME.to_string(),
                    size: MAP_1_SIZE,
                    file_offset: MAP_1_FILE_OFFSET,
                    vaddr: MAP_1_VADDR,
                    build_id: hex::decode(MAP_1_BUILD_ID).unwrap(),
                },
                MAP_2_ADDRESS => ExecutableRegion {
                    name: MAP_2_NAME.to_string(),
                    size: MAP_2_SIZE,
                    file_offset: MAP_2_FILE_OFFSET,
                    vaddr: MAP_2_VADDR,
                    build_id: hex::decode(MAP_2_BUILD_ID).unwrap(),
                },
            ],
        }
    }

    struct ProfileHelper<'a> {
        profile: &'a pprof::Profile,
        address_idx: Option<i64>,
        bytes_idx: Option<i64>,
        timestamp_idx: Option<i64>,
        thread_idx: Option<i64>,
    }
    impl<'a> ProfileHelper<'a> {
        fn new(profile: &'a pprof::Profile) -> ProfileHelper<'a> {
            let lookup = |text: &str| {
                (&profile.string_table).iter().position(|s| s == text).map(|i| i as i64)
            };
            ProfileHelper {
                profile,
                address_idx: lookup("address"),
                bytes_idx: lookup("bytes"),
                timestamp_idx: lookup("timestamp"),
                thread_idx: lookup("thread"),
            }
        }
        fn address(&self, sample: &'a pprof::Sample) -> Option<&'a String> {
            sample
                .label
                .iter()
                .find(|l| l.key == self.address_idx.unwrap())
                .map(|l| &self.profile.string_table[usize::try_from(l.str).unwrap()])
        }
        fn bytes(&self, sample: &'a pprof::Sample) -> Option<i64> {
            self.bytes_idx.and_then(|bytes_idx| {
                sample.label.iter().find(|l| l.key == bytes_idx).map(|l| l.num)
            })
        }
        fn timestamp(&self, sample: &'a pprof::Sample) -> Option<i64> {
            self.timestamp_idx.and_then(|timestamp_idx| {
                sample.label.iter().find(|l| l.key == timestamp_idx).map(|l| l.num)
            })
        }
        fn thread(&self, sample: &'a pprof::Sample) -> Option<&'a String> {
            self.thread_idx.and_then(|thread_idx| {
                sample
                    .label
                    .iter()
                    .find(|l| l.key == thread_idx)
                    .map(|l| &self.profile.string_table[usize::try_from(l.str).unwrap()])
            })
        }
    }

    fn assert_profile_matches_fake_snapshot(profile: &pprof::Profile, with_tags: bool) {
        // Helper function to access the string table.
        let st = |index: i64| profile.string_table[usize::try_from(index).unwrap()].as_str();
        let h = ProfileHelper::new(profile);

        // Helper function to resolve location IDs.
        let loc = |location_id: u64| profile.location.iter().find(|e| e.id == location_id).unwrap();

        // Verify the string table.
        assert_eq!(st(0), "", "The first entry in the string table should always be empty");

        // Verify that samples' data format.
        assert_eq!(profile.sample_type.len(), 2);
        assert_eq!(st(profile.sample_type[0].r#type), "objects");
        assert_eq!(st(profile.sample_type[0].unit), "count");
        assert_eq!(st(profile.sample_type[1].r#type), "allocated");
        assert_eq!(st(profile.sample_type[1].unit), "bytes");

        if with_tags {
            // Verify the tags' labels.
            for sample in &profile.sample {
                for label in &sample.label {
                    match st(label.key) {
                        "address" => {}
                        "bytes" => assert_eq!(st(label.num_unit), "bytes"),
                        "timestamp" => assert_eq!(st(label.num_unit), "nanoseconds"),
                        "thread" => {}
                        _ => unreachable!(),
                    }
                }
            }

            // Identify the three allocations from their sizes (which are unique in our sample snapshot)
            // and verify them.
            assert_eq!(profile.sample.len(), 3);
            let allocation1 =
                profile.sample.iter().find(|s| h.bytes(s) == Some(ALLOC_1_SIZE)).unwrap();
            assert_eq!(allocation1.value[0], ALLOC_1_COUNT as i64);
            assert_eq!(allocation1.value[1], ALLOC_1_SIZE);
            assert_eq!(h.address(allocation1), Some(&format!("0x{:x}", ALLOC_1_ADDRESS)));
            assert_eq!(allocation1.location_id.len(), STACK_TRACE_A.len());
            assert_eq!(loc(allocation1.location_id[0]).address, STACK_TRACE_A[0]);
            assert_eq!(loc(allocation1.location_id[1]).address, STACK_TRACE_A[1]);
            assert_eq!(loc(allocation1.location_id[2]).address, STACK_TRACE_A[2]);
            assert_eq!(h.timestamp(allocation1), Some(ALLOC_1_TIMESTAMP.into_nanos()));
            assert_eq!(
                h.thread(allocation1),
                Some(&format!("{}[{}]", ALLOC_1_THREAD_NAME, ALLOC_1_THREAD_KOID))
            );
            let allocation2 =
                profile.sample.iter().find(|s| h.bytes(s) == Some(ALLOC_2_SIZE)).unwrap();
            assert_eq!(allocation2.value[0], ALLOC_2_COUNT as i64);
            assert_eq!(allocation2.value[1], ALLOC_2_SIZE);
            assert_eq!(h.address(allocation2), Some(&format!("0x{:x}", ALLOC_2_ADDRESS)));
            assert_eq!(allocation2.location_id.len(), STACK_TRACE_B.len());
            assert_eq!(loc(allocation2.location_id[0]).address, STACK_TRACE_B[0]);
            assert_eq!(loc(allocation2.location_id[1]).address, STACK_TRACE_B[1]);
            assert_eq!(loc(allocation2.location_id[2]).address, STACK_TRACE_B[2]);
            assert_eq!(h.timestamp(allocation2), Some(ALLOC_2_TIMESTAMP.into_nanos()));
            assert_eq!(
                h.thread(allocation2),
                Some(&format!("{}[{}]", ALLOC_2_THREAD_NAME, ALLOC_2_THREAD_KOID))
            );
            let allocation3 =
                profile.sample.iter().find(|s| h.bytes(s) == Some(ALLOC_3_SIZE)).unwrap();
            assert_eq!(allocation3.value[0], ALLOC_3_COUNT as i64);
            assert_eq!(allocation3.value[1], ALLOC_3_SIZE);
            assert_eq!(h.address(allocation3), None);
            assert_eq!(allocation3.location_id.len(), STACK_TRACE_B.len());
            assert_eq!(loc(allocation3.location_id[0]).address, STACK_TRACE_B[0]);
            assert_eq!(loc(allocation3.location_id[1]).address, STACK_TRACE_B[1]);
            assert_eq!(loc(allocation3.location_id[2]).address, STACK_TRACE_B[2]);
            assert_eq!(h.timestamp(allocation3), Some(ALLOC_3_TIMESTAMP.into_nanos()));
            assert_eq!(
                h.thread(allocation3),
                Some(&format!("{}[{}]", ALLOC_3_THREAD_NAME, ALLOC_3_THREAD_KOID))
            );
        } else {
            // Verify that the samples were aggregated by stack trace correctly and
            // also verify that they are sorted by location ID.
            assert_eq!(profile.sample.len(), 2);
            assert_eq!(profile.sample[0].value[0], ALLOC_1_COUNT as i64);
            assert_eq!(profile.sample[0].value[1], ALLOC_1_SIZE);
            assert_eq!(profile.sample[0].location_id.len(), STACK_TRACE_A.len());
            assert_eq!(loc(profile.sample[0].location_id[0]).address, STACK_TRACE_A[0]);
            assert_eq!(loc(profile.sample[0].location_id[1]).address, STACK_TRACE_A[1]);
            assert_eq!(loc(profile.sample[0].location_id[2]).address, STACK_TRACE_A[2]);
            assert_eq!(profile.sample[1].value[0], (ALLOC_2_COUNT + ALLOC_3_COUNT) as i64);
            assert_eq!(profile.sample[1].value[1], ALLOC_2_SIZE + ALLOC_3_SIZE);
            assert_eq!(profile.sample[1].location_id.len(), STACK_TRACE_B.len());
            assert_eq!(loc(profile.sample[1].location_id[0]).address, STACK_TRACE_B[0]);
            assert_eq!(loc(profile.sample[1].location_id[1]).address, STACK_TRACE_B[1]);
            assert_eq!(loc(profile.sample[1].location_id[2]).address, STACK_TRACE_B[2]);
        }

        // Identify the mappings from their addresses and verify them as well as
        // that they are sorted by their starting address.
        assert_eq!(profile.mapping.len(), 2);
        assert_eq!(profile.mapping.iter().filter(|m| m.id == 0).next(), None, "ID 0 is reserved");
        assert_eq!(profile.mapping[0].memory_start, MAP_1_ADDRESS);
        assert_eq!(profile.mapping[0].memory_limit, MAP_1_ADDRESS + MAP_1_SIZE);
        assert_eq!(profile.mapping[0].file_offset, MAP_1_FILE_OFFSET);
        assert_eq!(st(profile.mapping[0].filename), MAP_1_NAME);
        assert_eq!(st(profile.mapping[0].build_id), MAP_1_BUILD_ID);
        assert_eq!(profile.mapping[1].memory_start, MAP_2_ADDRESS);
        assert_eq!(profile.mapping[1].memory_limit, MAP_2_ADDRESS + MAP_2_SIZE);
        assert_eq!(profile.mapping[1].file_offset, MAP_2_FILE_OFFSET);
        assert_eq!(st(profile.mapping[1].filename), MAP_2_NAME);
        assert_eq!(st(profile.mapping[1].build_id), MAP_2_BUILD_ID);

        // Identify the locations from their addresses and verify them.
        assert_eq!(profile.location.len(), 5);
        assert_eq!(profile.location.iter().filter(|l| l.id == 0).next(), None, "ID 0 is reserved");
        let loc1 = profile.location.iter().find(|l| l.address == LOC_1_ADDRESS).unwrap();
        assert_eq!(loc1.mapping_id, profile.mapping[0].id, "LOC_1_ADDRESS belongs to mapping 1");
        let loc2 = profile.location.iter().find(|l| l.address == LOC_2_ADDRESS).unwrap();
        assert_eq!(loc2.mapping_id, profile.mapping[1].id, "LOC_2_ADDRESS belongs to mapping 2");
        let loc3 = profile.location.iter().find(|l| l.address == LOC_3_ADDRESS).unwrap();
        assert_eq!(loc3.mapping_id, profile.mapping[1].id, "LOC_3_ADDRESS belongs to mapping 2");
        let loc4 = profile.location.iter().find(|l| l.address == LOC_4_ADDRESS).unwrap();
        assert_eq!(loc4.mapping_id, 0, "LOC_4_ADDRESS does not belong to any mapping");
        let loc5 = profile.location.iter().find(|l| l.address == LOC_5_ADDRESS).unwrap();
        assert_eq!(loc5.mapping_id, 0, "LOC_5_ADDRESS does not belong to any mapping");
    }

    /// Verifies that the protobuf message generated by `PProfProfileBuilder` contains correct data.
    #[test_case(true ; "with tags")]
    #[test_case(false ; "aggregated")]
    fn test_build_profile(with_tags: bool) {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let snapshot = generate_fake_snapshot();
        let mut builder = PProfProfileBuilder::new(&env.context, with_tags, false);
        builder.add(&snapshot, &[]).unwrap();
        let profile = builder.write_to_message();
        assert_profile_matches_fake_snapshot(&profile, with_tags);
    }

    /// Verifies that the file written by `PProfProfileBuilder` can be read back.
    #[test_case(true ; "with tags")]
    #[test_case(false ; "aggregated")]
    fn test_export_to_pprof(with_tags: bool) {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        // Create a temporary file.
        let mut tempfile = tempfile::tempfile().unwrap();

        // Write a snapshot to it.
        let snapshot = generate_fake_snapshot();
        let mut builder = PProfProfileBuilder::new(&env.context, with_tags, false);
        builder.add(&snapshot, &[]).unwrap();
        builder.write_to_file(&mut tempfile).unwrap();

        // Read it back.
        let mut buf = Vec::new();
        tempfile.rewind().unwrap();
        let mut decoder = flate2::read::GzDecoder::new(tempfile);
        decoder.read_to_end(&mut buf).unwrap();

        // Verify that it can be decoded correctly.
        let profile = pprof::Profile::decode(&buf[..]).unwrap();
        assert_profile_matches_fake_snapshot(&profile, with_tags);
    }

    fn generate_fake_aggregated_snapshot() -> Snapshot {
        let stack_trace_a = Rc::new(StackTrace { program_addresses: STACK_TRACE_A.to_vec() });
        let stack_trace_b = Rc::new(StackTrace { program_addresses: STACK_TRACE_B.to_vec() });

        Snapshot {
            allocations: vec![
                Allocation {
                    address: None,
                    size: ALLOC_1_SIZE.try_into().unwrap(),
                    count: ALLOC_1_COUNT,
                    thread_info: None,
                    stack_trace: stack_trace_a.clone(),
                    timestamp: None,
                    contents: None,
                },
                Allocation {
                    address: None,
                    size: ALLOC_2_SIZE.try_into().unwrap(),
                    count: ALLOC_2_COUNT,
                    thread_info: None,
                    stack_trace: stack_trace_b.clone(),
                    timestamp: None,
                    contents: None,
                },
                Allocation {
                    address: None,
                    size: ALLOC_3_SIZE.try_into().unwrap(),
                    count: ALLOC_3_COUNT,
                    thread_info: None,
                    stack_trace: stack_trace_b.clone(),
                    timestamp: None,
                    contents: None,
                },
            ],
            executable_regions: hashmap![
                MAP_1_ADDRESS => ExecutableRegion {
                    name: MAP_1_NAME.to_string(),
                    size: MAP_1_SIZE,
                    file_offset: MAP_1_FILE_OFFSET,
                    vaddr: MAP_1_VADDR,
                    build_id: hex::decode(MAP_1_BUILD_ID).unwrap(),
                },
                MAP_2_ADDRESS => ExecutableRegion {
                    name: MAP_2_NAME.to_string(),
                    size: MAP_2_SIZE,
                    file_offset: MAP_2_FILE_OFFSET,
                    vaddr: MAP_2_VADDR,
                    build_id: hex::decode(MAP_2_BUILD_ID).unwrap(),
                },
            ],
        }
    }

    fn assert_profile_matches_fake_aggregated_snapshot(profile: &pprof::Profile, with_tags: bool) {
        // Helper function to access the string table.
        let st = |index: i64| profile.string_table[usize::try_from(index).unwrap()].as_str();

        // Helper function to resolve location IDs.
        let loc = |location_id: u64| profile.location.iter().find(|e| e.id == location_id).unwrap();

        // Verify the string table.
        assert_eq!(st(0), "", "The first entry in the string table should always be empty");

        // Verify that samples' data format.
        assert_eq!(profile.sample_type.len(), 2);
        assert_eq!(st(profile.sample_type[0].r#type), "objects");
        assert_eq!(st(profile.sample_type[0].unit), "count");
        assert_eq!(st(profile.sample_type[1].r#type), "allocated");
        assert_eq!(st(profile.sample_type[1].unit), "bytes");

        if with_tags {
            // Verify the tags' labels.
            for sample in &profile.sample {
                assert_eq!(sample.value.len(), profile.sample_type.len());
                assert_eq!(sample.label.len(), 1);
                assert_eq!(st(sample.label[0].key), "bytes");
                assert_eq!(st(sample.label[0].num_unit), "bytes");
            }

            // Identify the three allocations from their sizes (which are unique in our sample snapshot)
            // and verify them.
            assert_eq!(profile.sample.len(), 3);
            let allocation1 =
                profile.sample.iter().find(|s| s.label[0].num == ALLOC_1_SIZE).unwrap();
            assert_eq!(allocation1.value[0], ALLOC_1_COUNT as i64);
            assert_eq!(allocation1.value[1], ALLOC_1_SIZE);
            assert_eq!(allocation1.location_id.len(), STACK_TRACE_A.len());
            assert_eq!(loc(allocation1.location_id[0]).address, STACK_TRACE_A[0]);
            assert_eq!(loc(allocation1.location_id[1]).address, STACK_TRACE_A[1]);
            assert_eq!(loc(allocation1.location_id[2]).address, STACK_TRACE_A[2]);

            let allocation2 =
                profile.sample.iter().find(|s| s.label[0].num == ALLOC_2_SIZE).unwrap();
            assert_eq!(allocation2.value[0], ALLOC_2_COUNT as i64);
            assert_eq!(allocation2.value[1], ALLOC_2_SIZE);
            assert_eq!(allocation2.location_id.len(), STACK_TRACE_B.len());
            assert_eq!(loc(allocation2.location_id[0]).address, STACK_TRACE_B[0]);
            assert_eq!(loc(allocation2.location_id[1]).address, STACK_TRACE_B[1]);
            assert_eq!(loc(allocation2.location_id[2]).address, STACK_TRACE_B[2]);

            let allocation3 =
                profile.sample.iter().find(|s| s.label[0].num == ALLOC_3_SIZE).unwrap();
            assert_eq!(allocation3.value[0], ALLOC_3_COUNT as i64);
            assert_eq!(allocation3.value[1], ALLOC_3_SIZE);
            assert_eq!(allocation3.location_id.len(), STACK_TRACE_B.len());
            assert_eq!(loc(allocation3.location_id[0]).address, STACK_TRACE_B[0]);
            assert_eq!(loc(allocation3.location_id[1]).address, STACK_TRACE_B[1]);
            assert_eq!(loc(allocation2.location_id[2]).address, STACK_TRACE_B[2]);
        } else {
            // Verify that the samples were aggregated by stack trace correctly and
            // also verify that they are sorted by location ID.
            assert_eq!(profile.sample.len(), 2);
            assert_eq!(profile.sample[0].value[0], ALLOC_1_COUNT as i64);
            assert_eq!(profile.sample[0].value[1], ALLOC_1_SIZE);
            assert_eq!(profile.sample[0].location_id.len(), STACK_TRACE_A.len());
            assert_eq!(loc(profile.sample[0].location_id[0]).address, STACK_TRACE_A[0]);
            assert_eq!(loc(profile.sample[0].location_id[1]).address, STACK_TRACE_A[1]);
            assert_eq!(loc(profile.sample[0].location_id[2]).address, STACK_TRACE_A[2]);
            assert_eq!(profile.sample[1].value[0], (ALLOC_2_COUNT + ALLOC_3_COUNT) as i64);
            assert_eq!(profile.sample[1].value[1], ALLOC_2_SIZE + ALLOC_3_SIZE);
            assert_eq!(profile.sample[1].location_id.len(), STACK_TRACE_B.len());
            assert_eq!(loc(profile.sample[1].location_id[0]).address, STACK_TRACE_B[0]);
            assert_eq!(loc(profile.sample[1].location_id[1]).address, STACK_TRACE_B[1]);
            assert_eq!(loc(profile.sample[1].location_id[2]).address, STACK_TRACE_B[2]);
        }

        // Identify the mappings from their addresses and verify them as well as
        // that they are sorted by their starting address.
        assert_eq!(profile.mapping.len(), 2);
        assert_eq!(profile.mapping.iter().filter(|m| m.id == 0).next(), None, "ID 0 is reserved");
        assert_eq!(profile.mapping[0].memory_start, MAP_1_ADDRESS);
        assert_eq!(profile.mapping[0].memory_limit, MAP_1_ADDRESS + MAP_1_SIZE);
        assert_eq!(profile.mapping[0].file_offset, MAP_1_FILE_OFFSET);
        assert_eq!(st(profile.mapping[0].filename), MAP_1_NAME);
        assert_eq!(st(profile.mapping[0].build_id), MAP_1_BUILD_ID);
        assert_eq!(profile.mapping[1].memory_start, MAP_2_ADDRESS);
        assert_eq!(profile.mapping[1].memory_limit, MAP_2_ADDRESS + MAP_2_SIZE);
        assert_eq!(profile.mapping[1].file_offset, MAP_2_FILE_OFFSET);
        assert_eq!(st(profile.mapping[1].filename), MAP_2_NAME);
        assert_eq!(st(profile.mapping[1].build_id), MAP_2_BUILD_ID);

        // Identify the locations from their addresses and verify them.
        assert_eq!(profile.location.len(), 5);
        assert_eq!(profile.location.iter().filter(|l| l.id == 0).next(), None, "ID 0 is reserved");
        let loc1 = profile.location.iter().find(|l| l.address == LOC_1_ADDRESS).unwrap();
        assert_eq!(loc1.mapping_id, profile.mapping[0].id, "LOC_1_ADDRESS belongs to mapping 1");
        let loc2 = profile.location.iter().find(|l| l.address == LOC_2_ADDRESS).unwrap();
        assert_eq!(loc2.mapping_id, profile.mapping[1].id, "LOC_2_ADDRESS belongs to mapping 2");
        let loc3 = profile.location.iter().find(|l| l.address == LOC_3_ADDRESS).unwrap();
        assert_eq!(loc3.mapping_id, profile.mapping[1].id, "LOC_3_ADDRESS belongs to mapping 2");
        let loc4 = profile.location.iter().find(|l| l.address == LOC_4_ADDRESS).unwrap();
        assert_eq!(loc4.mapping_id, 0, "LOC_4_ADDRESS does not belong to any mapping");
        let loc5 = profile.location.iter().find(|l| l.address == LOC_5_ADDRESS).unwrap();
        assert_eq!(loc5.mapping_id, 0, "LOC_5_ADDRESS does not belong to any mapping");
    }

    /// Verifies that the protobuf message generated by `build_profile` contains correct data.
    #[test_case(true ; "with tags")]
    #[test_case(false ; "aggregated")]
    fn test_build_profile_from_aggregated_snapshot(with_tags: bool) {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let snapshot = generate_fake_aggregated_snapshot();
        let mut builder = PProfProfileBuilder::new(&env.context, with_tags, false);
        builder.add(&snapshot, &[]).unwrap();
        let profile = builder.write_to_message();
        assert_profile_matches_fake_aggregated_snapshot(&profile, with_tags);
    }

    fn generate_two_fake_snapshots() -> (Snapshot, Snapshot) {
        let stack_trace_b = Rc::new(StackTrace { program_addresses: STACK_TRACE_B.to_vec() });
        let stack_trace_c = Rc::new(StackTrace { program_addresses: STACK_TRACE_C.to_vec() });

        let snapshot1 = Snapshot {
            allocations: vec![Allocation {
                address: None,
                size: ALLOC_1_SIZE.try_into().unwrap(),
                count: ALLOC_1_COUNT,
                thread_info: None,
                stack_trace: stack_trace_b.clone(),
                timestamp: None,
                contents: None,
            }],
            executable_regions: hashmap![
                MAP_1_ADDRESS => ExecutableRegion {
                    name: MAP_1_NAME.to_string(),
                    size: MAP_1_SIZE,
                    file_offset: MAP_1_FILE_OFFSET,
                    vaddr: MAP_1_VADDR,
                    build_id: hex::decode(MAP_1_BUILD_ID).unwrap(),
                },
            ],
        };

        let snapshot2 = Snapshot {
            allocations: vec![Allocation {
                address: None,
                size: ALLOC_2_SIZE.try_into().unwrap(),
                count: ALLOC_2_COUNT,
                thread_info: None,
                stack_trace: stack_trace_c.clone(),
                timestamp: None,
                contents: None,
            }],
            executable_regions: hashmap![
                MAP_2_ADDRESS => ExecutableRegion {
                    name: MAP_2_NAME.to_string(),
                    size: MAP_2_SIZE,
                    file_offset: MAP_2_FILE_OFFSET,
                    vaddr: MAP_2_VADDR,
                    build_id: hex::decode(MAP_2_BUILD_ID).unwrap(),
                },
            ],
        };

        (snapshot1, snapshot2)
    }

    fn assert_profile_matches_two_fake_snapshots_with_extra_labels(profile: &pprof::Profile) {
        // Helper function to access the string table.
        let st = |index: i64| profile.string_table[usize::try_from(index).unwrap()].as_str();

        // Helper function to resolve location IDs.
        let loc = |location_id: u64| profile.location.iter().find(|e| e.id == location_id).unwrap();

        // Helper function to find the extra label.
        let extra_label = |labels: &[pprof::Label]| {
            labels.iter().find(|l| st(l.key) == EXTRA_LABEL_KEY).unwrap().clone()
        };

        // Verify the string table.
        assert_eq!(st(0), "", "The first entry in the string table should always be empty");

        // Verify that samples' data format.
        assert_eq!(profile.sample_type.len(), 2);
        assert_eq!(st(profile.sample_type[0].r#type), "objects");
        assert_eq!(st(profile.sample_type[0].unit), "count");
        assert_eq!(st(profile.sample_type[1].r#type), "allocated");
        assert_eq!(st(profile.sample_type[1].unit), "bytes");

        // Identify the two allocations from their sizes (which are unique in our sample snapshot)
        // and verify them.
        assert_eq!(profile.sample.len(), 2);
        let allocation1 = profile.sample.iter().find(|s| s.value[1] == ALLOC_1_SIZE).unwrap();
        assert_eq!(allocation1.value[0], ALLOC_1_COUNT as i64);
        assert_eq!(allocation1.value[1], ALLOC_1_SIZE);
        assert_eq!(allocation1.location_id.len(), STACK_TRACE_B.len());
        assert_eq!(loc(allocation1.location_id[0]).address, STACK_TRACE_B[0]);
        assert_eq!(loc(allocation1.location_id[1]).address, STACK_TRACE_B[1]);
        assert_eq!(loc(allocation1.location_id[2]).address, STACK_TRACE_B[2]);
        assert_eq!(extra_label(allocation1.label.as_slice()).num, EXTRA_LABEL_VALUE_1);

        let allocation2 = profile.sample.iter().find(|s| s.value[1] == ALLOC_2_SIZE).unwrap();
        assert_eq!(allocation2.value[0], ALLOC_2_COUNT as i64);
        assert_eq!(allocation2.value[1], ALLOC_2_SIZE);
        assert_eq!(allocation2.location_id.len(), STACK_TRACE_C.len());
        assert_eq!(loc(allocation2.location_id[0]).address, STACK_TRACE_C[0]);
        assert_eq!(loc(allocation2.location_id[1]).address, STACK_TRACE_C[1]);
        assert_eq!(st(extra_label(allocation2.label.as_slice()).str), EXTRA_LABEL_VALUE_2);

        // Identify the mappings from their addresses and verify them.
        assert_eq!(profile.mapping.len(), 2);
        assert_eq!(profile.mapping.iter().filter(|m| m.id == 0).next(), None, "ID 0 is reserved");
        let mapping1 = profile.mapping.iter().find(|m| m.memory_start == MAP_1_ADDRESS).unwrap();
        assert_eq!(mapping1.memory_limit, MAP_1_ADDRESS + MAP_1_SIZE);
        assert_eq!(mapping1.file_offset, MAP_1_FILE_OFFSET);
        assert_eq!(st(mapping1.filename), MAP_1_NAME);
        assert_eq!(st(mapping1.build_id), MAP_1_BUILD_ID);
        let mapping2 = profile.mapping.iter().find(|m| m.memory_start == MAP_2_ADDRESS).unwrap();
        assert_eq!(mapping2.memory_limit, MAP_2_ADDRESS + MAP_2_SIZE);
        assert_eq!(mapping2.file_offset, MAP_2_FILE_OFFSET);
        assert_eq!(st(mapping2.filename), MAP_2_NAME);
        assert_eq!(st(mapping2.build_id), MAP_2_BUILD_ID);
    }

    #[test]
    fn test_build_profile_from_two_snapshots() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let (snapshot1, snapshot2) = generate_two_fake_snapshots();
        let mut builder = PProfProfileBuilder::new(&env.context, true, false);
        builder
            .add(&snapshot1, &[(EXTRA_LABEL_KEY, LabelValue::Number(EXTRA_LABEL_VALUE_1))])
            .unwrap();
        builder
            .add(&snapshot2, &[(EXTRA_LABEL_KEY, LabelValue::String(EXTRA_LABEL_VALUE_2))])
            .unwrap();
        let profile = builder.write_to_message();
        assert_profile_matches_two_fake_snapshots_with_extra_labels(&profile);
    }

    #[test]
    fn test_symbolize_multiple_mappings_same_build_id() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let snapshot = Snapshot {
            allocations: vec![Allocation {
                address: None,
                size: ALLOC_1_SIZE.try_into().unwrap(),
                count: ALLOC_1_COUNT,
                thread_info: None,
                stack_trace: Rc::new(StackTrace {
                    program_addresses: vec![LOC_1_ADDRESS, LOC_2_ADDRESS],
                }),
                timestamp: None,
                contents: None,
            }],
            executable_regions: hashmap![
                MAP_1_ADDRESS => ExecutableRegion {
                    name: MAP_1_NAME.to_string(),
                    size: MAP_1_SIZE,
                    file_offset: MAP_1_FILE_OFFSET,
                    vaddr: MAP_1_VADDR,
                    build_id: hex::decode(MAP_1_BUILD_ID).unwrap(),
                },
                MAP_2_ADDRESS => ExecutableRegion {
                    name: MAP_2_NAME.to_string(),
                    size: MAP_2_SIZE,
                    file_offset: MAP_2_FILE_OFFSET,
                    vaddr: MAP_2_VADDR,
                    build_id: hex::decode(MAP_1_BUILD_ID).unwrap(),
                },
            ],
        };

        let mut builder = PProfProfileBuilder::new(&env.context, false, true);
        builder.add(&snapshot, &[]).unwrap();
        let profile = builder.write_to_message();

        assert_eq!(profile.mapping.len(), 2);
        assert_eq!(profile.location.len(), 2);

        let loc1 = profile.location.iter().find(|l| l.address == LOC_1_ADDRESS).unwrap();
        let loc2 = profile.location.iter().find(|l| l.address == LOC_2_ADDRESS).unwrap();
        let map1 = profile.mapping.iter().find(|m| m.memory_start == MAP_1_ADDRESS).unwrap();
        let map2 = profile.mapping.iter().find(|m| m.memory_start == MAP_2_ADDRESS).unwrap();
        assert_eq!(loc1.mapping_id, map1.id);
        assert_eq!(loc2.mapping_id, map2.id);
    }
}
