// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use argh;

use make_fuchsia_vol::args::TopLevel;
use make_fuchsia_vol::run;

fn main() -> Result<(), Error> {
    run(argh::from_env::<TopLevel>())
}
