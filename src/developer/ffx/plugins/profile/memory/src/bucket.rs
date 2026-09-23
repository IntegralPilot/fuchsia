// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Bucketting related functionality.

use crate::processed::{Process, Vmo, VmoKoid};
use crate::raw::BucketDefinition;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Contains the result of a bucketting.
#[derive(Serialize, PartialEq, Debug)]
pub struct Bucket {
    /// Human-readable name of the bucket.
    pub name: String,
    /// Sum of the committed bytes of the VMOs that are in this bucket.
    pub size: u64,
    /// List of koids of VMOs matching this bucket
    pub vmos: HashSet<VmoKoid>,
}

/// If `regex_str` is empty, returns the ".*" regex.
/// If `regex_str` contains an invalid regex, returns a regex that matches nothing.
/// Otherwise turns `regex_str` into the corresponding regex.
fn regex_that_matches_everything_if_string_empty(regex_str: &String) -> regex::Regex {
    let normalized_regex_str = match regex_str.as_str() {
        "" => ".*",
        str => str,
    };
    match regex::Regex::new(normalized_regex_str) {
        Ok(regex) => regex,
        Err(error) => {
            eprintln!(
                "Invalid regex (\"{}\") found while parsing buckets definition: {}",
                regex_str, error
            );
            // Regex that matches nothing.
            regex::Regex::new("$^").unwrap()
        }
    }
}

/// Split the VMOs into buckets defined in `buckets_definitions`.
/// A VMO will be attributed to the first bucket it matches.
/// A VMO will match a given bucket if the bucket's `process` regex matches a process
/// that references the VMO *and* if the bucket's `vmo` regex matches the name of the VMO.
/// Returns the list of buckets, and the list of koids of VMOs not in any bucket.
pub fn compute_buckets(
    buckets_definitions: &Vec<BucketDefinition>,
    processes: &Vec<Process>,
    koid_to_vmo: &HashMap<VmoKoid, Vmo>,
) -> (Vec<Bucket>, HashSet<VmoKoid>) {
    let mut buckets: Vec<Bucket> = vec![];
    let mut digested_vmos = HashSet::new();

    for bucket_definition in buckets_definitions {
        let vmo_regex = regex_that_matches_everything_if_string_empty(&bucket_definition.vmo);
        let process_regex =
            regex_that_matches_everything_if_string_empty(&bucket_definition.process);

        let mut bucket_size = 0;
        let mut bucket_vmos = HashSet::new();

        for process in processes {
            if !process_regex.is_match(&process.name) {
                continue;
            }
            for vmo_koid in &process.vmos {
                if digested_vmos.contains(&vmo_koid) {
                    continue;
                }
                let vmo = &koid_to_vmo[&vmo_koid];

                if !vmo_regex.is_match(&vmo.name) {
                    continue;
                }
                bucket_size += vmo.committed_bytes;
                bucket_vmos.insert(*vmo_koid);
                digested_vmos.insert(vmo_koid);
            }
        }
        buckets.push(Bucket {
            name: bucket_definition.name.clone(),
            size: bucket_size,
            vmos: bucket_vmos,
        });
    }
    (buckets, koid_to_vmo.keys().filter(|&k| !digested_vmos.contains(k)).copied().collect())
}

