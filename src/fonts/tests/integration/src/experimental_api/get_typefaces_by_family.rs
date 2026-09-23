// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file

use super::util::*;
use crate::FONTS_SMALL_CM;

// Add new tests here so we don't overload component manager with requests (58150)
#[fuchsia::test]
async fn test_get_typefaces_by_family() {
    let factory = ProviderFactory::new();
    test_get_typefaces_by_family_basic(&factory).await.unwrap();
    test_get_typefaces_by_family_alias(&factory).await.unwrap();
    test_get_typefaces_by_family_not_found(&factory).await.unwrap();
}

async fn test_get_typefaces_by_family_basic(factory: &ProviderFactory) -> Result<(), Error> {
    let font_provider = factory.get_provider(FONTS_SMALL_CM).await?;
    let family = fonts::FamilyName { name: String::from("Roboto") };

    let response = font_provider.get_typefaces_by_family(&family).await?;
    let faces = response.unwrap().results.unwrap();

    assert_eq!(faces.len(), 3);
    assert_eq!(faces[0], roboto_info(1, fonts::WEIGHT_LIGHT));
    assert_eq!(faces[1], roboto_info(2, fonts::WEIGHT_MEDIUM));
    assert_eq!(faces[2], roboto_info(3, fonts::WEIGHT_NORMAL));
    Ok(())
}

async fn test_get_typefaces_by_family_alias(factory: &ProviderFactory) -> Result<(), Error> {
    let font_provider = factory.get_provider(FONTS_SMALL_CM).await?;
    let family = fonts::FamilyName { name: String::from("Material Design Icons") };
    let alias = fonts::FamilyName { name: String::from("MaterialIcons") };

    let by_family = font_provider.get_typefaces_by_family(&family).await?;
    let by_alias = font_provider.get_typefaces_by_family(&alias).await?;

    let by_family_faces = by_family.unwrap().results.unwrap();
    let by_alias_faces = by_alias.unwrap().results.unwrap();

    assert_eq!(by_family_faces.len(), 1);
    assert_eq!(by_alias_faces.len(), 1);
    assert_eq!(by_family_faces[0], by_alias_faces[0]);
    Ok(())
}

async fn test_get_typefaces_by_family_not_found(factory: &ProviderFactory) -> Result<(), Error> {
    let font_provider = factory.get_provider(FONTS_SMALL_CM).await?;
    let family = fonts::FamilyName { name: String::from("NoSuchFont") };
    let response = font_provider.get_typefaces_by_family(&family).await?;
    assert_eq!(response.unwrap_err(), fonts_exp::Error::NotFound);
    Ok(())
}
