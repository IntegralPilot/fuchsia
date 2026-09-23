// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use assembly_util::{read_config, write_json_file};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::Path;

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryBucket {
    /// When 'memory_monitor' writes the measurement of this bucket data to Inspect,
    /// this is the user-facing name that will be used.
    name: String,
    /// The regex to match process names against to be included in this bucket.
    process: String,
    /// The regex to match VMO names against to be included in this bucket.
    vmo: String,
    /// Regex to match VMOs or processes based on their principals' names.
    /// Note: This field is only supported by memory_monitor2.
    // Skip serializing when missing, for backwards compatibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    principal: Option<String>,
    /// The event code used by Cobalt to record the bucket measurement.
    event_code: u64,
    /// The order in which the bucket regex will be applied, in relation to all
    /// other buckets. A lower order number indicates that it will be applied
    /// earlier.
    #[serde(skip_serializing)]
    #[serde(default = "default_order")]
    order: u64,
}

/// Sets the default order to the maximum u64, to indicate that this bucket should
/// appear last in the order if left unspecified.
fn default_order() -> u64 {
    u64::MAX
}

impl Ord for MemoryBucket {
    /// When comparing two buckets, we compare their 'order' fields. If they are
    /// the same, then the one that specifies a VMO regex is sorted ahead of the
    /// other.
    fn cmp(&self, other: &Self) -> Ordering {
        if self.order != other.order {
            return self.order.cmp(&other.order);
        }
        return self.vmo.is_empty().cmp(&other.vmo.is_empty());
    }
}

impl PartialOrd for MemoryBucket {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A builder that produces a single memory buckets config file by merging
/// multiple memory buckets inputs from the platform, board, or product.
#[derive(Debug, Default, Serialize)]
pub struct MemoryBuckets {
    /// The collection of all buckets.
    buckets: Vec<MemoryBucket>,
}

impl MemoryBuckets {
    /// Add all the buckets from all the files in `buckets`.
    pub fn add_buckets(&mut self, buckets: &Vec<Utf8PathBuf>) -> Result<()> {
        for bucket_path in buckets {
            let mut buckets_values: Vec<MemoryBucket> = read_config(bucket_path)?;
            self.buckets.append(&mut buckets_values);
        }
        Ok(())
    }