#[cfg(test)]
mod tests {
    use crate::bucket::{Bucket, Process, compute_buckets};
    use crate::processed::{ProcessKoid, RetainedMemory, Vmo, VmoKoid};
    use crate::raw::BucketDefinition;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn compute_buckets_test() {
        // Step 1/3:
        // Define the test data.
        #[derive(Clone)]
        struct VmoForTest {
            pub vmo_name: &'static str,
            pub vmo_koid: VmoKoid,
            pub bytes: u64,
        }
        struct ProcessForTest {
            pub process_name: &'static str,
            pub process_koid: ProcessKoid,
            pub vmos: Vec<VmoForTest>,
        }
        let vmos_defs = vec![
            VmoForTest { vmo_name: "vmo_A", vmo_koid: VmoKoid::new(0), bytes: 10 },
            VmoForTest { vmo_name: "vmo_B", vmo_koid: VmoKoid::new(1), bytes: 11 },
            VmoForTest { vmo_name: "vmo_C", vmo_koid: VmoKoid::new(2), bytes: 12 },
        ];
        let processes_defs = vec![
            ProcessForTest {
                process_name: "process1",
                process_koid: ProcessKoid::new(100),
                vmos: vec![vmos_defs[0].clone(), vmos_defs[1].clone(), vmos_defs[2].clone()],
            },
            ProcessForTest {
                process_name: "process2",
                process_koid: ProcessKoid::new(101),
                vmos: vec![vmos_defs[2].clone()],
            },
        ];

        // Step 2/3:
        // Create the structs used by `compute_buckets`.
        let buckets_definitions = vec![
            // Matches nothing (no matching process).
            BucketDefinition {
                event_code: 1000,
                name: "bucket0".to_string(),
                process: "nothing".to_string(),
                vmo: "".to_string(),
            },
            // Matches nothing (no matching VMO).
            BucketDefinition {
                event_code: 1001,
                name: "bucket1".to_string(),
                process: "".to_string(),
                vmo: "nothing".to_string(),
            },
            // Matches nothing (invalid VMO regex).
            BucketDefinition {
                event_code: 1002,
                name: "bucket2".to_string(),
                process: ".*".to_string(),
                vmo: "[".to_string(),
            },
            // Matches a subset of VMOs in a given process.
            BucketDefinition {
                event_code: 1003,
                name: "bucket3".to_string(),
                process: "process1".to_string(),
                vmo: "vmo_A|vmo_B".to_string(),
            },
            // Matches VMOs that have already been bucketted.
            BucketDefinition {
                event_code: 1004,
                name: "bucket4".to_string(),
                process: "process1".to_string(),
                vmo: "vmo_A|vmo_B".to_string(),
            },
            // Matches a VMO shared by 2 processes.
            BucketDefinition {
                event_code: 1005,
                name: "bucket5".to_string(),
                process: "".to_string(),
                vmo: "vmo_C".to_string(),
            },
        ];
        let mut processes = Vec::new();
        for process_def in processes_defs {
            let mut vmo_koids = HashSet::new();
            for vmo_def in process_def.vmos {
                vmo_koids.insert(vmo_def.vmo_koid);
            }
            processes.push(Process {
                koid: process_def.process_koid,
                name: process_def.process_name.to_string(),
                memory: RetainedMemory::default(),
                name_to_vmo_memory: HashMap::new(),
                vmos: vmo_koids,
            })
        }

        let mut koid_to_vmo = HashMap::new();

        for vmo_def in &vmos_defs {
            koid_to_vmo.insert(
                vmo_def.vmo_koid,
                Vmo {
                    koid: vmo_def.vmo_koid,
                    name: vmo_def.vmo_name.to_string(),
                    parent_koid: VmoKoid::new(999999),
                    committed_bytes: vmo_def.bytes,
                    allocated_bytes: 0,
                    populated_bytes: None,
                },
            );
        }

        // Step 3/3:
        // Run `compute_buckets`, and check the output.
        let (buckets, undigested) = compute_buckets(&buckets_definitions, &processes, &koid_to_vmo);
        pretty_assertions::assert_eq!(
            buckets[0],
            Bucket { name: "bucket0".to_string(), size: 0, vmos: HashSet::new() }
        );
        pretty_assertions::assert_eq!(
            buckets[1],
            Bucket { name: "bucket1".to_string(), size: 0, vmos: HashSet::new() }
        );
        pretty_assertions::assert_eq!(
            buckets[2],
            Bucket { name: "bucket2".to_string(), size: 0, vmos: HashSet::new() }
        );
        pretty_assertions::assert_eq!(
            buckets[3],
            Bucket {
                name: "bucket3".to_string(),
                size: vmos_defs[0].bytes + vmos_defs[1].bytes,
                vmos: HashSet::from([VmoKoid::new(0), VmoKoid::new(1)])
            }
        );
        pretty_assertions::assert_eq!(
            buckets[4],
            Bucket { name: "bucket4".to_string(), size: 0, vmos: HashSet::new() }
        );
        pretty_assertions::assert_eq!(
            buckets[5],
            Bucket {
                name: "bucket5".to_string(),
                size: vmos_defs[2].bytes,
                vmos: HashSet::from([VmoKoid::new(2)])
            }
        );
        pretty_assertions::assert_eq!(buckets.len(), 6);
        pretty_assertions::assert_eq!(undigested, Default::default());
    }
}
