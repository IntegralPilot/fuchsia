// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use chrono::{Offset, TimeZone};
use ffx_build_version::VersionInfo;
use fho::{FfxContext, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::io::Write;

const UNKNOWN_BUILD_HASH: &str = "(unknown)";

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, JsonSchema)]
pub struct Versions {
    pub tool_version: VersionInfo,
    /// Preserved for backwards compatibility with external JSON schema consumers.
    pub daemon_version: Option<VersionInfo>,
}

pub fn format_version_info<O: Offset + Display>(
    header: &str,
    info: &VersionInfo,
    verbose: bool,
    tz: &impl TimeZone<Offset = O>,
) -> String {
    let build_version = info.build_version.as_deref().unwrap_or("(unknown build version)");
    if !verbose {
        return build_version.to_owned();
    }

    // Convert the ABI revision back to hex string so that it matches
    // the format in //sdk/version_history.json.
    let abi_revision = match info.abi_revision {
        Some(abi) => format!("{:#X}", abi),
        None => String::from("(unknown ABI revision)"),
    };
    let api_level = match info.api_level {
        Some(api) => format!("{}", api),
        None => String::from("(unknown API level)"),
    };

    let hash = info.commit_hash.as_deref().unwrap_or(UNKNOWN_BUILD_HASH);
    let timestamp_str = match info.commit_timestamp {
        Some(t) => tz.timestamp_opt(t as i64, 0).unwrap().to_rfc2822(),
        None => String::from("(unknown commit time)"),
    };

    return format!(
        "\
{header}:
  abi-revision: {abi_revision}
  api-level: {api_level}
  build-version: {build_version}
  integration-commit-hash: {hash}
  integration-commit-time: {timestamp_str}",
    );
}

pub fn format_versions<W: Write, O: Offset + Display>(
    version_info: &Versions,
    verbose: bool,
    w: &mut W,
    tz: impl TimeZone<Offset = O>,
) -> Result<()> {
    writeln!(w, "{}", format_version_info("ffx", &version_info.tool_version, verbose, &tz))
        .bug()?;

    if let Some(daemon_version_info) = version_info.daemon_version.as_ref() {
        writeln!(w, "\n{}", format_version_info("daemon", daemon_version_info, verbose, &tz))
            .bug()?;
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use chrono::Utc;

    pub const FAKE_FRONTEND_HASH: &str = "fake frontend fake";
    pub const FAKE_FRONTEND_BUILD_VERSION: &str = "fake frontend build";
    pub const TIMESTAMP: u64 = 1604080617;
    pub const FAKE_ABI_REVISION: u64 = 17063755220075245312;
    pub const FAKE_API_LEVEL: u64 = 7;

    pub fn frontend_info() -> VersionInfo {
        VersionInfo {
            commit_hash: Some(FAKE_FRONTEND_HASH.to_string()),
            commit_timestamp: Some(TIMESTAMP),
            build_version: Some(FAKE_FRONTEND_BUILD_VERSION.to_string()),
            abi_revision: Some(FAKE_ABI_REVISION),
            api_level: Some(FAKE_API_LEVEL),
            ..Default::default()
        }
    }

    fn run_version_test(
        tool_version: VersionInfo,
        daemon_version: Option<VersionInfo>,
        verbose: bool,
    ) -> String {
        let mut writer = Vec::new();
        let versions = Versions { tool_version, daemon_version };
        let result = format_versions(&versions, verbose, &mut writer, Utc);
        assert!(result.is_ok());
        String::from_utf8(writer).unwrap()
    }

    #[test]
    fn test_success() {
        let output = run_version_test(frontend_info(), None, false);
        assert_eq!(output, format!("{}\n", FAKE_FRONTEND_BUILD_VERSION));
    }

    #[test]
    fn test_empty_version_info_not_verbose() {
        let output = run_version_test(VersionInfo::default(), None, false);
        assert_eq!(output, "(unknown build version)\n");
    }
}