    /// Write the final merged config into `output`.
    pub fn write(&mut self, output: impl AsRef<Path>) -> Result<()> {
        self.buckets.sort();
        write_json_file(output, &self.buckets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs::File;
    use tempfile::NamedTempFile;

    #[test]
    fn test_add_buckets() {
        // Write the test inputs.
        let input1 = json!([
            {
                "name": "bucket name 1",
                "process": "",
                "vmo": "",
                "event_code": 1,
                "order": 1,
            },
        ]);
        let input2 = json!([
            {
                "name": "bucket name 2",
                "process": "",
                "vmo": "",
                "event_code": 2,
                "order": 2,
            },
        ]);
        let input3 = json!([
            {
                "name": "bucket name 3",
                "process": "",
                "vmo": "",
                "event_code": 3,
                "order": 3,
            },
        ]);
        let buckets_input1 = NamedTempFile::new().unwrap();
        let buckets_input2 = NamedTempFile::new().unwrap();
        let buckets_input3 = NamedTempFile::new().unwrap();
        serde_json::ser::to_writer(&buckets_input1, &input1).unwrap();
        serde_json::ser::to_writer(&buckets_input2, &input2).unwrap();
        serde_json::ser::to_writer(&buckets_input3, &input3).unwrap();
        let bucket_group1 = vec![
            Utf8PathBuf::from_path_buf(buckets_input1.path().to_path_buf()).unwrap(),
            Utf8PathBuf::from_path_buf(buckets_input2.path().to_path_buf()).unwrap(),
        ];
        let bucket_group2 =
            vec![Utf8PathBuf::from_path_buf(buckets_input3.path().to_path_buf()).unwrap()];

        // Run the builder.
        let mut buckets = MemoryBuckets::default();
        buckets.add_buckets(&bucket_group1).unwrap();
        buckets.add_buckets(&bucket_group2).unwrap();
        let buckets_output = NamedTempFile::new().unwrap();
        buckets.write(&buckets_output.path()).unwrap();

        // Assert the result.
        let buckets_output = File::open(buckets_output).unwrap();
        let output: serde_json::Value = serde_json::from_reader(&buckets_output).unwrap();
        let expected_output = json!([
            {
                "name": "bucket name 1",
                "process": "",
                "vmo": "",
                "event_code": 1
            },
            {
                "name": "bucket name 2",
                "process": "",
                "vmo": "",
                "event_code": 2
            },
            {
                "name": "bucket name 3",
                "process": "",
                "vmo": "",
                "event_code": 3
            },
        ]);
        assert_eq!(output, expected_output);
    }

    #[test]
    fn test_add_buckets_reorders_buckets_based_on_order() {
        // Write the test inputs.
        let input1 = json!([
            {
                "name": "bucket name 1",
                "process": "bucket_1.cm",
                "vmo": "",
                "event_code": 1
            },
        ]);
        let input2 = json!([
            {
                "name": "bucket name 2",
                "process": "",
                "vmo": "bucket_2_vmos",
                "event_code": 2,
                "order": 100
            },
        ]);
        let input3 = json!([
            {
                "name": "bucket name 3",
                "process": "",
                "vmo": "bucket_3_vmos",
                "event_code": 3,
                "order": 50
            },
        ]);
        let input4 = json!([
            {
                "name": "bucket name 4",
                "process": "",
                "vmo": "bucket_4_vmos",
                "event_code": 4
            },
        ]);
        let buckets_input1 = NamedTempFile::new().unwrap();
        let buckets_input2 = NamedTempFile::new().unwrap();
        let buckets_input3 = NamedTempFile::new().unwrap();
        let buckets_input4 = NamedTempFile::new().unwrap();
        serde_json::ser::to_writer(&buckets_input1, &input1).unwrap();
        serde_json::ser::to_writer(&buckets_input2, &input2).unwrap();
        serde_json::ser::to_writer(&buckets_input3, &input3).unwrap();
        serde_json::ser::to_writer(&buckets_input4, &input4).unwrap();
        let bucket_group1 = vec![
            Utf8PathBuf::from_path_buf(buckets_input1.path().to_path_buf()).unwrap(),
            Utf8PathBuf::from_path_buf(buckets_input2.path().to_path_buf()).unwrap(),
        ];
        let bucket_group2 = vec![
            Utf8PathBuf::from_path_buf(buckets_input3.path().to_path_buf()).unwrap(),
            Utf8PathBuf::from_path_buf(buckets_input4.path().to_path_buf()).unwrap(),
        ];

        // Run the builder.
        let mut buckets = MemoryBuckets::default();
        buckets.add_buckets(&bucket_group1).unwrap();
        buckets.add_buckets(&bucket_group2).unwrap();
        let buckets_output = NamedTempFile::new().unwrap();
        buckets.write(&buckets_output.path()).unwrap();

        // Assert the result, the expected order is [3, 2, 4, 1].
        // Bucket 3 should be first, as it has a specified order 50.
        // Bucket 2 should be second, as it has a specified order 100.
        // Bucket 4 should be third, as it has no specified order and is a VMO-only bucket.
        // Bucket 1 should be third, as it has no specified order.
        let buckets_output = File::open(buckets_output).unwrap();
        let output: serde_json::Value = serde_json::from_reader(&buckets_output).unwrap();
        let expected_output = json!([
            {
                "name": "bucket name 3",
                "process": "",
                "vmo": "bucket_3_vmos",
                "event_code": 3
            },
            {
                "name": "bucket name 2",
                "process": "",
                "vmo": "bucket_2_vmos",
                "event_code": 2
            },
            {
                "name": "bucket name 4",
                "process": "",
                "vmo": "bucket_4_vmos",
                "event_code": 4
            },
            {
                "name": "bucket name 1",
                "process": "bucket_1.cm",
                "vmo": "",
                "event_code": 1
            },
        ]);
        assert_eq!(output, expected_output);
    }
}
