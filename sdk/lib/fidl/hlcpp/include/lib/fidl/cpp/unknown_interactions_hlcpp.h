// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_UNKNOWN_INTERACTIONS_HLCPP_H_
#define LIB_FIDL_CPP_UNKNOWN_INTERACTIONS_HLCPP_H_

#include <lib/fidl/cpp/coding_traits.h>
#include <lib/fidl/cpp/framework_err.h>
#include <zircon/types.h>

#include "lib/fidl/cpp/internal/unknown_interactions_table.h"

namespace fidl {
using FrameworkErr = ::fidl::internal::FrameworkErr;

template <>
struct CodingTraits<::fidl::internal::FrameworkErr> {
  static constexpr size_t kInlineSize = sizeof(::fidl::internal::FrameworkErr);
  static void Encode(Encoder* encoder, ::fidl::internal::FrameworkErr* value, size_t offset,
                     std::optional<::fidl::HandleInformation> maybe_handle_info) {
    ZX_DEBUG_ASSERT(!maybe_handle_info);
    int32_t underlying = static_cast<int32_t>(*value);
    ::fidl::Encode(encoder, &underlying, offset);
  }
  static void Decode(Decoder* decoder, ::fidl::internal::FrameworkErr* value, size_t offset) {
    int32_t underlying = {};
    ::fidl::Decode(decoder, &underlying, offset);
    *value = static_cast<::fidl::internal::FrameworkErr>(underlying);
  }
};

inline zx_status_t Clone(::fidl::internal::FrameworkErr value,
                         ::fidl::internal::FrameworkErr* result) {
  *result = value;
  return ZX_OK;
}

template <>
struct Equality<::fidl::internal::FrameworkErr> {
  bool operator()(const ::fidl::internal::FrameworkErr& lhs,
                  const ::fidl::internal::FrameworkErr& rhs) const {
    return lhs == rhs;
  }
};

namespace internal {
// Encodes a FIDL union for the result union of a flexible method set to the
// framework_err variant with FrameworkErr::kUnknownMethod value.
::fidl::HLCPPOutgoingMessage EncodeUnknownMethodResponse(::fidl::MessageEncoder* encoder);
}  // namespace internal
}  // namespace fidl

#endif  // LIB_FIDL_CPP_UNKNOWN_INTERACTIONS_HLCPP_H_
