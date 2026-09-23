// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::PathHashMappingError;
use buf_read_ext::BufReadExt as _;
use fuchsia_hash::Hash;
use fuchsia_pkg::PackagePath;
use sorted_vec_map::SortedVecMap;
use std::io;
use std::marker::PhantomData;
use std::str::FromStr as _;

/// PhantomData type marker to indicate a `PathHashMapping` is a "data/static_packages" file.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Static;

/// PhantomData type marker to indicate a `PathHashMapping` is a "data/bootfs_packages" file.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Bootfs;

pub type StaticPackages = PathHashMapping<Static>;

/// A `PathHashMapping` reads and writes line-oriented "{package_path}={hash}\n" files, e.g.
/// "data/static_packages".
/// Deprecated.
#[derive(Debug, PartialEq, Eq)]
pub struct PathHashMapping<T> {
    contents: SortedVecMap<PackagePath, Hash>,
    phantom: PhantomData<T>,
}

impl<T> PathHashMapping<T> {
    /// Reads the line-oriented "package-path=hash" static_packages or cache_packages file.
    /// Validates the package paths and hashes.
    pub fn deserialize(mut reader: impl io::BufRead) -> Result<Self, PathHashMappingError> {
        let mut contents = SortedVecMap::builder();
        let mut lines = reader.lending_lines();
        while let Some(line) = lines.next() {
            let line = line?;
            let i = line.rfind('=').ok_or_else(|| PathHashMappingError::EntryHasNoEqualsSign {
                entry: line.to_owned(),
            })?;
            let hash = Hash::from_str(&line[i + 1..])?;
            let path = line[..i].parse()?;
            contents.insert(path, hash);
        }
        Ok(Self { contents: contents.build(), phantom: PhantomData })
    }

    /// Iterator over the contents of the mapping.
    pub fn contents(&self) -> &SortedVecMap<PackagePath, Hash> {
        &self.contents
    }

    /// Iterator over the contents of the mapping, consuming self.
    pub fn into_contents(self) -> SortedVecMap<PackagePath, Hash> {
        self.contents
    }

    /// Iterator over the contained hashes.
    pub fn hashes(&self) -> impl Iterator<Item = &Hash> {
        self.contents.values()
    }

    /// Get the hash for a package.
    pub fn hash_for_package(&self, path: &PackagePath) -> Option<Hash> {
        self.contents.get(path).copied()
    }

    /// Shrinks the capacity of the mapping as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.contents.shrink_to_fit();
    }

    /// Write a `static_packages` or `cache_packages` file.
    pub fn serialize(&self, mut writer: impl io::Write) -> Result<(), PathHashMappingError> {
        for (k, v) in &self.contents {
            writeln!(&mut writer, "{}={}", k, v)?;
        }
        Ok(())
    }
}

impl<T> FromIterator<(PackagePath, Hash)> for PathHashMapping<T> {
    fn from_iter<I: IntoIterator<Item = (PackagePath, Hash)>>(iter: I) -> Self {
        let contents = SortedVecMap::from_iter(iter);
        Self { contents, phantom: PhantomData }
    }
}

impl<T, const N: usize> From<[(PackagePath, Hash); N]> for PathHashMapping<T> {
    fn from(entries: [(PackagePath, Hash); N]) -> Self {
        let contents = SortedVecMap::from_iter(entries);
        Self { contents, phantom: PhantomData }
    }
}

impl<T> IntoIterator for PathHashMapping<T> {
    type Item = (PackagePath, Hash);
    type IntoIter = std::vec::IntoIter<(PackagePath, Hash)>;

