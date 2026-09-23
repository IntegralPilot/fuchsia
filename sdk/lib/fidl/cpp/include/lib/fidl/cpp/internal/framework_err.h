// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_INTERNAL_FRAMEWORK_ERR_H_
#define LIB_FIDL_CPP_INTERNAL_FRAMEWORK_ERR_H_

#include <lib/fidl/cpp/framework_err.h>
#include <lib/fidl/cpp/wire/internal/framework_err.h>

#include "lib/fidl/cpp/natural_coding_traits.h"

namespace fidl {
namespace internal {

template <>
struct NaturalCodingTraits<::fidl::internal::FrameworkErr,
                           ::fidl::internal::NaturalCodingConstraintEmpty> {
  static constexpr size_t kInlineSize = sizeof(int32_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(internal::NaturalEncoder* encoder, ::fidl::internal::FrameworkErr* value,
                     size_t offset, size_t recursion_depth) {
    switch (*value) {
      case ::fidl::internal::FrameworkErr::kUnknownMethod:
        break;
      default:
        encoder->SetError(::fidl::internal::kCodingErrorUnknownEnumValue);
        return;
    }
    *encoder->template GetPtr<::fidl::internal::FrameworkErr>(offset) = *value;
  }
  static void Decode(internal::NaturalDecoder* decoder, ::fidl::internal::FrameworkErr* value,
                     size_t offset, size_t recursion_depth) {
    *value = *decoder->template GetPtr<::fidl::internal::FrameworkErr>(offset);
    switch (*value) {
      case ::fidl::internal::FrameworkErr::kUnknownMethod:
        break;
      default:
        decoder->SetError(::fidl::internal::kCodingErrorUnknownEnumValue);
        return;
    }
  }
};

}  // namespace internal
}  // namespace fidl

#endif  // LIB_FIDL_CPP_INTERNAL_FRAMEWORK_ERR_H_
