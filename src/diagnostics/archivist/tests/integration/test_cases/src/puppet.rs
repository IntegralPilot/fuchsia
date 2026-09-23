// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_archivist_test::{LogPuppetLogRequest, PuppetProxy};
use fidl_fuchsia_diagnostics_types::Severity;

pub(crate) type LogMessage = (Severity, &'static str);

#[async_trait::async_trait]
pub(crate) trait PuppetProxyExt {
    async fn log_messages(&self, messages: Vec<LogMessage>);
}

#[async_trait::async_trait]
impl PuppetProxyExt for PuppetProxy {
    async fn log_messages(&self, messages: Vec<LogMessage>) {
        for (severity, message) in messages {
            let request = &LogPuppetLogRequest {
                severity: Some(severity),
                message: Some(message.to_string()),
                ..Default::default()
            };
            self.log(request).await.expect("log succeeds");
        }
    }
}
