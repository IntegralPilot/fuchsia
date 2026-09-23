// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::trace_runner::{TerminationResult, TraceRunner};
use anyhow::format_err;
use log::{info, warn};
use realm_client::InstalledNamespace;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static NEXT_TRACING_ID: AtomicUsize = AtomicUsize::new(0);
static DEFAULT_TRACE_TIMEOUT: Duration = Duration::from_mins(2);
static DEFAULT_TRACE_FILE_MAX_BYTES: usize = 100 * 1024 * 1024;

/// An RAII-style struct that starts tracing in the test realm upon creation via `Tracing::start`
/// and collects and writes the trace when the struct is dropped.
pub struct Tracing {
    output_trace_path: std::ffi::OsString,
    tracer: TraceRunner,
    always_record_trace: bool,

    // Hermetic tracing keeps a reference to the test namespace so the realm factory does
    // not destroy the trace_manager component before the test drops an instance of Tracing.
    //
    // Non-hermetic tracing relies on the trace_manager running on the system. There is no
    // need to keep a reference to the test namespace in that case since trace_manager will
    // always outlive the test.
    _test_ns: Option<Arc<InstalledNamespace>>,
}

pub(crate) struct HermeticityParameters {
    pub(crate) service_prefix: String,
}

pub(crate) enum Hermeticity {
    NonHermetic,
    Hermetic(HermeticityParameters),
}

impl Hermeticity {
    pub fn new_hermetic(service_prefix: &str) -> Self {
        Self::Hermetic(HermeticityParameters { service_prefix: service_prefix.to_string() })
    }
}

impl Tracing {
    pub async fn start_non_hermetic(trace_file_prefix: &str) -> Result<Self, anyhow::Error> {
        Self::start_(
            trace_file_prefix,
            Hermeticity::NonHermetic,
            false,
            DEFAULT_TRACE_TIMEOUT,
            DEFAULT_TRACE_FILE_MAX_BYTES,
            None,
        )
        .await
    }

    pub async fn start_at(test_ns: Arc<InstalledNamespace>) -> Result<Self, anyhow::Error> {
        let service_prefix = test_ns.prefix().to_owned();
        Self::start_(
            service_prefix.strip_prefix("/").ok_or_else(|| {
                format_err!("Provided service prefix does not start with '/': {service_prefix}")
            })?,
            Hermeticity::new_hermetic(&service_prefix),
            false,
            DEFAULT_TRACE_TIMEOUT,
            DEFAULT_TRACE_FILE_MAX_BYTES,
            Some(test_ns),
        )
        .await
    }

    async fn start_<'a>(
        trace_file_prefix: &'a str,
        hermeticity: Hermeticity,
        always_record_trace: bool,
        trace_timeout: Duration,
        trace_file_max_bytes: usize,
        test_ns: Option<Arc<InstalledNamespace>>,
    ) -> Result<Self, anyhow::Error> {
        // The test namespace prefix ensures all generated trace files for a test suite
        // are unique per test realm.  In case multiple trace files are generated in the
        // same process, the tracing_id ensures their names will still be unique even
        // when the same test namespace prefix is used.
        let tracing_id = NEXT_TRACING_ID.fetch_add(1, Ordering::SeqCst);
        let output_trace_path = std::path::Path::new(
            format!("/custom_artifacts/{tracing_id:04}-{trace_file_prefix}-trace.fxt").as_str(),
        )
        .to_path_buf();

        let tracer = TraceRunner::start(
            hermeticity,
            output_trace_path.clone(),
            trace_timeout,
            trace_file_max_bytes,
        )
        .await?;

        Ok(Tracing {
            output_trace_path: output_trace_path.into(),
            tracer,
            always_record_trace,
            _test_ns: test_ns,
        })
    }
}

impl Drop for Tracing {
    fn drop(&mut self) {
        let always_record_trace = self.always_record_trace;
        let output_trace_path = self.output_trace_path.clone();

        match self.tracer.terminate_trace(format!("Tracing::drop")) {
            TerminationResult { termination_signal: None, .. } => {
                warn!("Terminate signal sent before Tracing::drop. Keeping trace.");
                return;
            }
            TerminationResult { termination_signal: Some(Err(e)), .. } => {
                warn!("Failed to signal termination of trace-writer: {e:?}");
                return;
            }
            TerminationResult { trace_writer: Some(Err(e)), .. } => {
                warn!("Failed to terminate trace-writer: {e:?}");
                return;
            }
            _ => (),
        }

        if !always_record_trace {
            info!("Discarding trace because Tracing instance dropped before a panic.");
            let _: Result<(), ()> = std::fs::remove_file(&output_trace_path)
                .map_err(|e| warn!("Failed to remove {}: {e:?}", output_trace_path.display()));
        }
    }
}
