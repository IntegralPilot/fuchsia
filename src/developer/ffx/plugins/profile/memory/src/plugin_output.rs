// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::digest;
use attribution_processing::summary::ComponentSummaryProfileResult;
use digest::processed;
use serde::Serialize;

/// Contains the memory usage of processes, and the time at which the
/// data was captured.
#[derive(Debug, PartialEq, Serialize)]
pub struct ProcessesMemoryUsage {
    /// The list of process data.
    pub process_data: Vec<processed::Process>,
    /// The time at which the data was captured.
    pub capture_time: u64,
}

/// The plugin can output one of these based on the options:
/// * A complete digest of the memory usage.
/// * A digest of the memory usage of a subset of the processes running on the targeted device.
#[derive(Debug, PartialEq, Serialize)]
pub enum ProfileMemoryOutput {
    CompleteDigest(Box<processed::Digest>),
    ProcessDigest(Box<ProcessesMemoryUsage>),
    ComponentDigest(Box<ComponentSummaryProfileResult>),
}

/// Returns a ProfileMemoryOutput that only contains information related to the process identified by `koid`.
pub fn filter_digest_by_process(
    digest: processed::Digest,
    raw_koids: &[processed::ProcessKoid],
    names: &[String],
) -> ProfileMemoryOutput {
    let mut vec = Vec::new();
    for process in digest.processes {
        if raw_koids.contains(&process.koid) || names.contains(&process.name) {
            vec.push(process);
        }
    }
    return ProfileMemoryOutput::ProcessDigest(Box::new(ProcessesMemoryUsage {
        process_data: vec,
        capture_time: digest.time,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_process(koid: processed::ProcessKoid, name: &str) -> processed::Process {
        processed::Process {
            koid,
            name: name.to_string(),
            memory: Default::default(),
            name_to_vmo_memory: Default::default(),
            vmos: Default::default(),
        }
    }

    fn mock_digest() -> processed::Digest {
        processed::Digest {
            time: 100,
            total_committed_bytes_in_vmos: 1000,
            kernel: Default::default(),
            processes: vec![
                mock_process(processed::ProcessKoid::new(1), "process1"),
                mock_process(processed::ProcessKoid::new(2), "process2"),
                mock_process(processed::ProcessKoid::new(3), "process3"),
            ],
            vmos: vec![],
            buckets: None,
            total_undigested: None,
            kmem_stats_compression: Default::default(),
        }
    }

    #[test]
    fn filter_by_process_koid() {
        let digest = mock_digest();
        let capture_time = digest.time;
        let observed = filter_digest_by_process(digest, &[processed::ProcessKoid::new(1)], &[]);
        let expected = ProfileMemoryOutput::ProcessDigest(Box::new(ProcessesMemoryUsage {
            process_data: vec![mock_process(processed::ProcessKoid::new(1), "process1")],
            capture_time,
        }));
        assert_eq!(observed, expected);
    }

    #[test]
    fn filter_by_process_name() {
        let digest = mock_digest();
        let capture_time = digest.time;
        let observed = filter_digest_by_process(digest, &[], &[String::from("process1")]);
        let expected = ProfileMemoryOutput::ProcessDigest(Box::new(ProcessesMemoryUsage {
            process_data: vec![mock_process(processed::ProcessKoid::new(1), "process1")],
            capture_time,
        }));
        assert_eq!(observed, expected);
    }

    #[test]
    fn filter_by_process_koid_and_name() {
        let digest = mock_digest();
        let capture_time = digest.time;
        let observed = filter_digest_by_process(
            digest,
            &[processed::ProcessKoid::new(2)],
            &[String::from("process1")],
        );
        let expected = ProfileMemoryOutput::ProcessDigest(Box::new(ProcessesMemoryUsage {
            process_data: vec![
                mock_process(processed::ProcessKoid::new(1), "process1"),
                mock_process(processed::ProcessKoid::new(2), "process2"),
            ],
            capture_time,
        }));
        assert_eq!(observed, expected);
    }
}
