// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `cmc` is the Component Manifest Compiler.

use clap::Parser;
use cmc::{opts, run_cmc};

fn main() {
    let opt = opts::Opt::parse();
    if let Err(e) = run_cmc(opt) {
        println!("{e}");
        std::process::exit(1);
    }
}
