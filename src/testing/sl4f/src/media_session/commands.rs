// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::media_session::facade::MediaSessionFacade;
use crate::server::Facade;
use anyhow::{bail, Error};
use async_trait::async_trait;
use serde_json::{to_value, Value};

#[async_trait(?Send)]
/// The media session facade is added here for media latency e2e test.
impl Facade for MediaSessionFacade {
    async fn handle_request(&self, method: String, _args: Value) -> Result<Value, Error> {
        match method.as_ref() {
            "WatchActiveSessionStatus" => {
                let result = self.watch_active_session_status().await?;
                Ok(to_value(result)?)
            }
            "PublishMockPlayer" => {
                let result = self.publish_mock_player().await?;
                Ok(to_value(result)?)
            }
            "StopMockPlayer" => {
                let result = self.stop_mock_player().await?;
                Ok(to_value(result)?)
            }
            "ListReceivedRequests" => {
                let result = self.list_received_requests().await?;
                Ok(to_value(result)?)
            }
            _ => bail!("Invalid MediaSessionFacade FIDL method: {:?}", method),
        }
    }
}
