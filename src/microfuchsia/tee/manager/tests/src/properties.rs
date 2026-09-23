// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use std::path::Path;
use tee_properties::PropSet;

#[fuchsia::test]
async fn read_system_properties_from_ta_manager() -> Result<(), Error> {
    // The /properties dir is expected to be routed from TA manager.
    let res = PropSet::from_config_file(Path::new("/properties/system_properties"));
    assert!(res.is_ok(), "Failed to load system properties: {:?}.", res.err());
    Ok(())
}
