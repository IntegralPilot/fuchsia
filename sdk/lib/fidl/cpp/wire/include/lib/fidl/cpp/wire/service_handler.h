// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_WIRE_SERVICE_HANDLER_H_
#define LIB_FIDL_CPP_WIRE_SERVICE_HANDLER_H_

#include <lib/fidl/cpp/features.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fit/function.h>
#include <zircon/fidl.h>

#include <map>
#include <string>
#include <string_view>

#if __FIDL_SUPPORT_HANDLES
#include <lib/fidl/cpp/wire/internal/endpoints.h>
#include <lib/fidl/cpp/wire/internal/transport.h>
#include <lib/zx/result.h>
#endif  // __FIDL_SUPPORT_HANDLES

namespace fidl {

#if __FIDL_SUPPORT_HANDLES

template <typename Protocol>
using ProtocolHandler = fit::function<void(fidl::internal::ServerEndType<Protocol>)>;

// Class used by generated FIDL code for adding protocol members to a Service instance.
template <class Transport>
class ServiceInstanceHandler {
 public:
  using TransportType = typename Transport::OwnedType;

  // User-defined action for handling a connection attempt to a
  // member FIDL protocol defined by |Protocol|.
  // For example, if |Protocol| is spoken over Zircon channels, the handler takes a
  // |fidl::ServerEnd<Protocol>|.
  template <typename Protocol>
  using MemberHandler = ProtocolHandler<Protocol>;

  // User-defined action for handling a connection attempt to any
  // member FIDL protocol.
  using AnyMemberHandler = fit::function<void(TransportType)>;

  ServiceInstanceHandler() = default;
  ~ServiceInstanceHandler() = default;

  // Disallow copying.
  ServiceInstanceHandler(const ServiceInstanceHandler&) = delete;
  ServiceInstanceHandler& operator=(const ServiceInstanceHandler&) = delete;

  // Enable moving.
  ServiceInstanceHandler(ServiceInstanceHandler&&) noexcept = default;
  ServiceInstanceHandler& operator=(ServiceInstanceHandler&&) noexcept = default;

  // Add a |member| to the instance, which will be handled by |handler|.
  //
  // This method specifies the exact protocol |Protocol|, hence should be
  // used by end-users adding service member handlers to a service directory.
  //
  // # Errors
  //
  // ZX_ERR_ALREADY_EXISTS: The member already exists.
  // ZX_ERR_INVALID_ARGS: |handler| is null.
  template <typename Protocol>
  zx::result<> AddMember(MemberHandler<Protocol> handler, std::string_view member) {
    if (!handler) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    auto bridge_func = [handler = std::move(handler)](TransportType channel) {
      return handler(fidl::internal::ServerEndType<Protocol>(std::move(channel)));
    };
    return AddAnyMember(std::move(bridge_func), member);
  }

  // Add a |member| to the instance, which will be handled by |handler|.
  //
  // This method does not specify the exact protocol so it can be used to add a handler
  // when its protocol is unknown.
  //
  // # Errors
  //
  // ZX_ERR_ALREADY_EXISTS: The member already exists.
  // ZX_ERR_INVALID_ARGS: |handler| is null.
  zx::result<> AddAnyMember(AnyMemberHandler handler, std::string_view member) {
    if (!handler) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    std::string owned_member = std::string(member);
    if (handlers_.count(owned_member) != 0) {
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }
    handlers_[owned_member] = std::move(handler);
    return zx::ok();
  }

  // Return all registered member handlers. Key contains member name. Value
  // contains connector functions.
  //
  // Once taken, the `ServiceInstanceHandler` is no longer safe to use.
  std::map<std::string, AnyMemberHandler> TakeMemberHandlers() { return std::move(handlers_); }

 private:
  std::map<std::string, AnyMemberHandler> handlers_ = {};
};

#endif  // __FIDL_SUPPORT_HANDLES

}  // namespace fidl

#endif  // LIB_FIDL_CPP_WIRE_SERVICE_HANDLER_H_
