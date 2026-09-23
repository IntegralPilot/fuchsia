// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_recovery as frecovery;
use fuchsia_component::client::connect_to_protocol;

pub async fn factory_data_reset() -> Result<(), Error> {
    let proxy = connect_to_protocol::<frecovery::FactoryResetMarker>()?;
    zx::Status::ok(proxy.reset().await?)?;
    Ok(())
}
