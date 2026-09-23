// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::PartitionsArgs;
use anyhow::Result;
use assembly_container::AssemblyContainer;
use assembly_partitions_config::PartitionsConfig;

pub fn new(args: &PartitionsArgs) -> Result<()> {
    let config = PartitionsConfig::from_config_path(&args.config)?;
    config.write_to_dir(&args.output, args.depfile.as_ref())?;
    Ok(())
}
