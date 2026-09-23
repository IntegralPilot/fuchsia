// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use ffx_e2e_emu::IsolatedEmulator;
use pretty_assertions::assert_eq;

#[fuchsia::test(logging = true)]
async fn ssh_works_and_reports_failures() -> Result<()> {
    let emu = IsolatedEmulator::start("target-ssh").await?;

    let expected_message = "hello, world!";
    let message = emu.ssh_output(&["echo", expected_message]).await?;
    assert_eq!(message.trim(), expected_message);

    emu.ssh(&["false"])
        .await
        .expect_err("the false command should fail and the failure should be reported to caller");
    Ok(())
}
