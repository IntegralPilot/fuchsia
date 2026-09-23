// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeMap;
use std::ops::{Bound, Range};

use crate::{Error, Mapping};

/// Helper data structure to build and manage a list of Mappings.
pub struct ModuleMapBuilder {
    /// ID that will be assigned to the next added mapping (0 is reserved).
    next_available_id: u64,

    /// Mappings that have been added so far.
    mappings: Vec<Mapping>,

    /// "Reverse lookup table" of the `mappings`, to quickly find the ID of the
    /// mapping at a given address.
    resolver: ModuleMapResolver,
}

impl Default for ModuleMapBuilder {
    fn default() -> ModuleMapBuilder {
        ModuleMapBuilder::new(1)
    }
}

impl ModuleMapBuilder {
    pub fn new(next_available_id: u64) -> ModuleMapBuilder {
        assert_ne!(next_available_id, 0, "Mapping ID 0 is reserved");

        ModuleMapBuilder {
            next_available_id,
            mappings: Vec::new(),
            resolver: ModuleMapResolver { ranges: BTreeMap::new() },
        }
    }

    /// Inserts a new mapping and returns the ID it's been assigned.
    pub fn add_mapping(
        &mut self,
        memory_range: Range<u64>,
        file_offset: u64,
        filename_string_index: i64,
        build_id_string_index: i64,
    ) -> Result<u64, Error> {
        let mapping_id = self.next_available_id;

        // Fail if the new range would intersect one of the existing ones.
        if !self.resolver.try_register(memory_range.start, memory_range.end, mapping_id) {
            return Err(Error::MappingWouldOverlap);
        }

        // Ensure the ID we have just assigned is not reused.
        self.next_available_id += 1;

        self.mappings.push(Mapping {
            id: mapping_id,
            memory_start: memory_range.start,
            memory_limit: memory_range.end,
            file_offset,
            filename: filename_string_index,
            build_id: build_id_string_index,
            ..Default::default()
        });

        Ok(mapping_id)
    }

    /// Consumes this ModuleMapBuilder and returns all the mappings that were created and an object
    /// that resolves addresses to the corresponding mapping ID.
    pub fn build(self) -> (Vec<Mapping>, ModuleMapResolver) {
        (self.mappings, self.resolver)
    }
}

/// Helper data structure to resolve a program address into the corresponding mapping_id.
pub struct ModuleMapResolver {
    // Memory map: range_end -> (range_start, mapping_id)
    // where range_start <= x < range_end are the addresses that resolve to mapping_id.
    ranges: BTreeMap<u64, (u64, u64)>,
}

impl ModuleMapResolver {
    fn try_register(&mut self, range_start: u64, range_end: u64, mapping_id: u64) -> bool {
        assert_ne!(mapping_id, 0, "Mapping ID 0 is reserved");

        // Refuse to register a new range that intersects the existing ones.
        if self.find_intersection(range_start, range_end).is_none() {
            self.ranges.insert(range_end, (range_start, mapping_id));
            true
        } else {
            false
        }
    }

    fn find_intersection(&self, query_start: u64, query_end: u64) -> Option<u64> {
        if let Some((range_end, (range_start, mapping_id))) =
            self.ranges.range((Bound::Excluded(query_start), Bound::Unbounded)).next()
        {
            assert!(query_start < *range_end);
            if *range_start < query_end {
                return Some(*mapping_id);
            }
        }

        None
    }

