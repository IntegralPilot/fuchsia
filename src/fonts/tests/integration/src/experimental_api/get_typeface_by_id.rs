// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file

use super::util::*;
use crate::FONTS_SMALL_CM;
use anyhow::format_err;

#[fuchsia::test]
async fn test_get_typeface_by_id_basic() {
    let factory = ProviderFactory::new();
    let font_provider = factory.get_provider(FONTS_SMALL_CM).await.unwrap();
    // There will always be a font with index 0 unless manifest loading fails.
    let response = font_provider
        .get_typeface_by_id(0)
        .await
        .unwrap()
        .map_err(|e| format_err!("{:#?}", e))
        .unwrap();
    assert_eq!(response.buffer_id, Some(0));
    assert!(response.buffer.is_some());
}

#[fuchsia::test]
async fn test_get_typeface_by_id_not_found() {
    let factory = ProviderFactory::new();
    let font_provider = factory.get_provider(FONTS_SMALL_CM).await.unwrap();
    let response = font_provider.get_typeface_by_id(u32::MAX).await.unwrap();
    assert_eq!(response.unwrap_err(), fonts_exp::Error::NotFound);
}
