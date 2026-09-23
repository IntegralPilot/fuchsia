// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component_runner as fcrunner;
use log::warn;

/// A runner provided by a FIDL protocol.
pub struct RemoteRunner {
    client: fcrunner::ComponentRunnerProxy,
}

impl RemoteRunner {
    pub fn new(client: fcrunner::ComponentRunnerProxy) -> RemoteRunner {
        RemoteRunner { client }
    }

    pub fn start(
        &self,
        start_info: fcrunner::ComponentStartInfo,
        server_end: ServerEnd<fcrunner::ComponentControllerMarker>,
    ) {
        let resolved_url = start_info.resolved_url.clone().unwrap_or(String::new());
        if let Err(e) = self.client.start(start_info, server_end) {
            warn!(url:% = resolved_url, error:% = e; "Failed to call runner to start component");
        }
    }
}
