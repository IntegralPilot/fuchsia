// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_NAMESPACE_NAMESPACE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_NAMESPACE_NAMESPACE_H_

#include <lib/component/incoming/cpp/constants.h>
#include <lib/fidl/cpp/client.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <string_view>
#include <type_traits>

namespace display {

// fdf::Namespace subset that eases migration from DFv1 to DFv2.
//
// TODO(https://fxbug.dev/323061435): This class should be only used when the
// display drivers are being migrated to DFv2. Drivers should replace
// `display::Namespace` with `fdf::Namespace` when the migration is done.
class Namespace {
 public:
  Namespace() = default;
  virtual ~Namespace() = default;

  Namespace(const Namespace&) = delete;
  Namespace(Namespace&&) = delete;
  Namespace& operator=(const Namespace&) = delete;
  Namespace& operator=(Namespace&&) = delete;

  // Equivalent to `fdf::Namespace::Connect(instance)`.
  //
  // `ServiceMember` protocol must use channel transport.
  template <typename ServiceMember>
  zx::result<fidl::ClientEnd<typename ServiceMember::ProtocolType>> Connect(
      std::string_view instance = component::kDefaultInstance) const;

  // Equivalent to `fdf::Namespace::Connect(server_end, instance)`.
  //
  // `ServiceMember` protocol must use channel transport.
  template <typename ServiceMember>
  zx::result<> Connect(fidl::ServerEnd<typename ServiceMember::ProtocolType> server_end,
                       std::string_view instance = component::kDefaultInstance) const;

 private:
  // Connects `server_end` to the FIDL `service_member` protocol in the
  // `instance` of the `service`.
  //
  // `server_end` must be valid.
  virtual zx::result<> ConnectServerEndToFidlProtocol(zx::channel server_end,
                                                      std::string_view service,
                                                      std::string_view service_member,
                                                      std::string_view instance) const = 0;

  // Connects to the FIDL `service_member` protocol in the `instance` of the
  // `service`, and returns the client end channel of the protocol on success.
  zx::result<zx::channel> ConnectToFidlProtocol(std::string_view service,
                                                std::string_view service_member,
                                                std::string_view instance) const;
};

template <typename ServiceMember>
zx::result<fidl::ClientEnd<typename ServiceMember::ProtocolType>> Namespace::Connect(
    std::string_view instance) const {
  static_assert(fidl::IsServiceMemberV<ServiceMember>,
                "ServiceMember type must be the Protocol inside of a Service.");
  static_assert(std::is_same_v<typename ServiceMember::ProtocolType::Transport,
                               fidl::internal::ChannelTransport>,
                "NamespaceAdapter only supports FIDL services using channel transport.");

  zx::result<zx::channel> client_channel_result =
      ConnectToFidlProtocol(ServiceMember::ServiceName, ServiceMember::Name, instance);

  if (client_channel_result.is_error()) {
    return client_channel_result.take_error();
  }

  return zx::ok(fidl::ClientEnd<typename ServiceMember::ProtocolType>(
      std::move(client_channel_result).value()));
}

template <typename ServiceMember>
zx::result<> Namespace::Connect(fidl::ServerEnd<typename ServiceMember::ProtocolType> server_end,
                                std::string_view instance) const {
  static_assert(fidl::IsServiceMemberV<ServiceMember>,
                "ServiceMember type must be the Protocol inside of a Service.");
  static_assert(std::is_same_v<typename ServiceMember::ProtocolType::Transport,
                               fidl::internal::ChannelTransport>,
                "NamespaceAdapter only supports FIDL services using channel transport.");

  return ConnectServerEndToFidlProtocol(server_end.TakeChannel(), ServiceMember::ServiceName,
                                        ServiceMember::Name, instance);
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_NAMESPACE_NAMESPACE_H_
