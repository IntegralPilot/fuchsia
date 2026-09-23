// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START main]
use anyhow::Error;

/// Creates a simple session that just prints "Hello World" to the syslog.
#[fuchsia::main(logging = true)]
async fn main() -> Result<(), Error> {
    log::info!("Hello World!");

    Ok(())
}
// [END main]

#[cfg(test)]
mod tests {
    #[fuchsia::test]
    async fn smoke_test() {
        assert!(true);
    }
}
