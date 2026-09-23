// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input::Input;
use crate::util::digest_path;
use anyhow::{Context as _, Result, bail};
use flex_fuchsia_fuzzer::{Artifact as FidlArtifact, Result_ as FuzzResult};
use std::fs;
use std::path::{Path, PathBuf};
use zx_status as zx;

/// Combines the results of a long-running fuzzer workflow.
pub struct Artifact {
    /// If `status` is OK, indicates the outcome of the workflow; otherwise undefined.
    pub result: FuzzResult,

    /// The path to which the fuzzer input, if any, has been saved.
    pub path: Option<PathBuf>,
}

impl Artifact {
    /// Returns an artifact for a workflow that completed without producing results or data.
    pub fn ok() -> Self {
        Self { result: FuzzResult::NoErrors, path: None }
    }

    /// Returns an artifact for a workflow that produced a result without data.
    pub fn from_result(result: FuzzResult) -> Self {
        Self { result, path: None }
    }

    /// Returns `artifact.path` as a String, or an empty string if it is `None`.
    pub fn pathname(&self) -> String {
        self.path.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or(String::default())
    }
}

/// Reads fuzzer input data from a `FidlArtifact` and saves it locally.
///
/// Returns:
/// Returns an `Artifact` on success. Returns an error if the artifact indicates an error, if
/// it fails to read the data from the `input`, or if it fails to write the data to the file.
///
/// See also `utils::digest_path`.
///
pub async fn save_artifact<P: AsRef<Path>>(
    fidl_artifact: FidlArtifact,
    out_dir: P,
) -> Result<Option<Artifact>> {
    if let Some(e) = fidl_artifact.error {
        if e == zx::Status::PEER_CLOSED.into_raw() {
            return Ok(None);
        }
        bail!("workflow returned an error: ZX_ERR_{}", e);
    }
    let result = fidl_artifact.result.context("invalid FIDL artifact: missing result")?;
    let mut artifact = Artifact::from_result(result);
    if let Some(fidl_input) = fidl_artifact.input {
        let input =
            Input::try_receive(fidl_input).await.context("failed to receive fuzzer input data")?;
        if artifact.result != FuzzResult::NoErrors {
            let path = digest_path(out_dir, Some(artifact.result), &input.data);
            fs::write(&path, input.data).with_context(|| {
                format!("failed to write fuzzer input to '{}'", path.to_string_lossy())
            })?;
            artifact.path = Some(path);
        }
    };
    Ok(Some(artifact))
}

#[cfg(test)]
mod tests {
    use super::save_artifact;
    use crate::input::InputPair;
    use crate::util::digest_path;
    use anyhow::Result;
    use flex_fuchsia_fuzzer::{Artifact as FidlArtifact, Result_ as FuzzResult};
    use fuchsia_fuzzctl_test::{Test, verify_saved};
    use futures::join;

    #[fuchsia::test]
    async fn test_save_artifact() -> Result<()> {
        let test = Test::try_new()?;
        let saved_dir = test.create_dir("saved")?;

        let input_pair = InputPair::try_from_data(&test.domain(), b"data".to_vec())?;
        let (fidl_input, input) = input_pair.as_tuple();
        let send_fut = input.send();
        let fidl_artifact = FidlArtifact {
            result: Some(FuzzResult::Crash),
            input: Some(fidl_input),
            ..Default::default()
        };
        let save_fut = save_artifact(fidl_artifact, &saved_dir);
        let results = join!(send_fut, save_fut);
        assert!(results.0.is_ok());
        assert!(results.1.is_ok());
        let saved = digest_path(&saved_dir, Some(FuzzResult::Crash), b"data");
        verify_saved(&saved, b"data")?;
        Ok(())
    }
}