    /// Resolve an address to the corresponding mapping_id, or return 0 if not found.
    ///
    /// Note: Zero is the value that must be stored in the `mapping_id` field of the `Location`
    /// struct if no mapping is known or available.
    pub fn resolve(&self, address: u64) -> u64 {
        // Given our data structure, it's impossible to have mappings that cover address u64::MAX.
        // We can therefore safely skip searching (and avoid an overflow) in that case.
        if address != u64::MAX {
            if let Some(mapping_id) = self.find_intersection(address, address + 1) {
                return mapping_id;
            }
        }

        0 // Not found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itertools::Itertools;

    // Placeholders for filename and build ID string indices.
    const FAKE_FILENAME_1: i64 = 1;
    const FAKE_FILENAME_2: i64 = 2;
    const FAKE_FILENAME_3: i64 = 3;
    const FAKE_BUILD_ID_1: i64 = 4;
    const FAKE_BUILD_ID_2: i64 = 5;
    const FAKE_BUILD_ID_3: i64 = 6;

    #[test]
    fn test_mapping_can_resolve() {
        let mut mm = ModuleMapBuilder::default();

        // Describes a module that lives between addresses 100 and 200.
        let mapping1_id = mm.add_mapping(100..200, 0, FAKE_FILENAME_1, FAKE_BUILD_ID_1).unwrap();
        assert_ne!(mapping1_id, 0);

        // Describes a module that lives between addresses 1000 and 2000.
        let mapping2_id = mm.add_mapping(1000..1500, 0, FAKE_FILENAME_2, FAKE_BUILD_ID_2).unwrap();
        assert_ne!(mapping2_id, 0);
        let mapping3_id =
            mm.add_mapping(1500..2000, 500, FAKE_FILENAME_2, FAKE_BUILD_ID_2).unwrap();
        assert_ne!(mapping3_id, 0);

        // Describes a module that lives between addresses 800 and 1000.
        let mapping4_id = mm.add_mapping(800..1000, 200, FAKE_FILENAME_3, FAKE_BUILD_ID_3).unwrap();
        assert_ne!(mapping4_id, 0);

        let (table, resolver) = mm.build();

        // Verifies that the table was written properly.
        let mapping1_data =
            Itertools::exactly_one(table.iter().filter(|e| e.id == mapping1_id)).unwrap();
        assert_eq!(mapping1_data.memory_start, 100);
        assert_eq!(mapping1_data.memory_limit, 200);
        assert_eq!(mapping1_data.file_offset, 0);
        assert_eq!(mapping1_data.filename, FAKE_FILENAME_1);
        assert_eq!(mapping1_data.build_id, FAKE_BUILD_ID_1);
        let mapping2_data =
            Itertools::exactly_one(table.iter().filter(|e| e.id == mapping2_id)).unwrap();
        assert_eq!(mapping2_data.memory_start, 1000);
        assert_eq!(mapping2_data.memory_limit, 1500);
        assert_eq!(mapping2_data.file_offset, 0);
        assert_eq!(mapping2_data.filename, FAKE_FILENAME_2);
        assert_eq!(mapping2_data.build_id, FAKE_BUILD_ID_2);
        let mapping3_data =
            Itertools::exactly_one(table.iter().filter(|e| e.id == mapping3_id)).unwrap();
        assert_eq!(mapping3_data.memory_start, 1500);
        assert_eq!(mapping3_data.memory_limit, 2000);
        assert_eq!(mapping3_data.file_offset, 500);
        assert_eq!(mapping3_data.filename, FAKE_FILENAME_2);
        assert_eq!(mapping3_data.build_id, FAKE_BUILD_ID_2);
        let mapping4_data =
            Itertools::exactly_one(table.iter().filter(|e| e.id == mapping4_id)).unwrap();
        assert_eq!(mapping4_data.memory_start, 800);
        assert_eq!(mapping4_data.memory_limit, 1000);
        assert_eq!(mapping4_data.file_offset, 200);
        assert_eq!(mapping4_data.filename, FAKE_FILENAME_3);
        assert_eq!(mapping4_data.build_id, FAKE_BUILD_ID_3);
        assert_eq!(table.len(), 4, "No other entries should be present");

        // Verifies that addresses are resolved properly.
        assert_eq!(resolver.resolve(0), 0 /* not found */);
        assert_eq!(resolver.resolve(99), 0 /* not found */);
        assert_eq!(resolver.resolve(100), mapping1_id);
        assert_eq!(resolver.resolve(199), mapping1_id);
        assert_eq!(resolver.resolve(200), 0 /* not found */);
        assert_eq!(resolver.resolve(799), 0 /* not found */);
        assert_eq!(resolver.resolve(800), mapping4_id);
        assert_eq!(resolver.resolve(999), mapping4_id);
        assert_eq!(resolver.resolve(1000), mapping2_id);
        assert_eq!(resolver.resolve(1499), mapping2_id);
        assert_eq!(resolver.resolve(1500), mapping3_id);
        assert_eq!(resolver.resolve(1700), mapping3_id);
        assert_eq!(resolver.resolve(1999), mapping3_id);
        assert_eq!(resolver.resolve(2000), 0 /* not found */);
        assert_eq!(resolver.resolve(u64::MAX), 0 /* not found */);
    }

    #[test]
    fn test_mapping_cannot_overlap() {
        let mut mm = ModuleMapBuilder::default();

        // Insert a module that lives between addresses 100 and 200.
        let mapping_id = mm.add_mapping(100..200, 0, FAKE_FILENAME_1, FAKE_BUILD_ID_1).unwrap();

        // Try to insert overlapping mapping and verify that they all fail.
        mm.add_mapping(100..200, 0, FAKE_FILENAME_1, FAKE_BUILD_ID_1).expect_err("full range");
        mm.add_mapping(100..150, 0, FAKE_FILENAME_2, FAKE_BUILD_ID_2)
            .expect_err("at the beginning of the range");
        mm.add_mapping(150..200, 0, FAKE_FILENAME_2, FAKE_BUILD_ID_2)
            .expect_err("at the end of the range");
        mm.add_mapping(90..101, 0, FAKE_FILENAME_2, FAKE_BUILD_ID_2)
            .expect_err("crosses the beginning");
        mm.add_mapping(190..210, 0, FAKE_FILENAME_2, FAKE_BUILD_ID_2).expect_err("crosses the end");
        mm.add_mapping(110..190, 0, FAKE_FILENAME_2, FAKE_BUILD_ID_2).expect_err("subset");
        mm.add_mapping(90..210, 0, FAKE_FILENAME_2, FAKE_BUILD_ID_2).expect_err("superset");

        // Verify that the resulting table only contains the good mapping.
        let (table, _) = mm.build();
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].id, mapping_id);
        assert_eq!(table[0].memory_start, 100);
        assert_eq!(table[0].memory_limit, 200);
        assert_eq!(table[0].file_offset, 0);
        assert_eq!(table[0].filename, FAKE_FILENAME_1);
        assert_eq!(table[0].build_id, FAKE_BUILD_ID_1);
    }

    #[test]
    fn test_mapping_id_base() {
        const BASE_ID: u64 = 1234;

        let mut mm = ModuleMapBuilder::new(BASE_ID);
        let mapping1_id = mm.add_mapping(100..200, 0, FAKE_FILENAME_1, FAKE_BUILD_ID_1).unwrap();
        assert_eq!(mapping1_id, BASE_ID + 0);

        let mapping2_id = mm.add_mapping(200..300, 0, FAKE_FILENAME_2, FAKE_BUILD_ID_2).unwrap();
        assert_eq!(mapping2_id, BASE_ID + 1);

        let (table, _) = mm.build();
        assert_eq!(table.len(), 2);
        assert_eq!(table[0].id, mapping1_id);
        assert_eq!(table[1].id, mapping2_id);
    }
}
