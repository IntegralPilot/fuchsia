// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_COMPONENT_INCOMING_CPP_PROTOCOL_H_
#define LIB_COMPONENT_INCOMING_CPP_PROTOCOL_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/component/incoming/cpp/internal.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <string_view>
#include <type_traits>
#include <utility>

namespace component {

// Opens the directory containing incoming services in the component's incoming
// namespace.
//
// `path` must be absolute, containing a leading "/". Defaults to "/svc".
//
// # Errors
//
//   * `ZX_ERR_BAD_PATH`: `path` is too long.
//   * `ZX_ERR_NOT_FOUND`: `path` was not found in the incoming namespace.
zx::result<fidl::ClientEnd<fuchsia_io::Directory>> OpenServiceRoot(
    std::string_view path = component::kServiceDirectory);

// Connects to the FIDL Protocol in the component's incoming namespace.
//
// `server_end` is the channel used for the server connection.
//
// `path` must be absolute, containing a leading "/". Default to "/svc/{name}"
// where `{name}` is the fully qualified name of the FIDL Protocol.
//
// The operation completes asynchronously, which means a zx::ok() result does
// not ensure that the requested protocol actually exists.
//
// # Errors
//
//   * `ZX_ERR_INVALID_ARGS`: `path` is too long.
//   * `ZX_ERR_NOT_FOUND`: A prefix of `path` cannot be found in the namespace.
template <typename Protocol, typename = std::enable_if_t<fidl::IsProtocolV<Protocol>>>
zx::result<> Connect(fidl::ServerEnd<Protocol> server_end,
                     std::string_view path = fidl::DiscoverableProtocolDefaultPath<Protocol>) {
  internal::EnsureCanConnectToProtocol<Protocol>();
  return internal::ConnectRaw(server_end.TakeChannel(), path);
}

// Connects to the FIDL Protocol in the component's incoming namespace and
// returns a client end.
//
// `path` must be absolute, containing a leading "/". Default to "/svc/{name}"
// where `{name}` is the fully qualified name of the FIDL Protocol.
//
// The operation completes asynchronously, which means a zx::ok() result does
// not ensure that the requested protocol actually exists.
//
// # Errors
//
//   * `ZX_ERR_INVALID_ARGS`: `path` is too long.
//   * `ZX_ERR_NOT_FOUND`: A prefix of `path` cannot be found in the namespace.
template <typename Protocol, typename = std::enable_if_t<fidl::IsProtocolV<Protocol>>>
zx::result<fidl::ClientEnd<Protocol>> Connect(
    std::string_view path = fidl::DiscoverableProtocolDefaultPath<Protocol>) {
  internal::EnsureCanConnectToProtocol<Protocol>();
  auto [client, server] = fidl::Endpoints<Protocol>::Create();
  if (auto result = Connect<Protocol>(std::move(server), path); result.is_error()) {
    return result.take_error();
  }
  return zx::ok(std::move(client));
}

// Connects to the FIDL Protocol in the directory `svc_dir`.
//
// `server_end` is the channel used for the server connection.
//
// `name` must be a valid entry in `svc_dir`.
//
// The operation completes asynchronously, which means a zx::ok() result does
// not ensure that the requested protocol actually exists.
//
// # Errors
//
//   * `ZX_ERR_INVALID_ARGS`: `name` is too long.
template <typename Protocol, typename = std::enable_if_t<fidl::IsProtocolV<Protocol>>>
zx::result<> ConnectAt(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir,
                       fidl::ServerEnd<Protocol> server_end,
                       std::string_view name = fidl::DiscoverableProtocolName<Protocol>) {
  internal::EnsureCanConnectToProtocol<Protocol>();
  return internal::ConnectAtRaw(svc_dir, server_end.TakeChannel(), name);
}

// Connects to the FIDL Protocol in the directory `svc_dir`. Returns a client
// end to the protocol connection.
//
// `name` must be a valid entry in `svc_dir`.
//
// The operation completes asynchronously, which means a zx::ok() result does
// not ensure that the requested protocol actually exists.
//
// # Errors
//
//   * `ZX_ERR_INVALID_ARGS`: `name` is too long.
template <typename Protocol, typename = std::enable_if_t<fidl::IsProtocolV<Protocol>>>
zx::result<fidl::ClientEnd<Protocol>> ConnectAt(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir,
    std::string_view name = fidl::DiscoverableProtocolName<Protocol>) {
  internal::EnsureCanConnectToProtocol<Protocol>();
  auto [client, server] = fidl::Endpoints<Protocol>::Create();
  if (auto result = ConnectAt<Protocol>(svc_dir, std::move(server), name); result.is_error()) {
    return result.take_error();
  }
  return zx::ok(std::move(client));
}

}  // namespace component

#endif  // LIB_COMPONENT_INCOMING_CPP_PROTOCOL_H_
