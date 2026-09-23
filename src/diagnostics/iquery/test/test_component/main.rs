// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use clap::{CommandFactory, Parser};
use fuchsia_inspect::component;
use inspect_runtime::PublishOptions;
use inspect_testing::ExampleInspectData;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let opts = inspect_testing::Options::parse();
    if opts.rows == 0 || opts.columns == 0 {
        inspect_testing::Options::command().print_help()?;
        std::process::exit(1);
    }

    let mut inspect_data = ExampleInspectData::default();
    inspect_data.write_to(component::inspector().root());

    inspect_runtime::publish(component::inspector(), PublishOptions::default()).unwrap().await;

    Ok(())
}
