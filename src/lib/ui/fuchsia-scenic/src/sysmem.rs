// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::{ClientEnd, create_endpoints};
use fidl_fuchsia_sysmem2 as fsysmem2;
use fidl_fuchsia_ui_composition as fland;
use fsysmem2::BufferCollectionTokenDuplicateRequest;

// Pair of tokens to be used with Scenic Allocator FIDL protocol.
pub struct BufferCollectionTokenPair {
    pub export_token: fland::BufferCollectionExportToken,
    pub import_token: fland::BufferCollectionImportToken,
}

impl BufferCollectionTokenPair {
    pub fn new() -> BufferCollectionTokenPair {
        let (raw_export_token, raw_import_token) = zx::EventPair::create();
        BufferCollectionTokenPair {
            export_token: fland::BufferCollectionExportToken { value: raw_export_token },
            import_token: fland::BufferCollectionImportToken { value: raw_import_token },
        }
    }
}

/// Given a Scenic `BufferCollectionImportToken`, returns a new version which has been duplicated.
pub fn duplicate_buffer_collection_import_token(
    import_token: &fland::BufferCollectionImportToken,
) -> Result<fland::BufferCollectionImportToken, Error> {
    let handle = import_token.value.as_handle_ref().duplicate_handle(zx::Rights::SAME_RIGHTS)?;
    Ok(fland::BufferCollectionImportToken { value: handle.into() })
}

/// Calls `BufferCollectionToken.Duplicate()` on the provided token, passing the server end of a
/// newly-instantiated channel.  Then, calls `Sync()` on the provided token, so that the returned
/// token is safe to use immediately (i.e. the server has acknowledged that the duplication has
/// occurred).
pub async fn duplicate_buffer_collection_token(
    token: &mut fsysmem2::BufferCollectionTokenProxy,
) -> Result<ClientEnd<fsysmem2::BufferCollectionTokenMarker>, Error> {
    let (duplicate_token, duplicate_token_server_end) =
        create_endpoints::<fsysmem2::BufferCollectionTokenMarker>();

    token.duplicate(BufferCollectionTokenDuplicateRequest {
        rights_attenuation_mask: Some(fidl::Rights::SAME_RIGHTS),
        token_request: Some(duplicate_token_server_end),
        ..Default::default()
    })?;
    token.sync().await?;

    Ok(duplicate_token)
}
