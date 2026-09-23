// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::sync::LazyLock;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VersionInfo {
    /// Git commit hash of HEAD at build time.
    pub commit_hash: Option<String>,
    /// UTC timestamp of the HEAD commit in seconds.
    pub commit_timestamp: Option<u64>,
    /// The build version, pulled from //build/info.
    pub build_version: Option<String>,
    /// The ABI revision denotes the semantics of the Fuchsia System Interface
    /// that an application expects the platform to provide. The number
    /// has no ordering relationship (higher is not newer or older).
    /// Can be mapped to an api_level using //sdk/version_history.json.
    pub abi_revision: Option<u64>,
    /// The API level denotes a set of APIs available when building an
    /// application for a given release of the FUCHSIA IDK. Higher number
    /// means newer.
    /// Can be mapped to an abi_revision using //sdk/version_history.json.
    pub api_level: Option<u64>,
    /// Path of the actual FFX executable. This allows us to make absolutely
    /// sure commands match the daemon they're communicating with.
    pub exec_path: Option<String>,
    /// Build ID of the FFX executable.
    pub build_id: Option<String>,
}

type VersionBuf = [u8; 64];

#[used]
#[unsafe(no_mangle)]
// mach-o section specifiers require a segment and section separated by a comma.
#[cfg_attr(target_os = "macos", unsafe(link_section = ".FFX_VERSION,.ffx_version"))]
#[cfg_attr(not(target_os = "macos"), unsafe(link_section = ".ffx_version"))]
static VERSION_INFO: VersionBuf = ['v' as u8; 64];

#[used]
#[unsafe(no_mangle)]
#[cfg_attr(target_os = "macos", unsafe(link_section = ".FFX_BUILD,.ffx_build"))]
#[cfg_attr(not(target_os = "macos"), unsafe(link_section = ".ffx_build"))]
static BUILD_VERSION: VersionBuf = ['v' as u8; 64];

pub fn build_info() -> VersionInfo {
    static CACHED_BUILD_INFO: LazyLock<VersionInfo> = LazyLock::new(|| {
        // SAFETY: We're using read_volatile to prevent the compiler from optimizing
        // on the value of the statics provided in this file, since it will be
        // overridden in a later build step. The values we read are the same type as
        // the original statics.
        let version_info = &unsafe { (VERSION_INFO.as_ptr() as *const VersionBuf).read_volatile() };
        let build_version =
            &unsafe { (BUILD_VERSION.as_ptr() as *const VersionBuf).read_volatile() };

        let null_char = |b: &u8| *b == 0;
        let version_info =
            &version_info[..version_info.iter().position(null_char).unwrap_or(version_info.len())];
        let build_version = &build_version
            [..build_version.iter().position(null_char).unwrap_or(build_version.len())];
        build_info_impl(
            CString::new(version_info)
                .expect("ffx build error: invalid version string format embedded")
                .to_string_lossy()
                .trim()
                .to_string(),
            CString::new(build_version)
                .expect("ffx build error: invalid version string format embedded")
                .to_string_lossy()
                .trim()
                .to_string(),
        )
    });
    CACHED_BUILD_INFO.clone()
}

fn build_info_impl(raw_version_info: String, raw_build_version: String) -> VersionInfo {
    let split: Vec<&str> = raw_version_info.split("-").collect();
    if split.len() != 2 {
        return VersionInfo { build_version: Some(raw_build_version), ..Default::default() };
    }

    let raw_hash = split.get(0).unwrap().to_string();
    let hash_opt = if raw_hash.is_empty() { None } else { Some(raw_hash) };
    let timestamp_str = split.get(1).unwrap();
    let timestamp = timestamp_str.parse::<u64>().ok();
    let vh = version_history_data::HISTORY.get_misleading_version_for_ffx();

    return VersionInfo {
        commit_hash: hash_opt,
        commit_timestamp: timestamp,
        build_version: Some(raw_build_version.trim().to_string()),
        abi_revision: Some(vh.abi_revision.as_u64()),
        api_level: Some(
            #[allow(deprecated)]
            vh.api_level.as_u64(),
        ),
        exec_path: std::env::current_exe().map(|x| x.to_string_lossy().to_string()).ok(),
        ..Default::default()
    };
}

