// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/product_info_provider.h"

#include <fidl/fuchsia.hwinfo/cpp/fidl.h>
#include <fidl/fuchsia.intl/cpp/fidl.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/annotations/types.h"

namespace forensics::feedback {
namespace {

using ::testing::Pair;
using ::testing::UnorderedElementsAreArray;

TEST(ProductInfoToAnnotationsTest, ConvertSuccess) {
  ProductInfoToAnnotations convert;

  fuchsia_hwinfo::ProductInfo info;
  fuchsia_hwinfo::ProductGetInfoResponse response{{.info = info}};
  EXPECT_THAT(convert(response),
              UnorderedElementsAreArray({
                  Pair(kHardwareProductSKUKey, Error::kMissingValue),
                  Pair(kHardwareProductLanguageKey, Error::kMissingValue),
                  Pair(kHardwareProductRegulatoryDomainKey, Error::kMissingValue),
                  Pair(kHardwareProductLocaleListKey, Error::kMissingValue),
                  Pair(kHardwareProductNameKey, Error::kMissingValue),
                  Pair(kHardwareProductModelKey, Error::kMissingValue),
                  Pair(kHardwareProductManufacturerKey, Error::kMissingValue),
              }));

  info.sku("sku");
  response = fuchsia_hwinfo::ProductGetInfoResponse{{.info = info}};
  EXPECT_THAT(convert(response),
              UnorderedElementsAreArray({
                  Pair(kHardwareProductSKUKey, ErrorOrString("sku")),
                  Pair(kHardwareProductLanguageKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductRegulatoryDomainKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductLocaleListKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductNameKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductModelKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductManufacturerKey, ErrorOrString(Error::kMissingValue)),
              }));

  info.language("language");
  response = fuchsia_hwinfo::ProductGetInfoResponse{{.info = info}};
  EXPECT_THAT(convert(response),
              UnorderedElementsAreArray({
                  Pair(kHardwareProductSKUKey, ErrorOrString("sku")),
                  Pair(kHardwareProductLanguageKey, ErrorOrString("language")),
                  Pair(kHardwareProductRegulatoryDomainKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductLocaleListKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductNameKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductModelKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductManufacturerKey, ErrorOrString(Error::kMissingValue)),
              }));

  fuchsia_intl::RegulatoryDomain regulatory_domain;
  regulatory_domain.country_code("country");
  info.regulatory_domain(regulatory_domain);
  response = fuchsia_hwinfo::ProductGetInfoResponse{{.info = info}};
  EXPECT_THAT(convert(response),
              UnorderedElementsAreArray({
                  Pair(kHardwareProductSKUKey, ErrorOrString("sku")),
                  Pair(kHardwareProductLanguageKey, ErrorOrString("language")),
                  Pair(kHardwareProductRegulatoryDomainKey, ErrorOrString("country")),
                  Pair(kHardwareProductLocaleListKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductNameKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductModelKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductManufacturerKey, ErrorOrString(Error::kMissingValue)),
              }));

  info.locale_list(std::vector{
      fuchsia_intl::LocaleId{{.id = "locale1"}},
      fuchsia_intl::LocaleId{{.id = "locale2"}},
      fuchsia_intl::LocaleId{{.id = "locale3"}},
  });
  response = fuchsia_hwinfo::ProductGetInfoResponse{{.info = info}};
  EXPECT_THAT(convert(response),
              UnorderedElementsAreArray({
                  Pair(kHardwareProductSKUKey, ErrorOrString("sku")),
                  Pair(kHardwareProductLanguageKey, ErrorOrString("language")),
                  Pair(kHardwareProductRegulatoryDomainKey, ErrorOrString("country")),
                  Pair(kHardwareProductLocaleListKey, ErrorOrString("locale1, locale2, locale3")),
                  Pair(kHardwareProductNameKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductModelKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductManufacturerKey, ErrorOrString(Error::kMissingValue)),
              }));

  info.name("name");
  response = fuchsia_hwinfo::ProductGetInfoResponse{{.info = info}};
  EXPECT_THAT(convert(response),
              UnorderedElementsAreArray({
                  Pair(kHardwareProductSKUKey, ErrorOrString("sku")),
                  Pair(kHardwareProductLanguageKey, ErrorOrString("language")),
                  Pair(kHardwareProductRegulatoryDomainKey, ErrorOrString("country")),
                  Pair(kHardwareProductLocaleListKey, ErrorOrString("locale1, locale2, locale3")),
                  Pair(kHardwareProductNameKey, ErrorOrString("name")),
                  Pair(kHardwareProductModelKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kHardwareProductManufacturerKey, ErrorOrString(Error::kMissingValue)),
              }));

  info.model("model");
  response = fuchsia_hwinfo::ProductGetInfoResponse{{.info = info}};
  EXPECT_THAT(convert(response),
              UnorderedElementsAreArray({
                  Pair(kHardwareProductSKUKey, ErrorOrString("sku")),
                  Pair(kHardwareProductLanguageKey, ErrorOrString("language")),
                  Pair(kHardwareProductRegulatoryDomainKey, ErrorOrString("country")),
                  Pair(kHardwareProductLocaleListKey, ErrorOrString("locale1, locale2, locale3")),
                  Pair(kHardwareProductNameKey, ErrorOrString("name")),
                  Pair(kHardwareProductModelKey, ErrorOrString("model")),
                  Pair(kHardwareProductManufacturerKey, ErrorOrString(Error::kMissingValue)),
              }));

  info.manufacturer("manufacturer");
  response = fuchsia_hwinfo::ProductGetInfoResponse{{.info = info}};
  EXPECT_THAT(convert(response),
              UnorderedElementsAreArray({
                  Pair(kHardwareProductSKUKey, ErrorOrString("sku")),
                  Pair(kHardwareProductLanguageKey, ErrorOrString("language")),
                  Pair(kHardwareProductRegulatoryDomainKey, ErrorOrString("country")),
                  Pair(kHardwareProductLocaleListKey, ErrorOrString("locale1, locale2, locale3")),
                  Pair(kHardwareProductNameKey, ErrorOrString("name")),
                  Pair(kHardwareProductModelKey, ErrorOrString("model")),
                  Pair(kHardwareProductManufacturerKey, ErrorOrString("manufacturer")),
              }));
}

TEST(ProductInfoToAnnotationsTest, ConvertError) {
  ProductInfoToAnnotations convert;
  EXPECT_THAT(convert(Error::kConnectionError),
              UnorderedElementsAreArray({
                  Pair(kHardwareProductSKUKey, Error::kConnectionError),
                  Pair(kHardwareProductLanguageKey, Error::kConnectionError),
                  Pair(kHardwareProductRegulatoryDomainKey, Error::kConnectionError),
                  Pair(kHardwareProductLocaleListKey, Error::kConnectionError),
                  Pair(kHardwareProductNameKey, Error::kConnectionError),
                  Pair(kHardwareProductModelKey, Error::kConnectionError),
                  Pair(kHardwareProductManufacturerKey, Error::kConnectionError),
              }));
}

TEST(ProductInfoProvider, Keys) {
  // Safe to pass nullptrs b/c objects are never used.
  ProductInfoProvider provider(nullptr, nullptr, nullptr);

  EXPECT_THAT(provider.GetKeys(), UnorderedElementsAreArray({
                                      kHardwareProductSKUKey,
                                      kHardwareProductLanguageKey,
                                      kHardwareProductRegulatoryDomainKey,
                                      kHardwareProductLocaleListKey,
                                      kHardwareProductNameKey,
                                      kHardwareProductModelKey,
                                      kHardwareProductManufacturerKey,
                                  }));
}

}  // namespace
}  // namespace forensics::feedback
