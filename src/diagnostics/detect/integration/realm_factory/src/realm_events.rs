// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::{RequestStream, ServerEnd};
use fidl_test_detect_factory::{TriageDetectEventsControlHandle, TriageDetectEventsMarker};
use log::error;

// A handle that emits TriageDetectEvents to the test suite.
// local mock components use this when they receive FIDL requests from the
// triage detect component, and the realm factory server uses it to notify
// when the component terminates.
#[derive(Clone)]
pub(crate) struct TriageDetectEventSender(TriageDetectEventsControlHandle);

impl TriageDetectEventSender {
    pub fn new(ctrl: TriageDetectEventsControlHandle) -> Self {
        Self(ctrl)
    }

    pub fn send_crash_report(&self, signature: &str, program_name: &str) {
        Self::log_err(self.0.send_on_crash_report(signature, program_name));
    }

    pub fn send_crash_reporting_product_registration(
        &self,
        product_name: &str,
        program_name: &str,
    ) {
        Self::log_err(
            self.0.send_on_crash_reporting_product_registration(product_name, program_name),
        );
    }

    pub fn send_on_done(&self) {
        Self::log_err(self.0.send_on_done());
    }

    pub fn send_on_bail(&self) {
        Self::log_err(self.0.send_on_bail());
    }

    pub fn send_on_diagnostic_fetch(&self) {
        Self::log_err(self.0.send_on_diagnostic_fetch());
    }

    fn log_err(result: Result<(), fidl::Error>) {
        if let Err(message) = result {
            error!("{:?}", message)
        }
    }
}

impl From<ServerEnd<TriageDetectEventsMarker>> for TriageDetectEventSender {
    fn from(value: ServerEnd<TriageDetectEventsMarker>) -> Self {
        let control_handle = value.into_stream().control_handle();
        Self::new(control_handle)
    }
}

// We use TriageDetectEventSender to send events from FakeArchiveAccessor.
#[async_trait::async_trait]
impl fake_archive_accessor::EventSignaler for TriageDetectEventSender {
    async fn signal_done(&self) {
        let _ = self.send_on_done();
    }

    async fn signal_fetch(&self) {
        let _ = self.send_on_diagnostic_fetch();
    }

    async fn signal_error(&self, _error: &str) {
        unimplemented!()
    }
}
