// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::main(logging_tags = ["structured_log"])]
async fn main() -> Result<(), &'static str> {
    Err("This is a test error")
}
