// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::server::Facade;
use anyhow::{Error, format_err};
use async_trait::async_trait;
use log::*;
use serde_json::{Value, to_value};

// Testing helper methods
use crate::wlan::facade::WlanFacade;

#[async_trait(?Send)]
impl Facade for WlanFacade {
    async fn handle_request(&self, method: String, _args: Value) -> Result<Value, Error> {
        match method.as_ref() {
            "status" => {
                info!(tag = "WlanFacade"; "fetching connection status");
                let result = self.status().await?;
                to_value(result).map_err(|e| format_err!("error handling connection status: {}", e))
            }
            _ => return Err(format_err!("unsupported command!")),
        }
    }
}
