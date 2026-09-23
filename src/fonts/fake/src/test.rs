// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use assert_matches::assert_matches;
use fuchsia_component::client;
use {fidl_fuchsia_fonts as fonts, fidl_fuchsia_fonts_experimental as fonts_exp};

#[fuchsia::test]
async fn test_stable_api() -> Result<(), Error> {
    let font_provider = client::connect_to_protocol::<fonts::ProviderMarker>()
        .context("connecting to stable server")?;

    let req = fonts::Request {
        family: None,
        weight: 400,
        width: 5,
        slant: fonts::Slant::Upright,
        language: None,
        character: 0,
        fallback_group: fonts::FallbackGroup::None,
        flags: 0,
    };
    let res = font_provider.get_font(&req).await?;
    assert_matches!(res, None);

    let res = font_provider.get_family_info("Roboto").await?;
    assert_matches!(res, None);

    let res = font_provider.get_typeface(&fonts::TypefaceRequest::default()).await?;
    assert_eq!(res, fonts::TypefaceResponse::default());

    let res = font_provider
        .get_font_family_info(&fonts::FamilyName { name: "Roboto".to_string() })
        .await?;
    assert_eq!(res, fonts::FontFamilyInfo::default());

    Ok(())
}

#[fuchsia::test]
async fn test_experimental_api() -> Result<(), Error> {
    let font_provider = client::connect_to_protocol::<fonts_exp::ProviderMarker>()
        .context("connecting to exp server")?;

    let res = font_provider.get_typeface_by_id(0).await?;
    assert_matches!(res, Err(fonts_exp::Error::NotFound));

    let (_client_end, server_end) = fidl::endpoints::create_endpoints();
    let res = font_provider
        .list_typefaces(&fonts_exp::ListTypefacesRequest::default(), server_end)
        .await?;
    assert_matches!(res, Err(fonts_exp::Error::NotFound));

    let res = font_provider
        .get_typefaces_by_family(&fonts::FamilyName { name: "Roboto".to_string() })
        .await?;
    assert_matches!(res, Err(fonts_exp::Error::NotFound));

    Ok(())
}
