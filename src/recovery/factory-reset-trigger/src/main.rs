// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use forced_fdr::perform_fdr_if_necessary;

#[fuchsia::main(logging_tags = ["factory-reset-trigger"])]
async fn main() -> Result<(), Error> {
    perform_fdr_if_necessary().await;

    Ok(())
}
