// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_WIRE_INTERNAL_FRAMEWORK_ERR_H_
#define LIB_FIDL_CPP_WIRE_INTERNAL_FRAMEWORK_ERR_H_

#include <lib/fidl/cpp/framework_err.h>
#include <lib/fidl/cpp/wire/internal/display_error.h>
#include <lib/fidl/cpp/wire/traits.h>
#include <lib/fidl/cpp/wire/wire_types.h>

namespace fidl {

template <>
struct IsFidlType<::fidl::internal::FrameworkErr> : public std::true_type {};
template <>
struct ContainsHandle<::fidl::internal::FrameworkErr> : public std::false_type {};

template <bool IsRecursive>
struct internal::WireCodingTraits<::fidl::internal::FrameworkErr,
                                  ::fidl::internal::WireCodingConstraintEmpty, IsRecursive> {
  static constexpr size_t kInlineSize = sizeof(int32_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(internal::WireEncoder* encoder, ::fidl::internal::FrameworkErr* value,
                     ::fidl::internal::WirePosition position,
                     RecursionDepth<IsRecursive> recursion_depth) {
    switch (*value) {
      case ::fidl::internal::FrameworkErr::kUnknownMethod:
        break;
      default:
        encoder->SetError(::fidl::internal::kCodingErrorUnknownEnumValue);
        return;
    }
    *position.As<::fidl::internal::FrameworkErr>() = *value;
  }
  static void Decode(internal::WireDecoder* decoder, ::fidl::internal::WirePosition position,
                     RecursionDepth<IsRecursive> recursion_depth) {
    ::fidl::internal::FrameworkErr value = *position.As<::fidl::internal::FrameworkErr>();
    switch (value) {
      case ::fidl::internal::FrameworkErr::kUnknownMethod:
        break;
      default:
        decoder->SetError(::fidl::internal::kCodingErrorUnknownEnumValue);
        return;
    }
  }
};

}  // namespace fidl

#endif  // LIB_FIDL_CPP_WIRE_INTERNAL_FRAMEWORK_ERR_H_
