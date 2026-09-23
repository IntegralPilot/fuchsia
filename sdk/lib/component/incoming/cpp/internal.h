// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_COMPONENT_INCOMING_CPP_INTERNAL_H_
#define LIB_COMPONENT_INCOMING_CPP_INTERNAL_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.unknown/cpp/wire.h>
#include <lib/component/incoming/cpp/constants.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <type_traits>
#include <utility>

namespace component::internal {

// Implementation of |component::Connect| that delegates to |fdio_service_connect|.
zx::result<> ConnectRaw(zx::channel server_end, std::string_view path);

// Implementation of |component::ConnectAt| for a service directory that delegates to
// |fdio_service_connect_at|.
zx::result<> ConnectAtRaw(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir,
                          zx::channel server_end, std::string_view protocol_name);

// Implementation of |component::OpenDirectory| that delegates to |fdio_open3|.
zx::result<fidl::ClientEnd<fuchsia_io::Directory>> OpenDirectory(std::string_view path,
                                                                 fuchsia_io::wire::Flags flags);

// Implementation of |component::OpenDirectoryAt| that delegates to |fdio_open3_at|.
zx::result<fidl::ClientEnd<fuchsia_io::Directory>> OpenDirectoryAt(
    fidl::UnownedClientEnd<fuchsia_io::Directory> dir, std::string_view path,
    fuchsia_io::wire::Flags flags);

// Implementation of |component::Clone| for |fuchsia.unknown/Cloneable|.
zx::result<> CloneRaw(fidl::UnownedClientEnd<fuchsia_unknown::Cloneable>&& cloneable,
                      zx::channel server_end);

template <typename Protocol>
zx::result<zx::channel> CloneRaw(fidl::UnownedClientEnd<Protocol>&& client) {
  zx::channel client_end, server_end;
  if (zx_status_t status = zx::channel::create(0, &client_end, &server_end); status != ZX_OK) {
    return zx::error(status);
  }
  if (zx::result<> status = CloneRaw(std::move(client), std::move(server_end)); status.is_error()) {
    return status.take_error();
  }
  return zx::ok(std::move(client_end));
}

// Implementation of |component::OpenService| that is independent from the
// actual |Service|.
zx::result<> OpenNamedServiceRaw(std::string_view service, std::string_view instance,
                                 zx::channel remote);

// Implementation of |component::OpenServiceAt| that is independent from the
// actual |Service|.
zx::result<> OpenNamedServiceAtRaw(fidl::UnownedClientEnd<fuchsia_io::Directory> dir,
                                   std::string_view service_path, std::string_view instance,
                                   zx::channel remote);

// The internal |ProtocolOpenFunc| needs to take raw Zircon channels because
// the FIDL runtime that interfaces with it cannot depend on the |fuchsia.io|
// FIDL library.
zx::result<> ProtocolOpenFunc(zx::unowned_channel dir, fidl::StringView path,
                              fidl::internal::AnyTransport remote);

zx::result<fidl::ClientEnd<fuchsia_io::Directory>> GetGlobalServiceDirectory();

// Determines if |Protocol| contains the |fuchsia.unknown/Cloneable.Clone| method.
template <typename Protocol, typename = void>
struct has_fidl_method_fuchsia_unknown_clone : public ::std::false_type {};

template <typename Protocol>
struct has_fidl_method_fuchsia_unknown_clone<
    Protocol, std::void_t<decltype(fidl::WireRequest<typename Protocol::Clone>{
                  std::declval<fidl::ServerEnd<fuchsia_unknown::Cloneable>&&>() /* request */})>>
    : public std::true_type {};

template <typename Protocol>
constexpr inline auto has_fidl_method_fuchsia_unknown_clone_v =
    has_fidl_method_fuchsia_unknown_clone<Protocol>::value;

// Determines if |T| is fully defined i.e. |sizeof(T)| can be evaluated.
template <typename T, typename = void>
struct is_complete : public ::std::false_type {};
template <typename T>
struct is_complete<T, std::void_t<std::integral_constant<std::size_t, sizeof(T)>>>
    : public std::true_type {};
template <typename T>
constexpr inline auto is_complete_v = is_complete<T>::value;

// Ensures that |Protocol| is *not* a fuchsia.io protocol. Unlike most services/protocols,
// fuchsia.io connections require a set of flags to be passed during opening that set the expected
// rights on the resulting connection.
//
// This is not a type trait so that we can provide a consistent error message.
template <typename Protocol>
constexpr void EnsureCanConnectToProtocol() {
  constexpr bool is_directory_protocol = std::is_same_v<Protocol, fuchsia_io::Directory>;
  constexpr bool is_other_node_protocol = std::is_same_v<Protocol, fuchsia_io::Node> ||
                                          std::is_same_v<Protocol, fuchsia_io::File> ||
                                          std::is_same_v<Protocol, fuchsia_io::Symlink>;
  static_assert(!is_directory_protocol,
                "Use component::OpenDirectory or component::OpenDirectoryAt to open a directory.");
  static_assert(!is_other_node_protocol, "Use std::filesystem or fdio to open a file/symlink.");
}

}  // namespace component::internal

#endif  // LIB_COMPONENT_INCOMING_CPP_INTERNAL_H_
