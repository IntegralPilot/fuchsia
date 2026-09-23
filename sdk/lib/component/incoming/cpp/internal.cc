// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/component/incoming/cpp/constants.h>
#include <lib/component/incoming/cpp/internal.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <zircon/assert.h>

#include <cstring>

namespace component::internal {

namespace {

namespace fio = fuchsia_io;

constexpr size_t kMaxComponentLength = fio::wire::kMaxNameLength;

// Buffer used to hold the maximum possible path of a service instance. A service instance path is
// made from two path components, each separated by '/', and must be terminated with a null byte.
using ServiceInstancePathBuffer = char[(2 * kMaxComponentLength) + 1 + 1];

// Combines `service` and `instance` with a '/' separator. Guarantees the result ends with a null
// byte so it can be safely passed to functions that require C strings.
zx::result<std::string_view> MakeInstancePath(ServiceInstancePathBuffer& buffer,
                                              std::string_view service, std::string_view instance) {
  if (service.empty() || service.size() > kMaxComponentLength) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (instance.size() > kMaxComponentLength) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (service[0] == '/') {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  const uint64_t path_size = service.size() + instance.size() + 1;
  ZX_ASSERT(path_size < sizeof buffer);
  std::memcpy(buffer, service.data(), service.size());
  buffer[service.size()] = '/';
  std::memcpy(buffer + service.size() + 1, instance.data(), instance.size());
  buffer[path_size] = '\0';
  return zx::ok(std::string_view(buffer, path_size));
}

zx::result<fio::wire::Flags> EnsureDirectoryProtocol(fio::wire::Flags flags) {
  const fio::wire::Flags protocols = flags & fio::wire::kMaskKnownProtocols;
  if (protocols && (protocols != fio::wire::Flags::kProtocolDirectory)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(flags | fio::wire::Flags::kProtocolDirectory);
}

}  // namespace

zx::result<> ConnectRaw(zx::channel server_end, std::string_view path) {
  std::string owned_path(path);
  if (zx_status_t status = fdio_service_connect(owned_path.c_str(), server_end.release());
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok();
}

zx::result<> ConnectAtRaw(fidl::UnownedClientEnd<fio::Directory> svc_dir, zx::channel server_end,
                          std::string_view protocol_name) {
  std::string path(protocol_name);
  if (zx_status_t status =
          fdio_service_connect_at(svc_dir.handle()->get(), path.c_str(), server_end.release());
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok();
}

zx::result<fidl::ClientEnd<fio::Directory>> OpenDirectory(std::string_view path,
                                                          fio::wire::Flags flags) {
  zx::result directory_flags = EnsureDirectoryProtocol(flags);
  if (directory_flags.is_error()) {
    return directory_flags.take_error();
  }
  auto [client, server] = fidl::Endpoints<fio::Directory>::Create();
  std::string owned_path(path);
  if (zx_status_t status = fdio_open3(owned_path.c_str(), uint64_t{*directory_flags},
                                      server.TakeChannel().release());
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(client));
}

zx::result<fidl::ClientEnd<fio::Directory>> OpenDirectoryAt(
    fidl::UnownedClientEnd<fio::Directory> dir, std::string_view path, fio::wire::Flags flags) {
  zx::result directory_flags = EnsureDirectoryProtocol(flags);
  if (directory_flags.is_error()) {
    return directory_flags.take_error();
  }
  auto [client, server] = fidl::Endpoints<fio::Directory>::Create();
  std::string owned_path(path);
  if (zx_status_t status =
          fdio_open3_at(dir.handle()->get(), owned_path.c_str(), uint64_t{*directory_flags},
                        server.TakeChannel().release());
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(client));
}

zx::result<> CloneRaw(fidl::UnownedClientEnd<fuchsia_unknown::Cloneable>&& cloneable,
                      zx::channel server_end) {
  const fidl::Status result = fidl::WireCall(cloneable)->Clone(
      fidl::ServerEnd<fuchsia_unknown::Cloneable>(std::move(server_end)));
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok();
}

zx::result<> OpenNamedServiceRaw(std::string_view service, std::string_view instance,
                                 zx::channel remote) {
  auto client = OpenServiceRoot();
  if (client.is_error()) {
    return client.take_error();
  }
  return OpenNamedServiceAtRaw(*client, service, instance, std::move(remote));
}

zx::result<> OpenNamedServiceAtRaw(fidl::UnownedClientEnd<fio::Directory> dir,
                                   std::string_view service, std::string_view instance,
                                   zx::channel remote) {
  ServiceInstancePathBuffer path_buffer;
  zx::result<std::string_view> path = MakeInstancePath(path_buffer, service, instance);
  if (!path.is_ok()) {
    return path.take_error();
  }
  return zx::make_result(fdio_open3_at(dir.channel()->get(), path->data(),
                                       uint64_t{kServiceRootFlags}, remote.release()));
}

zx::result<> ProtocolOpenFunc(zx::unowned_channel dir, fidl::StringView path,
                              fidl::internal::AnyTransport remote) {
  std::string owned_path(path.get());
  return zx::make_result(
      fdio_service_connect_at(dir->get(), owned_path.c_str(),
                              remote.release<fidl::internal::ChannelTransport>().release()));
}

}  // namespace component::internal
