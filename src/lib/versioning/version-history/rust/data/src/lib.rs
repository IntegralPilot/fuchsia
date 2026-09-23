// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use version_history::*;

const VERSIONS: &[Version] = &version_history_macro::declare_version_history!();

/// Global [VersionHistory] instance generated at build-time from the contents
/// of `//sdk/version_history.json`.
pub const HISTORY: VersionHistory = VersionHistory::new(VERSIONS);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_history_works() {
        assert_eq!(
            VERSIONS[0],
            Version {
                api_level: 4.into(),
                abi_revision: 0x601665C5B1A89C7F.into(),
                status: Status::Unsupported,
            }
        )
    }

    #[test]
    fn test_example_supported_abi_revision_is_supported() {
        let example_version = HISTORY.get_example_supported_version_for_tests();

        assert_eq!(
            HISTORY.check_api_level_for_build(example_version.api_level),
            Ok(example_version.abi_revision)
        );

        HISTORY
            .check_abi_revision_for_runtime(example_version.abi_revision)
            .expect("you said it was supported!");
    }
}
