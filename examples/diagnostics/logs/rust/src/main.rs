// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::main]
async fn main() -> Result<(), anyhow::Error> {
    log::trace!("should not print");
    log::info!("should print");
    log::info!(foo = 1, bar = "baz"; "hello, world!");
    Ok(())
}
