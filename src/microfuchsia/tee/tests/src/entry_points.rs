// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_tee::ApplicationMarker;
use fuchsia_component::client::connect_to_protocol;

#[fuchsia::test]
async fn entry_points_ta_session() -> Result<(), Error> {
    let app = connect_to_protocol::<ApplicationMarker>()
        .context("Failed to connect to application instance")?;
    let (session_id, op_result) = app.open_session2(vec![]).await?;
    assert_eq!(op_result.return_code, Some(0));
    assert_eq!(session_id, 1);
    app.close_session(session_id).await?;
    std::mem::drop(app);
    Ok(())
}
