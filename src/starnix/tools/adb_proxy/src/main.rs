// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use server::ProxyServer;

#[fuchsia::main(logging_tags = ["adb_proxy"])]
async fn main() -> Result<(), Error> {
    let mut proxy_server = ProxyServer::new();
    proxy_server.start(5555).await?;
    Ok(())
}
