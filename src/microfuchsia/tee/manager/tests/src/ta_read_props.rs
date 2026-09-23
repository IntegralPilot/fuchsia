// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// These tests use a rust test component to talk to the test_props TA component.
// The TA component itself performs the interesting test via internal core APIs.
use anyhow::Error;
use fuchsia_component::client::connect_to_protocol_at;
use tee_internal::binding::TEE_SUCCESS;
use {fidl_fuchsia_tee, fuchsia_fs as _};

// TODO: single source of truth for TA CMD IDs, which should be defined by the TA.
const CMD_TEST_PROPS_EXIST: u32 = 0;

#[fuchsia::test]
async fn test_props_file_exists() -> Result<(), Error> {
    const TA_READ_PROPS_UUID: &str = "932517f3-e807-48f8-b584-fbeec848c2b6";

    let noop_ta = connect_to_protocol_at::<fidl_fuchsia_tee::ApplicationMarker>(
        "/ta/".to_owned() + TA_READ_PROPS_UUID,
    )?;
    let (session_id, op_result) = noop_ta.open_session2(vec![]).await?;
    assert_eq!(op_result.return_code, Some(TEE_SUCCESS as u64));
    assert_eq!(op_result.return_origin, Some(fidl_fuchsia_tee::ReturnOrigin::TrustedApplication));

    let op_result = noop_ta.invoke_command(session_id, CMD_TEST_PROPS_EXIST, vec![]).await?;
    assert_eq!(op_result.return_code, Some(TEE_SUCCESS as u64));
    assert_eq!(op_result.return_origin, Some(fidl_fuchsia_tee::ReturnOrigin::TrustedApplication));
    noop_ta.close_session(session_id).await?;

    Ok(())
}
