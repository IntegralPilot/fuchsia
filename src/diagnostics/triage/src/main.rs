// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use clap::Parser;
use log::*;
use triage_app_lib::Options;
use triage_app_lib::app::App;

/// Reports an [Error] to stdout and logs at "error" level.
fn report_failure(e: Error) {
    error!("Triage failed: {:?}", e);
    println!("Triage failed: {e:?}");
}

fn run_app() -> Result<i32, Error> {
    let app = App::new(Options::parse());
    let results = app.run(&mut std::io::stdout())?;
    Ok(match results {
        true => 1,
        false => 0,
    })
}

fn main() {
    std::process::exit(match run_app() {
        Ok(code) => code,
        Err(err) => {
            report_failure(err);
            1
        }
    })
}
