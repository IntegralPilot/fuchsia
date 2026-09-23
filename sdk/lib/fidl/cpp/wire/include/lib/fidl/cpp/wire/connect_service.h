// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_WIRE_CONNECT_SERVICE_H_
#define LIB_FIDL_CPP_WIRE_CONNECT_SERVICE_H_

#include <lib/fidl/cpp/features.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/internal/transport.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fidl/cpp/wire/sync_call.h>
#include <zircon/fidl.h>

#if __FIDL_SUPPORT_HANDLES
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#endif  // __FIDL_SUPPORT_HANDLES

namespace fidl {

#if __FIDL_SUPPORT_HANDLES

namespace internal {

// The method signature required to implement the method that issues the Directory::Open
// FIDL call for a Service's member protocol.
using ConnectMemberFunc = zx::result<> (*)(zx::unowned_channel service_dir,
                                           fidl::StringView member_name,
                                           fidl::internal::AnyTransport channel);

}  // namespace internal

#endif  // __FIDL_SUPPORT_HANDLES

namespace internal {
// Helper type for compile-time string concatenation.
template <const char*, typename>
struct default_service_path;
template <const char* n, size_t... i>
struct default_service_path<n, std::integer_sequence<size_t, i...>> {
  static constexpr const char value[]{'/', 's', 'v', 'c', '/', n[i]...};
};
}  // namespace internal

// DiscoverableProtocolName<Protocol> evaluates to a string containing the name of the protocol,
// including its library.
template <typename Protocol>
constexpr const char* DiscoverableProtocolName = Protocol::kDiscoverableName;

// DiscoverableProtocolDefaultPath<Protocol> evaluates to a string containing the default path for
// the protocol endpoint, something like "/svc/fuchsia.library.Protocol".
template <typename Protocol>
constexpr const char* DiscoverableProtocolDefaultPath = fidl::internal::default_service_path<
    Protocol::kDiscoverableName,
    std::make_integer_sequence<size_t, sizeof(Protocol::kDiscoverableName)>>::value;

}  // namespace fidl

#endif  // LIB_FIDL_CPP_WIRE_CONNECT_SERVICE_H_
