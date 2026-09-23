// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use ffx_scrutiny_verify_args::ota::Command;
use scrutiny_collection::ota_verification::OtaVerificationReport;
use scrutiny_collector::ota_verification::OtaVerificationCollector;
use scrutiny_utils::http_artifact::{HttpArtifactReader, RealHttpFetcher};
use std::collections::HashSet;
use std::path::PathBuf;

pub async fn verify(cmd: &Command) -> Result<HashSet<PathBuf>> {
    let update_package = cmd.update_package.clone();
    let blob_server_url = cmd.blob_server_url.clone();
    let delivery_blob_type = cmd.delivery_blob_type;

    // We use `unblock` to safely execute the synchronous verification logic
    // on a background thread pool, preventing it from blocking the main
    // ffx async executor.
    let report = fuchsia_async::unblock(move || -> Result<OtaVerificationReport> {
        let fetcher = RealHttpFetcher::new();
        let mut artifact_reader: Box<dyn scrutiny_utils::artifact::ArtifactReader> =
            Box::new(HttpArtifactReader::new(fetcher, blob_server_url, delivery_blob_type)?);

        OtaVerificationCollector::collect(&update_package, &mut artifact_reader)
    })
    .await?;

    let json_output = serde_json::to_string_pretty(&report)
        .context("Failed to serialize OTA verification report to JSON")?;
    println!("{}", json_output);

    if !report.errors.is_empty() || !report.failed_blobs.is_empty() {
        errors::ffx_bail!("OTA Verification found errors in the update package");
    }
    Ok(report.deps)
}
