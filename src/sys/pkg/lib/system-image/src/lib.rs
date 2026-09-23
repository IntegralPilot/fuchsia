// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]

mod anchored_packages;
mod cache_packages;
mod errors;
mod path_hash_mapping;
mod system_image;

pub use crate::anchored_packages::AnchoredPackages;
pub use crate::cache_packages::CachePackages;
pub use crate::errors::{
    AllowListError, CachePackagesInitError, PathHashMappingError, StaticPackagesInitError,
};
pub use crate::path_hash_mapping::{Bootfs, PathHashMapping, StaticPackages};
pub use crate::system_image::{ExecutabilityRestrictions, SystemImage};

static PKGFS_BOOT_ARG_VALUE_PREFIX: &str = "bin/pkgsvr+";

pub fn get_system_image_hash(
    system_image_hash: &str,
) -> Result<fuchsia_hash::Hash, SystemImageHashError> {
    // To drop support for the prefix the ota tests would have to branch based on the build version.
    // https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/host-target-testing/zbi/zbi.go;l=142;drc=8ef678f1bcc92eb0a87adbad0111ed73310bac19
    let hash =
        system_image_hash.strip_prefix(PKGFS_BOOT_ARG_VALUE_PREFIX).unwrap_or(system_image_hash);
    hash.parse().map_err(SystemImageHashError::BadHash)
}

#[derive(Debug, thiserror::Error)]
pub enum SystemImageHashError {
    #[error("invalid system image hash: {:?}", .0)]
    BadHash(#[source] fuchsia_hash::ParseHashError),
}

#[cfg(test)]
mod test_get_system_image_hash {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn bad_prefix() {
        assert_matches!(get_system_image_hash("bad-prefix"), Err(SystemImageHashError::BadHash(_)));
    }

    #[test]
    fn bad_hash() {
        assert_matches!(
            get_system_image_hash("bin/pkgsvr+bad-hash"),
            Err(SystemImageHashError::BadHash(_))
        );
    }

    #[test]
    fn success() {
        assert_eq!(
            get_system_image_hash(
                "bin/pkgsvr+0000000000000000000000000000000000000000000000000000000000000000"
            )
            .unwrap(),
            fuchsia_hash::Hash::from([0; 32])
        );
    }

    #[test]
    fn success_no_prefix() {
        assert_eq!(
            get_system_image_hash(
                "0000000000000000000000000000000000000000000000000000000000000000"
            )
            .unwrap(),
            fuchsia_hash::Hash::from([0; 32])
        );
    }
}
