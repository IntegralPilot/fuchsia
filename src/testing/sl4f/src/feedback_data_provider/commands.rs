// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::feedback_data_provider::facade::FeedbackDataProviderFacade;
use crate::feedback_data_provider::types::*;
use crate::server::Facade;
use anyhow::Error;
use async_trait::async_trait;
use serde_json::Value;

#[async_trait(?Send)]
impl Facade for FeedbackDataProviderFacade {
    async fn handle_request(&self, method: String, _args: Value) -> Result<Value, Error> {
        match method.parse()? {
            FeedbackDataProviderMethod::GetSnapshot => self.get_snapshot().await,
        }
    }
}