    fn into_iter(self) -> Self::IntoIter {
        self.contents.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a PathHashMapping<T> {
    type Item = (&'a PackagePath, &'a Hash);
    type IntoIter = sorted_vec_map::sorted_vec_map::Iter<'a, PackagePath, Hash>;

    fn into_iter(self) -> Self::IntoIter {
        (&self.contents).into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_pkg::test::random_package_path;
    use proptest::prelude::*;

    #[test]
    fn deserialize_empty_file() {
        let empty = Vec::new();
        let static_packages = StaticPackages::deserialize(empty.as_slice()).unwrap();
        assert_eq!(static_packages.hashes().count(), 0);
    }

    #[test]
    fn deserialize_valid_file_list_hashes() {
        let bytes =
            "name/variant=0000000000000000000000000000000000000000000000000000000000000000\n\
             other-name/other-variant=1111111111111111111111111111111111111111111111111111111111111111\n"
                .as_bytes();
        let static_packages = StaticPackages::deserialize(bytes).unwrap();
        assert_eq!(
            static_packages.hashes().cloned().collect::<Vec<_>>(),
            vec![
                "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap(),
                "1111111111111111111111111111111111111111111111111111111111111111".parse().unwrap()
            ]
        );
    }

    #[test]
    fn deserialze_rejects_invalid_package_path() {
        let bytes =
            "name/=0000000000000000000000000000000000000000000000000000000000000000\n".as_bytes();
        let res = StaticPackages::deserialize(bytes);
        assert_matches!(res, Err(PathHashMappingError::ParsePackagePath(_)));
    }

    #[test]
    fn deserialize_rejects_invalid_hash() {
        let bytes = "name/variant=invalid-hash\n".as_bytes();
        let res = StaticPackages::deserialize(bytes);
        assert_matches!(res, Err(PathHashMappingError::ParseHash(_)));
    }

    #[test]
    fn deserialize_rejects_missing_equals() {
        let bytes =
            "name/variant~0000000000000000000000000000000000000000000000000000000000000000\n"
                .as_bytes();
        let res = StaticPackages::deserialize(bytes);
        assert_matches!(res, Err(PathHashMappingError::EntryHasNoEqualsSign { .. }));
    }

    #[test]
    fn from_serialize() {
        let static_packages = StaticPackages::from([(
            PackagePath::from_name_and_variant("name0".parse().unwrap(), "0".parse().unwrap()),
            "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap(),
        )]);

        let mut serialized = vec![];
        static_packages.serialize(&mut serialized).unwrap();
        assert_eq!(
            serialized,
            &b"name0/0=0000000000000000000000000000000000000000000000000000000000000000\n"[..]
        );
    }

    #[test]
    fn hash_for_package_success() {
        let bytes =
            "name/variant=0000000000000000000000000000000000000000000000000000000000000000\n\
             "
            .as_bytes();
        let static_packages = StaticPackages::deserialize(bytes).unwrap();
        let res = static_packages.hash_for_package(&PackagePath::from_name_and_variant(
            "name".parse().unwrap(),
            "variant".parse().unwrap(),
        ));
        assert_eq!(
            res,
            Some(
                "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap(),
            )
        );
    }

    #[test]
    fn hash_for_missing_package_is_none() {
        let bytes =
            "name/variant=0000000000000000000000000000000000000000000000000000000000000000\n\
             "
            .as_bytes();
        let static_packages = StaticPackages::deserialize(bytes).unwrap();
        let res = static_packages.hash_for_package(&PackagePath::from_name_and_variant(
            "nope".parse().unwrap(),
            "variant".parse().unwrap(),
        ));
        assert_eq!(res, None);
    }

    prop_compose! {
        fn random_hash()(s in "[A-Fa-f0-9]{64}") -> Hash {
            s.parse().unwrap()
        }
    }

    prop_compose! {
        fn random_static_packages()
            (vec in prop::collection::vec(
                (random_package_path(), random_hash()), 0..4)
            ) -> PathHashMapping<Static> {
                StaticPackages::from_iter(vec)
            }
    }

    proptest! {
        #[test]
        fn serialize_deserialize_identity(static_packages in random_static_packages()) {
            let mut serialized = vec![];
            static_packages.serialize(&mut serialized).unwrap();
            let deserialized = StaticPackages::deserialize(serialized.as_slice()).unwrap();
            prop_assert_eq!(
                static_packages,
                deserialized
            );
        }
    }
}
