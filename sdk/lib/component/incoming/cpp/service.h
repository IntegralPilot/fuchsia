// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_COMPONENT_INCOMING_CPP_SERVICE_H_
#define LIB_COMPONENT_INCOMING_CPP_SERVICE_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.unknown/cpp/wire.h>
#include <lib/component/incoming/cpp/constants.h>
#include <lib/component/incoming/cpp/internal.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <type_traits>
#include <utility>

namespace component {

// Opens a connection to the given instance of the provided FIDL service. The
// result, if successful, is a `Service::ServiceClient` that exposes methods
// that connect to the various members of the FIDL service.
//
// `instance ` must be an entry in incoming namespace's service directory.
// Defaults to `default`.
//
// # Errors
//
//   * `ZX_ERR_NOT_FOUND`: `instance` was not found in the incoming namespace.
//   * `ZX_ERR_INVALID_ARGS`: `instance` is more than 255 characters long.
//
// # Example
//
// ```C++
// using Echo = fuchsia_echo::Echo;
// using EchoService = fuchsia_echo::EchoService;
//
// zx::result<EchoService::ServiceClient> open_result =
//     component::OpenService<EchoService>();
// ASSERT_TRUE(open_result.is_ok());
//
// EchoService::ServiceClient service = open_result.take_value();
//
// zx::result<fidl::ClientEnd<Echo>> connect_result = service.ConnectFoo();
// ASSERT_TRUE(connect_result.is_ok());
//
// fidl::WireSyncClient<Echo> client{connect_result.take_value()};
// ```
template <typename Service, typename = std::enable_if_t<fidl::IsServiceV<Service>>>
zx::result<typename Service::ServiceClient> OpenService(
    std::string_view instance = kDefaultInstance) {
  zx::channel local, remote;
  if (zx_status_t status = zx::channel::create(0, &local, &remote); status != ZX_OK) {
    return zx::error(status);
  }

  zx::result<> result = internal::OpenNamedServiceRaw(Service::Name, instance, std::move(remote));
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok(typename Service::ServiceClient(std::move(local), internal::ProtocolOpenFunc));
}

// Opens a connection to the given instance of the provide FIDL service. The
// result, if successful, is a `Service::ServiceClient` that exposes methods
// that connect to the various members of the FIDL service.
//
// `instance ` must be an entry in the directory `svc_dir`. Defaults to
// `default`.
//
// # Errors
//
//   * `ZX_ERR_NOT_FOUND`: `instance` was not found in the incoming namespace.
//   * `ZX_ERR_INVALID_ARGS`: `instance` is more than 255 characters long.
template <typename Service, typename = std::enable_if_t<fidl::IsServiceV<Service>>>
zx::result<typename Service::ServiceClient> OpenServiceAt(
    fidl::UnownedClientEnd<fuchsia_io::Directory> dir,
    std::string_view instance = kDefaultInstance) {
  zx::channel local, remote;
  if (zx_status_t status = zx::channel::create(0, &local, &remote); status != ZX_OK) {
    return zx::error(status);
  }

  zx::result<> result =
      internal::OpenNamedServiceAtRaw(dir, Service::Name, instance, std::move(remote));
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok(typename Service::ServiceClient(std::move(local), internal::ProtocolOpenFunc));
}

// Gets the relative path to a service member.
//
// This is in the format of:
// `ServiceName/instance/Name`
//
// This relative path can be appended to `/svc/` to construct a full path.
template <typename ServiceMember,
          typename = std::enable_if_t<fidl::IsServiceMemberV<ServiceMember>>>
std::string MakeServiceMemberPath(std::string_view instance) {
  return std::string(ServiceMember::ServiceName)
      .append("/")
      .append(instance)
      .append("/")
      .append(ServiceMember::Name);
}

// Connects to the FIDL Protocol of the provided |ServiceMember| in the
// directory `svc_dir`. Specifically, the FIDL Protocol used is one pointed to
// |ServiceMember::ProtocolType|.
//
// `server_end` is the channel used for the server connection.
//
// `instance` is an instance name in the directory `svc_dir`.
//
// # Errors
//
//   * `ZX_ERR_BAD_PATH`: `instance` is too long.
//   * `ZX_ERR_NOT_FOUND`: No entry was found using the provided `instance`.
template <typename ServiceMember,
          typename = std::enable_if_t<fidl::IsServiceMemberV<ServiceMember>>>
zx::result<> ConnectAtMember(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir,
                             fidl::ServerEnd<typename ServiceMember::ProtocolType> server_end,
                             std::string_view instance = kDefaultInstance) {
  auto path = MakeServiceMemberPath<ServiceMember>(instance);
  return ConnectAt<typename ServiceMember::ProtocolType>(svc_dir, std::move(server_end), path);
}

// Connects to the FIDL Protocol of the provided |ServiceMember| in the
// directory `svc_dir`. Specifically, the FIDL Protocol used is one pointed to
// |ServiceMember::ProtocolType|. Returns a client end to the protocol
// connection.
//
// `server_end` is the channel used for the server connection.
//
// `instance` is an instance name in the directory `svc_dir`.
//
// # Errors
//
//   * `ZX_ERR_BAD_PATH`: `instance` is too long.
//   * `ZX_ERR_NOT_FOUND`: No entry was found using the provided `instance`.
template <typename ServiceMember,
          typename = std::enable_if_t<fidl::IsServiceMemberV<ServiceMember>>>
zx::result<fidl::ClientEnd<typename ServiceMember::ProtocolType>> ConnectAtMember(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir,
    std::string_view instance = kDefaultInstance) {
  auto endpoints = fidl::CreateEndpoints<typename ServiceMember::ProtocolType>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }

  if (auto result = ConnectAtMember<ServiceMember>(svc_dir, std::move(endpoints->server), instance);
      result.is_error()) {
    return result.take_error();
  }

  return zx::ok(std::move(endpoints->client));
}

}  // namespace component

#endif  // LIB_COMPONENT_INCOMING_CPP_SERVICE_H_
