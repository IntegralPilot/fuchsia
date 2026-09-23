// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::logs::error::LogsError;
use crate::logs::listener::Listener;
use crate::logs::repository::LogsRepository;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_diagnostics::StreamMode;
use futures::StreamExt;
use log::warn;
use std::pin::pin;
use std::sync::Arc;
use {fidl_fuchsia_logger as flogger, fuchsia_async as fasync};

pub struct LogServer {
    /// The repository holding the logs.
    logs_repo: Arc<LogsRepository>,

    /// Scope in which we spawn all of the server tasks.
    scope: fasync::Scope,
}

impl LogServer {
    pub fn new(logs_repo: Arc<LogsRepository>, scope: fasync::Scope) -> Self {
        Self { logs_repo, scope }
    }

    /// Spawn a task to handle requests from components reading the shared log.
    pub fn spawn(&self, stream: flogger::LogRequestStream) {
        let logs_repo = Arc::clone(&self.logs_repo);
        let scope = self.scope.to_handle();
        self.scope.spawn(async move {
            if let Err(e) = Self::handle_requests(logs_repo, stream, scope).await {
                warn!("error handling Log requests: {}", e);
            }
        });
    }

    /// Handle requests to `fuchsia.logger.Log`. All request types read the
    /// whole backlog from memory, `DumpLogs(Safe)` stops listening after that.
    async fn handle_requests(
        logs_repo: Arc<LogsRepository>,
        mut stream: flogger::LogRequestStream,
        scope: fasync::ScopeHandle,
    ) -> Result<(), LogsError> {
        let connection_id = logs_repo.new_interest_connection();
        while let Some(request) = stream.next().await {
            let request = request.map_err(|source| LogsError::HandlingRequests {
                protocol: flogger::LogMarker::PROTOCOL_NAME,
                source,
            })?;
            let listener = match request {
                flogger::LogRequest::ListenSafe { log_listener, options, .. } => {
                    Listener::new(log_listener, options)?
                }
            };
            let logs =
                logs_repo.logs_cursor(StreamMode::SnapshotThenSubscribe, Vec::new()).map(Arc::new);
            scope.spawn(async move {
                listener.run(pin!(logs)).await;
            });
        }
        logs_repo.finish_interest_connection(connection_id);
        Ok(())
    }
}
