// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use std::cmp::Ordering;
use std::collections::BTreeSet;

#[derive(Debug, Default, PartialEq)]
pub struct SizeDiff {
    packages: BTreeSet<PackageDiff>,
    blobs: BTreeSet<BlobDiff>,
}

impl SizeDiff {
    pub fn package(&mut self, package: PackageDiff) {
        self.packages.insert(package);
    }

    pub fn blob(&mut self, blob: BlobDiff) {
        self.blobs.insert(blob);
    }

    pub fn print<W: std::io::Write>(&self, writer: &mut W) -> Result<()> {
        for line in self.get_print_lines() {
            writeln!(writer, "{}", line).context("Failed to write size diff output")?;
        }
        Ok(())
    }

    fn get_print_lines(&self) -> Vec<String> {
        let mut lines = vec![];

        // Calculate the total size change.
        let mut total_before = 0i64;
        let mut total_after = 0i64;
        for blob in &self.blobs {
            match blob.mode {
                BlobDiffMode::Added => total_after += blob.size as i64,
                BlobDiffMode::Removed => total_before += blob.size as i64,
                BlobDiffMode::Updated => {
                    total_before += blob.size as i64 - blob.size_delta;
                    total_after += blob.size as i64;
                }
                BlobDiffMode::Unchanged => {
                    total_before += blob.size as i64;
                    total_after += blob.size as i64;
                }
            }
        }
        lines.push("TOTAL SIZE".to_string());
        lines.push(format!("{:-<117}", ""));
        lines.push(format!("before: {: >10}", total_before));
        lines.push(format!("after:  {: >10}", total_after));
        lines.push(format!("diff:   {: >+10}", total_after - total_before));
        lines.push(String::new());
        lines.push(String::new());

        // Print packages that have been added or removed.
        let changed_packages: BTreeSet<&PackageDiff> =
            self.packages.iter().filter(|p| p.size_delta != 0).collect();
        if !changed_packages.is_empty() {
            lines.push(PackageDiff::get_header());
            lines.push(format!("{:-<117}", ""));
            for package in &changed_packages {
                lines.extend(package.get_print_lines());
            }
            lines.push(String::new());
            lines.push(String::new());
        }

        // Print blobs that have been added, removed, or updated.
        let changed_blobs: BTreeSet<&BlobDiff> =
            self.blobs.iter().filter(|b| b.size_delta != 0 || b.psize_delta != 0).collect();
        if !changed_blobs.is_empty() {
            lines.push(BlobDiff::get_header());
            lines.push(format!("{:-<137}", ""));
            for blob in &changed_blobs {
                lines.extend(blob.get_print_lines());
            }
        }

        lines
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackageDiffMode {
    Added,
    Removed,
    Updated,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PackageDiff {
    mode: PackageDiffMode,
    name: String,
    size: u64,
    size_delta: i64,
}

impl PackageDiff {
    pub fn added(name: String, size: u64) -> Self {
        Self { mode: PackageDiffMode::Added, name, size, size_delta: size as i64 }
    }

    pub fn removed(name: String, size: u64) -> Self {
        Self { mode: PackageDiffMode::Removed, name, size, size_delta: -(size as i64) }
    }

    pub fn updated(name: String, size: u64, size_delta: i64) -> Self {
        Self { mode: PackageDiffMode::Updated, name, size, size_delta }
    }

    fn get_header() -> String {
        format!("{: <32}{: >10}", "PACKAGES", "SIZE")
    }

    fn get_print_lines(&self) -> Vec<String> {
        let mut lines = vec![];
        let prefix = match self.mode {
            PackageDiffMode::Added => "++",
            PackageDiffMode::Removed => "--",
            _ => "  ",
        };
        lines.push(format!("{}{: <30}{}", prefix, self.name, self.format_size()));
        lines
    }

    fn format_size(&self) -> String {
        let size = match self.mode {
            PackageDiffMode::Added | PackageDiffMode::Removed => "".to_string(),
            PackageDiffMode::Updated => {
                format!("({})", self.size)
            }
        };
        format!("{: >+10}{: <10}", self.size_delta, size)
    }
}

impl PartialOrd for PackageDiff {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> Ord for PackageDiff {
    fn cmp(&self, other: &Self) -> Ordering {
        // We multiply the sizes by -1 in order to sort the largest sizes up at the top while
        // keeping the modes sorted normally.
        let one = (-self.size_delta, &self.name, self.mode);
        let two = (-other.size_delta, &other.name, other.mode);
        one.cmp(&two)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum BlobDiffMode {
    Added,
    Removed,
    Updated,
    Unchanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageReferenceDiff {
    mode: BlobDiffMode,
    name: String,
    path: String,
}

impl PackageReferenceDiff {
    pub fn added(name: String, path: String) -> Self {
        Self { mode: BlobDiffMode::Added, name, path }
    }

    pub fn removed(name: String, path: String) -> Self {
        Self { mode: BlobDiffMode::Removed, name, path }
    }

    pub fn updated(name: String, path: String) -> Self {
        Self { mode: BlobDiffMode::Updated, name, path }
    }
}

impl PartialOrd for PackageReferenceDiff {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> Ord for PackageReferenceDiff {
    fn cmp(&self, other: &Self) -> Ordering {
        let one = (&self.mode, &self.name, &self.path);
        let two = (&other.mode, &other.name, &other.path);
        one.cmp(&two)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct BlobDiff {
    mode: BlobDiffMode,
    hash: String,
    old_hash: Option<String>,
    size: u64,
    size_delta: i64,
    psize: u64,
    psize_delta: i64,
    share: u64,
    share_delta: i64,
    references: Vec<PackageReferenceDiff>,
}

impl BlobDiff {
    pub fn added(
        hash: String,
        size: u64,
        psize: u64,
        references: Vec<PackageReferenceDiff>,
    ) -> Self {
        Self {
            mode: BlobDiffMode::Added,
            hash,
            old_hash: None,
            size,
            size_delta: size as i64,
            psize,
            psize_delta: psize as i64,
            share: references.len() as u64,
            share_delta: references.len() as i64,
            references,
        }
    }

    pub fn removed(
        hash: String,
        size: u64,
        psize: u64,
        references: Vec<PackageReferenceDiff>,
    ) -> Self {
        Self {
            mode: BlobDiffMode::Removed,
            hash,
            old_hash: None,
            size,
            size_delta: -(size as i64),
            psize,
            psize_delta: -(psize as i64),
            share: references.len() as u64,
            share_delta: -(references.len() as i64),
            references,
        }
    }

    pub fn updated(
        hash: String,
        old_hash: String,
        size: u64,
        size_delta: i64,
        psize: u64,
        psize_delta: i64,
        references: Vec<PackageReferenceDiff>,
    ) -> Self {
        let share: u64 = references
            .iter()
            .map(|r| match r.mode {
                BlobDiffMode::Removed => 0,
                _ => 1,
            })
            .sum();
        let share_delta: i64 = references
            .iter()
            .map(|r| match r.mode {
                BlobDiffMode::Added => 1,
                BlobDiffMode::Removed => -1,
                _ => 0,
            })
            .sum();
        Self {
            mode: BlobDiffMode::Updated,
            hash,
            old_hash: Some(old_hash),
            size,
            size_delta,
            psize,
            psize_delta,
            share,
            share_delta,
            references,
        }
    }

    pub fn unchanged(
        hash: String,
        size: u64,
        psize: u64,
        references: Vec<PackageReferenceDiff>,
    ) -> Self {
        Self {
            mode: BlobDiffMode::Unchanged,
            hash,
            old_hash: None,
            size,
            size_delta: 0,
            psize,
            psize_delta: 0,
            share: references.len() as u64,
            share_delta: 0,
            references,
        }
    }

    fn get_header() -> String {
        format!(
            "{: <22} {: <30}{: <30}{: >10}{: >20}{: >20}",
            "BLOB MERKLE", "PACKAGE", "PATH", "SIZE", "PSIZE", "SHARE"
        )
    }

    fn get_print_lines(&self) -> Vec<String> {
        let mut lines = vec![];
        let mut references = self.references.clone();
        references.sort();

        let prefix = match self.mode {
            BlobDiffMode::Added => "++",
            BlobDiffMode::Removed => "--",
            _ => "  ",
        };

        let mut first_line = true;
        for reference in references {
            if first_line {
                lines.push(format!(
                    "{}{}{}{}{}{}",
                    prefix,
                    self.format_hash(),
                    Self::format_reference(&reference),
                    self.format_size(),
                    self.format_psize(),
                    self.format_share(),
                ));
            } else {
                lines.push(format!("{: <22}{}", "", Self::format_reference(&reference)));
            }
            first_line = false;
        }
        lines
    }

    fn format_hash(&self) -> String {
        if let Some(old_hash) = &self.old_hash {
            if old_hash != &self.hash {
                return format!("{} -> {: <10}", &old_hash[..6], &self.hash[..6]);
            }
        }
        format!("{: <20}", &self.hash[..6])
    }

    fn format_reference(reference: &PackageReferenceDiff) -> String {
        let m = match reference.mode {
            BlobDiffMode::Added => "+",
            BlobDiffMode::Removed => "-",
            BlobDiffMode::Updated => " ",
            BlobDiffMode::Unchanged => " ",
        };
        format!("{}{: <30}{: <30}", m, reference.name, reference.path)
    }

    fn format_size(&self) -> String {
        let size = match self.mode {
            BlobDiffMode::Added | BlobDiffMode::Removed => "".to_string(),
            BlobDiffMode::Updated => {
                format!("({})", self.size)
            }
            BlobDiffMode::Unchanged => unreachable!("unchanged blobs should not be printed"),
        };
        format!("{: >+10}{: <10}", self.size_delta, size)
    }

    fn format_psize(&self) -> String {
        let psize = match self.mode {
            BlobDiffMode::Added | BlobDiffMode::Removed => "".to_string(),
            BlobDiffMode::Updated => {
                format!("({})", self.psize)
            }
            BlobDiffMode::Unchanged => unreachable!("unchanged blobs should not be printed"),
        };
        format!("{: >+10}{: <10}", self.psize_delta, psize)
    }

    fn format_share(&self) -> String {
        let share = match self.mode {
            BlobDiffMode::Added | BlobDiffMode::Removed => "".to_string(),
            BlobDiffMode::Updated => {
                format!("({})", self.share)
            }
            BlobDiffMode::Unchanged => unreachable!("unchanged blobs should not be printed"),
        };
        format!("{: >+10}{: <10}", self.share_delta, share)
    }
}

impl PartialOrd for BlobDiff {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BlobDiff {
    fn cmp(&self, other: &Self) -> Ordering {
        // We multiply the sizes by -1 in order to sort the largest sizes up at the top while
        // keeping the modes/hashes sorted normally.
        let one = (-self.size_delta, -self.psize_delta, self.mode, &self.hash);
        let two = (-other.size_delta, -other.psize_delta, other.mode, &other.hash);
        one.cmp(&two)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn package_cmp() {
        let add_pkg_one = PackageDiff::added("pkg-one".to_string(), 100);
        // Sorted after pkg-one due to size.
        let add_pkg_two = PackageDiff::added("pkg-two".to_string(), 10);
        // Sorted after pkg-one due to package name.
        let add_pkg_three = PackageDiff::added("pkg-three".to_string(), 100);
        let remove_pkg_one = PackageDiff::removed("pkg-one".to_string(), 100);
        let remove_pkg_two = PackageDiff::removed("pkg-two".to_string(), 10);

        assert!(add_pkg_one < add_pkg_two);
        assert!(add_pkg_one < add_pkg_three);
        assert!(add_pkg_one < remove_pkg_one);
        assert!(remove_pkg_two < remove_pkg_one);
    }

    #[test]
    fn package_reference_cmp() {
        let add_pkg_one =
            PackageReferenceDiff::added("pkg-one".to_string(), "lib/liba".to_string());
        // Sorted after liba due to path.
        let add_pkg_one_libb =
            PackageReferenceDiff::added("pkg-one".to_string(), "lib/libb".to_string());
        // Sorted after pkg-one due to package name.
        let add_pkg_two =
            PackageReferenceDiff::added("pkg-two".to_string(), "lib/liba".to_string());
        // Sorted after pkg-one due to mode.
        let remove_pkg_one =
            PackageReferenceDiff::removed("pkg-one".to_string(), "lib/liba".to_string());
        // Sorted after remove pkg-two due to name.
        let remove_pkg_two =
            PackageReferenceDiff::removed("pkg-two".to_string(), "lib/liba".to_string());

        assert!(add_pkg_one < add_pkg_two);
        assert!(add_pkg_one < add_pkg_one_libb);
        assert!(add_pkg_one < remove_pkg_one);
        assert!(remove_pkg_one < remove_pkg_two);
    }

    #[test]
    fn blob_cmp() {
        let add_10 = BlobDiff::added(
            "abcdef".to_string(),
            10,
            10,
            vec![PackageReferenceDiff::added("pkg-one".to_string(), "lib/lib1".to_string())],
        );
        let add_100 = BlobDiff::added(
            "abcdef".to_string(),
            100,
            100,
            vec![PackageReferenceDiff::added("pkg-one".to_string(), "lib/lib1".to_string())],
        );
        let add_p100 = BlobDiff::updated(
            "abcdef".to_string(),
            "fedcba".to_string(),
            100,
            0,
            100,
            10,
            vec![
                PackageReferenceDiff::added("pkg-one".to_string(), "lib/lib1".to_string()),
                PackageReferenceDiff::updated("pkg-two".to_string(), "lib/lib1".to_string()),
            ],
        );
        let remove_10 = BlobDiff::removed(
            "abcdef".to_string(),
            10,
            10,
            vec![PackageReferenceDiff::removed("pkg-one".to_string(), "lib/lib2".to_string())],
        );
        let remove_100 = BlobDiff::removed(
            "abcdef".to_string(),
            100,
            100,
            vec![PackageReferenceDiff::removed("pkg-one".to_string(), "lib/lib2".to_string())],
        );
        let add_abcdef = BlobDiff::added(
            "abcdef".to_string(),
            10,
            10,
            vec![PackageReferenceDiff::added("pkg-one".to_string(), "lib/lib2".to_string())],
        );
        let add_bbcdef = BlobDiff::added(
            "bbcdef".to_string(),
            10,
            10,
            vec![PackageReferenceDiff::added("pkg-one".to_string(), "lib/lib2".to_string())],
        );

        assert!(add_10 < remove_10);
        assert!(add_10 < remove_100);
        assert!(add_100 < add_10);
        assert!(add_100 < add_p100);
        assert!(remove_10 < remove_100);
        assert!(add_abcdef < add_bbcdef);
    }

    #[test]
    fn print_no_diff() {
        let mut diff = SizeDiff::default();
        diff.blob(BlobDiff::unchanged(
            "1234567890".to_string(),
            10,
            10,
            vec![PackageReferenceDiff::updated("pkg-one".to_string(), "lib/lib1".to_string())],
        ));
        let lines = diff.get_print_lines();
        let expected_lines: Vec<String> = vec![
            "TOTAL SIZE",
            "---------------------------------------------------------------------------------------------------------------------",
            "before:         10",
            "after:          10",
            "diff:           +0",
            "",
            "",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(expected_lines, lines);
    }

    #[test]
    fn print() {
        let mut diff = SizeDiff::default();
        diff.blob(BlobDiff::added(
            "1234567890".to_string(),
            10,
            10,
            vec![
                PackageReferenceDiff::updated("pkg-three".to_string(), "lib/lib2".to_string()),
                PackageReferenceDiff::updated("pkg-three".to_string(), "lib/lib1".to_string()),
                PackageReferenceDiff::updated("pkg-two".to_string(), "lib/lib1".to_string()),
                PackageReferenceDiff::added("pkg-one".to_string(), "lib/lib1".to_string()),
            ],
        ));
        diff.blob(BlobDiff::added(
            "2345678901".to_string(),
            100,
            100,
            vec![PackageReferenceDiff::added("pkg-one".to_string(), "lib/lib1.5".to_string())],
        ));
        diff.blob(BlobDiff::removed(
            "0987654321".to_string(),
            30,
            15,
            vec![
                PackageReferenceDiff::removed("pkg-one".to_string(), "lib/lib2".to_string()),
                PackageReferenceDiff::removed("pkg-two".to_string(), "lib/lib2".to_string()),
            ],
        ));
        // Size increased by 30.
        diff.blob(BlobDiff::updated(
            "123_merkle".to_string(),
            "321_merkle".to_string(),
            60,
            30,
            30,
            10,
            vec![
                PackageReferenceDiff::updated("pkg-one".to_string(), "lib/lib3".to_string()),
                PackageReferenceDiff::updated("pkg-two".to_string(), "lib/lib3".to_string()),
                PackageReferenceDiff::updated("pkg-three".to_string(), "lib/lib3".to_string()),
            ],
        ));
        // Share reduced by 1.
        diff.blob(BlobDiff::updated(
            "456_merkle".to_string(),
            "654_merkle".to_string(),
            10,
            0,
            20,
            10,
            vec![
                PackageReferenceDiff::updated("pkg-one".to_string(), "lib/lib4".to_string()),
                PackageReferenceDiff::removed("pkg-two".to_string(), "lib/lib4".to_string()),
            ],
        ));
        // Size decreased by 30.
        diff.blob(BlobDiff::updated(
            "789_merkle".to_string(),
            "987_merkle".to_string(),
            30,
            -30,
            30,
            -10,
            vec![
                PackageReferenceDiff::updated("pkg-one".to_string(), "lib/lib5".to_string()),
                PackageReferenceDiff::updated("pkg-two".to_string(), "lib/lib5".to_string()),
                PackageReferenceDiff::updated("pkg-three".to_string(), "lib/lib5".to_string()),
            ],
        ));

        diff.package(PackageDiff::updated("pkg-one".to_string(), 100, 10));
        diff.package(PackageDiff::added("pkg-two".to_string(), 50));
        diff.package(PackageDiff::removed("pkg-three".to_string(), 60));
        let lines = diff.get_print_lines();
        let expected_lines: Vec<String> = vec![
            "TOTAL SIZE",
            "---------------------------------------------------------------------------------------------------------------------",
            "before:        130",
            "after:         210",
            "diff:          +80",
            "",
            "",
            "PACKAGES                              SIZE",
            "---------------------------------------------------------------------------------------------------------------------",
            "++pkg-two                              +50          ",
            "  pkg-one                              +10(100)     ",
            "--pkg-three                            -60          ",
            "",
            "",
            "BLOB MERKLE            PACKAGE                       PATH                                SIZE               PSIZE               SHARE",
            "-----------------------------------------------------------------------------------------------------------------------------------------",
            "++234567              +pkg-one                       lib/lib1.5                          +100                +100                  +1          ",
            "  321_me -> 123_me     pkg-one                       lib/lib3                             +30(60)             +10(30)              +0(3)       ",
            "                       pkg-three                     lib/lib3                      ",
            "                       pkg-two                       lib/lib3                      ",
            "++123456              +pkg-one                       lib/lib1                             +10                 +10                  +4          ",
            "                       pkg-three                     lib/lib1                      ",
            "                       pkg-three                     lib/lib2                      ",
            "                       pkg-two                       lib/lib1                      ",
            "  654_me -> 456_me    -pkg-two                       lib/lib4                              +0(10)             +10(20)              -1(1)       ",
            "                       pkg-one                       lib/lib4                      ",
            "  987_me -> 789_me     pkg-one                       lib/lib5                             -30(30)             -10(30)              +0(3)       ",
            "                       pkg-three                     lib/lib5                      ",
            "                       pkg-two                       lib/lib5                      ",
            "--098765              -pkg-one                       lib/lib2                             -30                 -15                  -2          ",
            "                      -pkg-two                       lib/lib2                      ",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(expected_lines, lines);

        let mut output = Vec::new();
        diff.print(&mut output).unwrap();
        let expected_output = expected_lines.join("\n") + "\n";
        assert_eq!(String::from_utf8(output).unwrap(), expected_output);
    }
}
