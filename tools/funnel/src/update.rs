// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(feature = "update")]
mod update {

    use camino::{Utf8Path, Utf8PathBuf};
    use std::io;
    use std::process::Command;
    use thiserror::Error;

    use crate::errors::IntoExitCode;

    #[derive(Debug, Error)]
    pub enum UpdateError {
        #[error("invalid parent directory for funnel located at: {funnel_path}")]
        InvalidParentPath { funnel_path: Utf8PathBuf },
        #[error("binary cipd not found on $PATH")]
        CipdNotFound,
        #[error("no cipd manifest found at \"{expected_path}\"")]
        NoCipdEnsureFile { expected_path: Utf8PathBuf },
        #[error("error returned from cipd-ensure. Code {code}")]
        CipdEnsureError { code: i32 },
        #[error("cipd-ensure was terminated by a signal")]
        CipdEnsureTerminated,
        #[error("could not canonicalize path: {given_path}")]
        CouldNotCanonicalizePath {
            given_path: Utf8PathBuf,
            #[source]
            source: io::Error,
        },
        #[error("unknown error updating: {0}")]
        Unknown(#[source] io::Error),
    }

    impl IntoExitCode for UpdateError {
        fn exit_code(&self) -> i32 {
            match self {
                Self::InvalidParentPath { funnel_path: _ } => 20,
                Self::CipdNotFound => 21,
                Self::NoCipdEnsureFile { expected_path: _ } => 22,
                Self::CipdEnsureError { code } => *code,
                Self::CipdEnsureTerminated => 23,
                Self::CouldNotCanonicalizePath { given_path: _, source } => {
                    source.raw_os_error().unwrap_or_else(|| 24)
                }
                Self::Unknown(_) => 1,
            }
        }
    }

    const CIPD_MANIFEST_NAME: &str = "funnel-cipd-manifest";

    pub async fn self_update(funnel_path: impl AsRef<Utf8Path>) -> Result<(), UpdateError> {
        // Check if  cipd manifest exists in the same directory as our funnel binary
        let funnel_parent = funnel_path.as_ref().parent().ok_or_else(|| {
            UpdateError::InvalidParentPath { funnel_path: funnel_path.as_ref().into() }
        })?;
        let ensure_file = funnel_parent.join(CIPD_MANIFEST_NAME);
        if !ensure_file.exists() {
            return Err(UpdateError::NoCipdEnsureFile { expected_path: ensure_file.into() });
        }
        // Run cipd ensure
        let mut cipd = Command::new("cipd");
        cipd.arg("ensure");
        cipd.arg("-ensure-file");
        cipd.arg(ensure_file.canonicalize().map_err(|e| {
            UpdateError::CouldNotCanonicalizePath { given_path: ensure_file, source: e }
        })?);
        cipd.arg("-root");
        cipd.arg(funnel_parent.canonicalize().map_err(|e| {
            UpdateError::CouldNotCanonicalizePath { given_path: funnel_parent.into(), source: e }
        })?);

        log::debug!("about to run cipd command: {:?}", cipd);
        // Check errors
        match cipd.spawn() {
            Ok(mut cipd_process) => {
                // Have the child process. Wait for it to exit.
                match cipd_process.wait() {
                    Ok(exit_status) => {
                        if exit_status.success() {
                            return Ok(());
                        }
                        match exit_status.code() {
                            None => Err(UpdateError::CipdEnsureTerminated),
                            Some(code) => Err(UpdateError::CipdEnsureError { code }),
                        }
                    }
                    Err(e) => Err(UpdateError::Unknown(e)),
                }
            }
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => Err(UpdateError::CipdNotFound),
                _ => Err(UpdateError::Unknown(e)),
            },
        }
    }
}
