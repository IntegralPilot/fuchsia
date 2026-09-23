// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cml::error::Error;
use component_manager_config::compile;

fn main() -> Result<(), Error> {
    compile(argh::from_env())
}
