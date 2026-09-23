// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_hash::Hash;
use serde::Serialize;
use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, Serialize, Eq, PartialEq, Default)]
pub struct PackageSizeInfo {
    pub name: String,
    /// Space used by this package in blobfs if each blob is counted fully.
    pub used_space_in_blobfs: u64,
    /// Size of the package in blobfs if each blob is divided equally among all the packages that reference it.
    pub proportional_size: u64,
    /// Blobs in this package and information about their size.
    pub blobs: Vec<PackageBlobSizeInfo>,
}

#[derive(Debug, Serialize, Eq, PartialEq, Clone)]
pub struct PackageBlobSizeInfo {
    pub merkle: Hash,
    pub path_in_package: String,
    /// Space used by this blob in blobfs
    pub used_space_in_blobfs: u64,
    /// Number of packages that contain this blob.
    pub share_count: u64,
    /// Count of all occurrences of the blob within and across all packages.
    pub absolute_share_count: u64,
}

#[allow(dead_code)] // TODO(https://fxbug.dev/421409514)
pub struct PackageSizeInfos<'a>(pub &'a Vec<PackageSizeInfo>);

/// Implementing Display to show a easy to comprehend structured output of PackageSizeInfos
impl fmt::Display for PackageSizeInfos<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut duplicate_found = false;
        let package_column_width: usize = 50;
        let path_column_width: usize = 50;
        let size_column_width: usize = 10;
        let proportional_size_column_width: usize = 20;
        let share_column_width: usize = 5;
        let merkle_column_width: usize = 15;
        writeln!(
            f,
            "{0: <5$}  {1: >6$}  {2: >7$}  {3: >8$} {4: >9$}",
            "Package",
            "Merkle",
            "Size",
            "Proportional Size",
            "Share",
            package_column_width,
            merkle_column_width,
            size_column_width,
            proportional_size_column_width,
            share_column_width,
        )
        .unwrap();
        let mut refs = self.0.iter().collect::<Vec<&_>>();
        // Sorted names are easier to diff.
        refs.sort_by_key(|a| a.name.as_str());
        refs.iter().for_each(|p| {
            writeln!(
                f,
                "{0: <5$}  {1: >6$}  {2: >7$}  {3: >8$} {4: >9$}",
                p.name,
                "",
                p.used_space_in_blobfs,
                p.proportional_size,
                "",
                package_column_width,
                merkle_column_width,
                size_column_width,
                proportional_size_column_width,
                share_column_width,
            )
            .unwrap();
            let mut merkle_set: HashSet<String> = HashSet::new();
            let mut refs = p.blobs.iter().collect::<Vec<&_>>();
            // Sorted paths are easier to diff.
            refs.sort_by_key(|a| a.path_in_package.as_str());
            refs.iter().for_each(|b| {
                writeln!(
                    f,
                    "{0: <5$}  {1: >6$}  {2: >7$}  {3: >8$} {4: >9$}",
                    process_path_str(
                        b.path_in_package.to_string(),
                        path_column_width,
                        merkle_set.contains(&b.merkle.to_string())
                    ),
                    &b.merkle.to_string()[..10],
                    b.used_space_in_blobfs,
                    b.used_space_in_blobfs / b.share_count,
                    b.share_count,
                    path_column_width,
                    merkle_column_width,
                    size_column_width,
                    proportional_size_column_width,
                    share_column_width,
                )
                .unwrap();
                duplicate_found |= merkle_set.contains(&b.merkle.to_string());
                merkle_set.insert(b.merkle.to_string());
            });
        });
        if duplicate_found {
            writeln!(f,"\n* indicates that this blob is a duplicate within this package and it therefore does not contribute to the overall package size").unwrap();
        }

        Ok(())
    }
}

#[allow(dead_code)] // TODO(https://fxbug.dev/421409514)
fn process_path_str(input_path: String, column_width: usize, is_duplicate: bool) -> String {
    let path_str = if is_duplicate { format!("{input_path}*") } else { input_path };
    if path_str.len() > (column_width - 3) {
        let mut v: Vec<String> =
            textwrap::wrap(&path_str, column_width - 5).iter().map(|x| x.to_string()).collect();
        let first_line = format!("   {0: <1$}", v[0], column_width - 3);
        let last_line = format!("{0: <1$}", v[v.len() - 1], column_width - 5);
        let len = v.len();
        v[0] = first_line;
        v[len - 1] = last_line;
        v.join("\n     ")
    } else {
        format!("   {0: <1$}", path_str, column_width - 3)
    }
}

pub fn wrap_text(indent: usize, length: usize, path: &String) -> String {
    if path.len() <= length {
        return path.clone();
    }

    let mut wrapped = String::new();

    // Add the first line.
    let first_length = length - 3;
    wrapped += &format!("{}...\n", &path[..first_length]);

    // Add the following lines.
    let following_length = length - 6;
    let following_chars = path[first_length..].chars().collect::<Vec<char>>();
    let following = following_chars
        .chunks(following_length)
        .map(|c| c.iter().collect::<String>())
        .map(|s| format!("{:indent$}   {}", "", s, indent = indent))
        .collect::<Vec<String>>();
    wrapped += &following.join("...\n");

    // Add any remaining spaces to line back up to length.
    let remaining = length + indent - following.last().map(|s| s.len()).unwrap_or(0);
    wrapped += &format!("{:r$}", "", r = remaining);
    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_text() {
        let path = "abcdefghijklmnopqrstuvwxyz".to_string();
        let expected_path = r"abcdefg...
      hijk...
      lmno...
      pqrs...
      tuvw...
      xyz    ";
        assert_eq!(expected_path, wrap_text(3, 10, &path));
    }
}
