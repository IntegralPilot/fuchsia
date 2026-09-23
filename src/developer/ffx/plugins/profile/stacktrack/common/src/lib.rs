// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use errors::{ffx_bail, ffx_error};
use fdomain_fuchsia_memory_stacktrack_client as fstacktrack_client;

mod realm_query;
pub use realm_query::connect_to_collector;

mod resolved_snapshot;
pub use resolved_snapshot::{
    CallFrame, ExecutableRegion, ResolvedLocation, ResolvedSnapshot, StackTraceGroup,
};

/// Builds a ProcessSelector value from command-line arguments.
///
/// If none of the command-line options are specified, it returns None, which
/// allows any process to be selected.
pub fn build_process_selector(
    by_name: Option<String>,
    by_koid: Option<u64>,
) -> anyhow::Result<Option<fstacktrack_client::ProcessSelector>> {
    match (by_name, by_koid) {
        (None, None) => Ok(None),
        (Some(selected_name), None) => {
            Ok(Some(fstacktrack_client::ProcessSelector::ByName(selected_name)))
        }
        (None, Some(selected_koid)) => {
            Ok(Some(fstacktrack_client::ProcessSelector::ByKoid(selected_koid)))
        }
        _ => ffx_bail!("Please use either --by-name or --by-koid"),
    }
}

/// Converts a CollectorError into a user-friendly error.
///
/// The returned error is meant to be returned by the ffx plugin's main function.
pub fn prettify_collector_error(error: fstacktrack_client::CollectorError) -> anyhow::Error {
    // Match known errors and return an FfxError (which is simply printed as-is).
    // For unknown errors, return a regular anyhow Error so that the full context with a "BUG"
    // banner is printed instead.
    match error {
        fstacktrack_client::CollectorError::ProcessSelectorUnsupported => {
            ffx_error!("Unsupported process filter")
        }
        fstacktrack_client::CollectorError::ProcessSelectorNoMatch => {
            ffx_error!("No process matches the requested filter")
        }
        fstacktrack_client::CollectorError::ProcessSelectorAmbiguous => {
            ffx_error!("More than one process matches the requested filter")
        }
        fstacktrack_client::CollectorError::GetStackTracesFailed => {
            ffx_error!("Failed to get stack traces")
        }
        other => return anyhow::anyhow!("Unrecognized CollectorError: {:?}", other),
    }
    .into()
}
