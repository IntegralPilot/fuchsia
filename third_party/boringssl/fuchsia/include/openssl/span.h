// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef THIRD_PARTY_BORINGSSL_FUCHSIA_INCLUDE_OPENSSL_SPAN_H_
#define THIRD_PARTY_BORINGSSL_FUCHSIA_INCLUDE_OPENSSL_SPAN_H_

#include_next <openssl/span.h>

#if defined(__cplusplus) && defined(BORINGSSL_NO_CXX)

#include <span>

namespace bssl {
template <typename T>
using Span = std::span<T>;
}  // namespace bssl

#endif  // defined(__cplusplus) && defined(BORINGSSL_NO_CXX)

#endif  // THIRD_PARTY_BORINGSSL_FUCHSIA_INCLUDE_OPENSSL_SPAN_H_
