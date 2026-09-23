// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_examples_reverser as freverser;
use trf_codegen_macro as trf;

#[trf::test]
pub async fn test_reverser_trf_basic(realm: &TestRealm) -> Result<(), Error> {
    let client = realm.connect_to_protocol::<freverser::ReverserMarker>().await?;

    let input = "Hello Fuchsia TRF!";
    let output = client.reverse(input).await?;
    assert_eq!(output, "!FRT aishcuF olleH");
    assert_eq!(input.len(), output.len());

    Ok(())
}