#[cfg(test)]
mod test {
    use super::*;

    const HASH: &str = "hashyhashhash";
    const TIMESTAMP: u64 = 12345689;
    const FAKE_BUILD_VERSION: &str = "20201118";

    #[test]
    fn test_valid_string_dirty() {
        let s = format!("{}-{}", HASH, TIMESTAMP);
        let result = build_info_impl(s, FAKE_BUILD_VERSION.to_string());

        let version = version_history_data::HISTORY.get_misleading_version_for_ffx();
        assert_eq!(
            result,
            VersionInfo {
                commit_hash: Some(HASH.to_string()),
                commit_timestamp: Some(TIMESTAMP),
                build_version: Some(FAKE_BUILD_VERSION.to_string()),
                abi_revision: Some(version.abi_revision.as_u64()),
                api_level: Some(
                    #[allow(deprecated)]
                    version.api_level.as_u64()
                ),
                exec_path: std::env::current_exe().map(|x| x.to_string_lossy().to_string()).ok(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_valid_string_clean() {
        let s = format!("{}-{}", HASH, TIMESTAMP);
        let result = build_info_impl(s, FAKE_BUILD_VERSION.to_string());

        let version = version_history_data::HISTORY.get_misleading_version_for_ffx();
        assert_eq!(
            result,
            VersionInfo {
                commit_hash: Some(HASH.to_string()),
                commit_timestamp: Some(TIMESTAMP),
                build_version: Some(FAKE_BUILD_VERSION.to_string()),
                abi_revision: Some(version.abi_revision.as_u64()),
                api_level: Some(
                    #[allow(deprecated)]
                    version.api_level.as_u64()
                ),
                exec_path: std::env::current_exe().map(|x| x.to_string_lossy().to_string()).ok(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_invalid_string_empty() {
        let result = build_info_impl(String::default(), FAKE_BUILD_VERSION.to_string());

        assert_eq!(
            result,
            VersionInfo {
                commit_hash: None,
                commit_timestamp: None,
                build_version: Some(FAKE_BUILD_VERSION.to_string()),
                abi_revision: None,
                api_level: None,
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_invalid_string_empty_with_hyphens() {
        let result = build_info_impl("--".to_string(), FAKE_BUILD_VERSION.to_string());

        assert_eq!(
            result,
            VersionInfo {
                commit_hash: None,
                commit_timestamp: None,
                build_version: Some(FAKE_BUILD_VERSION.to_string()),
                abi_revision: None,
                api_level: None,
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_invalid_string_clean_missing_hash() {
        let result = build_info_impl(format!("-{}", TIMESTAMP), FAKE_BUILD_VERSION.to_string());

        let version = version_history_data::HISTORY.get_misleading_version_for_ffx();
        assert_eq!(
            result,
            VersionInfo {
                commit_hash: None,
                commit_timestamp: Some(TIMESTAMP),
                build_version: Some(FAKE_BUILD_VERSION.to_string()),
                abi_revision: Some(version.abi_revision.as_u64()),
                #[allow(deprecated)]
                api_level: Some(version.api_level.as_u64()), //
                exec_path: std::env::current_exe().map(|x| x.to_string_lossy().to_string()).ok(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_invalid_string_clean_missing_hash_and_timestamp() {
        let result = build_info_impl("--".to_string(), FAKE_BUILD_VERSION.to_string());

        assert_eq!(
            result,
            VersionInfo {
                commit_hash: None,
                commit_timestamp: None,
                build_version: Some(FAKE_BUILD_VERSION.to_string()),
                abi_revision: None,
                api_level: None,
                ..Default::default()
            }
        );
    }
}
