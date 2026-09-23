// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use errors::{ffx_bail, ffx_error};
use fdomain_fuchsia_memory_heapdump_client as fheapdump_client;

mod pprof;
pub use crate::pprof::{LabelValue, PProfProfileBuilder};

mod realm_query;
pub use realm_query::connect_to_collector;

/// Builds a ProcessSelector value from command-line arguments.
///
/// If none of the command-line options are specified, it returns None, which
/// allows any process to be selected.
pub fn build_process_selector(
    by_name: Option<String>,
    by_koid: Option<u64>,
) -> anyhow::Result<Option<fheapdump_client::ProcessSelector>> {
    match (by_name, by_koid) {
        (None, None) => Ok(None),
        (Some(selected_name), None) => {
            Ok(Some(fheapdump_client::ProcessSelector::ByName(selected_name)))
        }
        (None, Some(selected_koid)) => {
            Ok(Some(fheapdump_client::ProcessSelector::ByKoid(selected_koid)))
        }
        _ => ffx_bail!("Please use either --by-name or --by-koid"),
    }
}

/// Converts a CollectorError into a user-friendly error.
///
/// The returned error is meant to be returned by the ffx plugin's main function.
pub fn prettify_collector_error(error: fheapdump_client::CollectorError) -> anyhow::Error {
    // Match known errors and return an FfxError (which is simply printed as-is).
    // For unknown errors, return a regular anyhow Error so that the full context with a "BUG"
    // banner is printed instead.
    match error {
        fheapdump_client::CollectorError::ProcessSelectorUnsupported => {
            ffx_error!("Unsupported process filter")
        }
        fheapdump_client::CollectorError::ProcessSelectorNoMatch => {
            ffx_error!("No process matches the requested filter")
        }
        fheapdump_client::CollectorError::ProcessSelectorAmbiguous => {
            ffx_error!("More than one process matches the requested filter")
        }
        fheapdump_client::CollectorError::LiveSnapshotFailed => {
            ffx_error!("Failed to take a snapshot of the current live allocations in the process")
        }
        fheapdump_client::CollectorError::StoredSnapshotNotFound => {
            ffx_error!("The requested snapshot ID does not exist")
        }
        other => return anyhow::anyhow!("Unrecognized CollectorError: {:?}", other),
    }
    .into()
}

/// Converts a Result potentially containing heapdump_snapshot_fdomain::Error
/// into a user-friendly error.
///
/// The returned error is meant to be returned by the ffx plugin's main function.
pub fn check_snapshot_error<T>(
    value: Result<T, heapdump_snapshot_fdomain::Error>,
) -> anyhow::Result<T> {
    if let Err(heapdump_snapshot_fdomain::Error::CollectorError(error)) = value {
        Err(prettify_collector_error(error))
    } else {
        Ok(value?)
    }
}
