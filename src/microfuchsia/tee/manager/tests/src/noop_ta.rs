// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code)]
#![allow(unused_imports)]
use anyhow::{Context, Error};
use fuchsia_component::client::{connect_to_protocol_at, connect_to_protocol_at_path};
use {fidl_fuchsia_tee, fuchsia_fs};

#[fuchsia::test]
async fn connect_noop_ta() -> Result<(), Error> {
    const NOOP_TA_UUID: &str = "185d0391-bb47-495a-ba57-d6c6b808bfae";

    let ta_dir = fuchsia_fs::directory::open_in_namespace("/ta", fuchsia_fs::PERM_READABLE)
        .context("Failed to open /ta directory")?;
    let entries = fuchsia_fs::directory::readdir(&ta_dir).await?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, NOOP_TA_UUID);
    assert_eq!(entries[0].kind, fuchsia_fs::directory::DirentKind::Directory);
    let noop_ta = connect_to_protocol_at::<fidl_fuchsia_tee::ApplicationMarker>(
        "/ta/".to_owned() + NOOP_TA_UUID,
    )?;
    let (session_id, op_result) = noop_ta.open_session2(vec![]).await?;
    assert_eq!(op_result.return_code, Some(0));
    assert_eq!(op_result.return_origin, Some(fidl_fuchsia_tee::ReturnOrigin::TrustedApplication));
    let op_result = noop_ta.invoke_command(session_id, 0, vec![]).await?;
    assert_eq!(op_result.return_code, Some(0));
    assert_eq!(op_result.return_origin, Some(fidl_fuchsia_tee::ReturnOrigin::TrustedApplication));
    noop_ta.close_session(session_id).await?;

    Ok(())
}
