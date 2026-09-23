// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file is compiled into a binary and used in zxdb tests to query symbol information. The
// actual code is not run. Line numbers matter and must be in sync with the test expectations.

async fn async_fn(i: i32) -> i32 {
    i + 1
}

#[fuchsia::main()]
async fn main() {
    // Setting a breakpoint on this line is tricky, the generated code will also be attributed to
    // this line, but can sometimes contain line tables that point to instructions that will not
    // execute. See https://fxbug.dev/331475631.
    let _i = async_fn(5).await;
}
