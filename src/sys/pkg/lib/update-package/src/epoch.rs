// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Typesafe wrappers around parsing the epoch.json file.

use epoch::EpochFile;
use fidl_fuchsia_io as fio;
use thiserror::Error;
use zx_status::Status;

/// An error encountered while parsing the epoch.json file.
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum ParseEpochError {
    #[error("while opening the file")]
    OpenFile(#[source] fuchsia_fs::node::OpenError),

    #[error("while reading the file")]
    ReadFile(#[source] fuchsia_fs::file::ReadError),

    #[error("while deserializing: `{0:?}`")]
    Deserialize(String, #[source] serde_json::Error),
}

pub(crate) async fn epoch(proxy: &fio::DirectoryProxy) -> Result<Option<u64>, ParseEpochError> {
    let file = match fuchsia_fs::directory::open_file(proxy, "epoch.json", fio::PERM_READABLE).await
    {
        Ok(file) => file,
        Err(fuchsia_fs::node::OpenError::OpenError(Status::NOT_FOUND)) => return Ok(None),
        Err(e) => return Err(ParseEpochError::OpenFile(e)),
    };
    let contents =
        fuchsia_fs::file::read_to_string(&file).await.map_err(ParseEpochError::ReadFile)?;
    match serde_json::from_str(&contents).map_err(|e| ParseEpochError::Deserialize(contents, e))? {
        EpochFile::Version1 { epoch } => Ok(Some(epoch)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestUpdatePackage;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    async fn parse_epoch_success() {
        let p = TestUpdatePackage::new()
            .add_file("epoch.json", serde_json::to_vec(&EpochFile::Version1 { epoch: 3 }).unwrap())
            .await;
        assert_matches!(p.epoch().await, Ok(Some(3)));
    }

    #[fuchsia::test]
    async fn parse_epoch_success_missing_epoch_file() {
        let p = TestUpdatePackage::new();
        assert_matches!(p.epoch().await, Ok(None));
    }

    #[fuchsia::test]
    async fn parse_epoch_fail_deserialize() {
        let p = TestUpdatePackage::new().add_file("epoch.json", "oh no! this isn't json.").await;
        assert_matches!(
            p.epoch().await,
            Err(ParseEpochError::Deserialize(s,_)) if s == "oh no! this isn't json."
        );
    }
}
