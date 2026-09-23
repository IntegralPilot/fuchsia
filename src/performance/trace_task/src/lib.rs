// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use flex_fuchsia_tracing_controller::{self as trace, RecordingError, StartError};

mod trace_task;
mod triggers;

pub use trace_task::TraceTask;
pub use triggers::{Trigger, TriggerAction, TriggersWatcher};

#[derive(Debug, thiserror::Error)]
pub enum TracingError {
    /// Error encountered when opening the proxy to the target.
    #[error("cannot open proxy")]
    TargetProxyOpen,
    /// This is a general error when starting a trace.
    #[error("cannot start recording: {0}")]
    RecordingStart(String),
    /// An error encountered if a trace recording has already been started
    /// for a given Fuchsia target.
    #[error("recording already started")]
    RecordingAlreadyStarted,
    /// An error encountered when attempting to stop a trace. This causes an
    /// immediate termination of the client channel, so the user should not
    /// attempt to run `StopRecording` again.
    #[error("unable to stop recording: {0:?}")]
    RecordingStop(String),
    /// Error for when a trace file is already being written to by the tracing
    /// service.
    #[error("trace file {0} already exists.")]
    DuplicateTraceFile(String),
    /// When attempting to stop a trace, there were no active traces found for
    /// the given lookup name.
    #[error("trace file {0} does not exist.")]
    NoSuchTraceFile(String),

    #[error("fidl error: {0:?}")]
    FidlError(#[from] fidl::Error),

    #[error("general error: {0}")]
    GeneralError(String),
}

impl From<StartError> for TracingError {
    fn from(value: StartError) -> Self {
        match value {
            StartError::NotInitialized => Self::RecordingStart("not initialized".into()),
            StartError::AlreadyStarted => Self::RecordingAlreadyStarted,
            StartError::Stopping => Self::RecordingStart("tracing is stopping".into()),
            StartError::Terminating => Self::RecordingStart("tracing is terminating".into()),
            e => Self::GeneralError(format!("Unknown StartError: {e:?}")),
        }
    }
}

impl Into<RecordingError> for TracingError {
    fn into(self) -> RecordingError {
        match self {
            TracingError::TargetProxyOpen => RecordingError::TargetProxyOpen,
            TracingError::RecordingStart(_) => RecordingError::RecordingStart,
            TracingError::RecordingAlreadyStarted => RecordingError::RecordingAlreadyStarted,
            TracingError::RecordingStop(_) => RecordingError::RecordingStop,
            TracingError::DuplicateTraceFile(_) => RecordingError::DuplicateTraceFile,
            TracingError::NoSuchTraceFile(_) => RecordingError::NoSuchTraceFile,
            TracingError::FidlError(_) | TracingError::GeneralError(_) => {
                RecordingError::RecordingStart
            }
        }
    }
}

impl PartialEq for TracingError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::RecordingStart(l0), Self::RecordingStart(r0)) => l0 == r0,
            (Self::RecordingStop(l0), Self::RecordingStop(r0)) => l0 == r0,
            (Self::DuplicateTraceFile(l0), Self::DuplicateTraceFile(r0)) => l0 == r0,
            (Self::NoSuchTraceFile(l0), Self::NoSuchTraceFile(r0)) => l0 == r0,
            (Self::FidlError(l0), Self::FidlError(r0)) => l0.to_string() == r0.to_string(),
            (Self::GeneralError(l0), Self::GeneralError(r0)) => l0 == r0,
            _ => core::mem::discriminant(self) == core::mem::discriminant(other),
        }
    }
}

pub(crate) async fn trace_shutdown(
    proxy: &trace::SessionProxy,
    cancel: bool,
) -> Result<trace::StopResult, TracingError> {
    let res = proxy
        .stop_tracing(&trace::StopOptions { write_results: Some(!cancel), ..Default::default() })
        .await
        .map_err(|e| {
            log::warn!("stopping tracing: {:?}", e);
            TracingError::RecordingStop(e.to_string())
        })?
        .map_err(|e| {
            let msg = format!("Received stop error: {:?}", e);
            log::warn!("{msg}");
            TracingError::RecordingStop(msg)
        });
    res
}
