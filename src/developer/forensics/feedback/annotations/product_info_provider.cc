// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/product_info_provider.h"

#include <numeric>

#include "src/developer/forensics/feedback/annotations/constants.h"

namespace forensics::feedback {

Annotations ProductInfoToAnnotations::operator()(
    const fuchsia_hwinfo::ProductGetInfoResponse& response) {
  const fuchsia_hwinfo::ProductInfo& info = response.info();

  Annotations annotations = operator()(Error::kMissingValue);

  if (info.sku().has_value()) {
    annotations.insert_or_assign(kHardwareProductSKUKey, ErrorOrString(*info.sku()));
  }

  if (info.language().has_value()) {
    annotations.insert_or_assign(kHardwareProductLanguageKey, ErrorOrString(*info.language()));
  }

  if (info.regulatory_domain().has_value() &&
      info.regulatory_domain()->country_code().has_value()) {
    annotations.insert_or_assign(kHardwareProductRegulatoryDomainKey,
                                 ErrorOrString(*info.regulatory_domain()->country_code()));
  }

  if (info.locale_list().has_value() && !info.locale_list()->empty()) {
    auto begin = std::begin(*info.locale_list());
    auto end = std::end(*info.locale_list());

    const std::string locale_list = std::accumulate(
        std::next(begin), end, begin->id(),
        [](auto acc, const auto& locale) { return acc.append(", ").append(locale.id()); });
    annotations.insert_or_assign(kHardwareProductLocaleListKey, ErrorOrString(locale_list));
  }

  if (info.name().has_value()) {
    annotations.insert_or_assign(kHardwareProductNameKey, ErrorOrString(*info.name()));
  }

  if (info.model().has_value()) {
    annotations.insert_or_assign(kHardwareProductModelKey, ErrorOrString(*info.model()));
  }

  if (info.manufacturer().has_value()) {
    annotations.insert_or_assign(kHardwareProductManufacturerKey,
                                 ErrorOrString(*info.manufacturer()));
  }

  return annotations;
}

Annotations ProductInfoToAnnotations::operator()(const Error error) {
  return Annotations{
      {kHardwareProductSKUKey, ErrorOrString(error)},
      {kHardwareProductLanguageKey, ErrorOrString(error)},
      {kHardwareProductRegulatoryDomainKey, ErrorOrString(error)},
      {kHardwareProductLocaleListKey, ErrorOrString(error)},
      {kHardwareProductNameKey, ErrorOrString(error)},
      {kHardwareProductModelKey, ErrorOrString(error)},
      {kHardwareProductManufacturerKey, ErrorOrString(error)},
  };
}

std::set<std::string> ProductInfoProvider::GetAnnotationKeys() {
  return {
      kHardwareProductSKUKey,
      kHardwareProductLanguageKey,
      kHardwareProductRegulatoryDomainKey,
      kHardwareProductLocaleListKey,
      kHardwareProductNameKey,
      kHardwareProductModelKey,
      kHardwareProductManufacturerKey,
  };
}

std::set<std::string> ProductInfoProvider::GetKeys() const {
  return ProductInfoProvider::GetAnnotationKeys();
}

}  // namespace forensics::feedback
