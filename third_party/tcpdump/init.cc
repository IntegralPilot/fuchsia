// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.posix.socket.packet/cpp/wire.h>
#include <fidl/fuchsia.sys2/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/vfs/cpp/composed_service_dir.h>
#include <lib/zx/channel.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <filesystem>

namespace {

constexpr char kRealmQueryPath[] = "/svc/fuchsia.sys2.RealmQuery.root";
constexpr char kNetstackMoniker[] = "./core/network/netstack";
constexpr char kRootDirectory[] = "/";
constexpr char kServiceDirectory[] = "/svc";
constexpr const char* kPacketSocketProviderName =
    fidl::DiscoverableProtocolName<fuchsia_posix_socket_packet::Provider>;
constexpr fuchsia_io::Flags kServeFlags = fuchsia_io::wire::kPermReadable;

}  // namespace

// Attempts to make a packet socket provider available to this program if not
// already available.
//
// The packet socket provider exposed by the core realm's netstack is used if it
// is available.
__attribute__((constructor)) void init_packet_socket_provider() {
  std::filesystem::path svc_dir_path(kServiceDirectory);

  if (std::filesystem::exists(svc_dir_path / kPacketSocketProviderName)) {
    // Packet socket provider is already available.
    return;
  }

  static async::Loop composed_dir_loop(&kAsyncLoopConfigNoAttachToCurrentThread);

  // Replace the default service directory with our composed service directory
  // to make packet socket provider available to the program and start serving
  // requests to the composed service directory.
  {
    fdio_ns_t* ns;
    {
      zx_status_t status = fdio_ns_get_installed(&ns);
      ZX_ASSERT_MSG(status == ZX_OK, "fdio_ns_get_installed(_): %s", zx_status_get_string(status));
    }

    auto bind_to_ns = [ns](const char* path, vfs::ComposedServiceDir& composed_dir) {
      auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
      {
        zx_status_t status = fdio_ns_bind(ns, path, client.TakeChannel().release());
        ZX_ASSERT_MSG(status == ZX_OK, "fdio_ns_bind(_, %s, _): %s", path,
                      zx_status_get_string(status));
      }
      {
        zx_status_t status =
            composed_dir.Serve(kServeFlags, std::move(server), composed_dir_loop.dispatcher());
        ZX_ASSERT_MSG(status == ZX_OK, "Serve failed: %s", zx_status_get_string(status));
      }
    };

    static vfs::ComposedServiceDir composed_svc_dir;

    // Our composed service directory should be a superset of the default service
    // directory.
    {
      zx::result original_svc_dir = component::OpenServiceRoot();
      switch (zx_status_t status = original_svc_dir.status_value(); status) {
        case ZX_OK:
          break;
        case ZX_ERR_NOT_FOUND:
          // Environment didn't populate a service directory for us to use as a
          // fallback.
          return;
        default:
          ZX_PANIC("component::OpenServiceRoot(): %s", zx_status_get_string(status));
      }
      composed_svc_dir.SetFallback(std::move(*original_svc_dir));
    }

    // Add the packet socket provider service to our composed service directory
    // by reaching into netstack's exposed directory.
    {
      zx::result realm_query = component::Connect<fuchsia_sys2::RealmQuery>(kRealmQueryPath);
      ZX_ASSERT_MSG(realm_query.is_ok(), "Failed to connect to %s: %s", kRealmQueryPath,
                    realm_query.status_string());
      auto [exposed_client, exposed_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
      fidl::WireResult open_dir =
          fidl::WireCall(realm_query.value())
              ->OpenDirectory(kNetstackMoniker, fuchsia_sys2::OpenDirType::kExposedDir,
                              std::move(exposed_server));
      ZX_ASSERT_MSG(open_dir.ok(), "Failed to open %s: %s", kNetstackMoniker,
                    open_dir.status_string());

      composed_svc_dir.AddService(
          kPacketSocketProviderName,
          std::make_unique<vfs::Service>(
              [exposed_client = std::move(exposed_client)](
                  fidl::ServerEnd<fuchsia_posix_socket_packet::Provider> server_end) mutable {
                zx::result result =
                    component::ConnectAt(exposed_client.borrow(), std::move(server_end));
                ZX_ASSERT_MSG(result.is_ok(), "Failed to connect to packet socker provider: %s",
                              result.status_string());
              }));
    }

    // Attempt to unbind the service directory from the namespace so we can
    // replace it.
    switch (zx_status_t status = fdio_ns_unbind(ns, svc_dir_path.c_str()); status) {
      // We get |ZX_OK| when the namespace the service directory is a mount
      // point.
      case ZX_OK:
        bind_to_ns(svc_dir_path.c_str(), composed_svc_dir);
        break;
      // We get |ZX_ERR_BAD_PATH| when the service directory isn't a mount point
      // in the namespace (process launched with delayed directories after
      // https://fuchsia.googlesource.com/fuchsia/+/82ad8d81396d5659515e830a7364cf33b1605b69).
      case ZX_ERR_BAD_PATH: {
        std::filesystem::path root(kRootDirectory);
        static vfs::ComposedServiceDir composed_root_dir;
        {
          auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
          {
            zx_status_t status =
                fdio_open3(root.c_str(), uint64_t{kServeFlags}, server.TakeChannel().release());
            ZX_ASSERT_MSG(status == ZX_OK, "fdio_open3(%s, ...): %s", root.c_str(),
                          zx_status_get_string(status));
          }
          composed_root_dir.SetFallback(std::move(client));
        }

        composed_root_dir.AddService(
            svc_dir_path.filename().c_str(),
            std::make_unique<vfs::Service>(
                [](fidl::ServerEnd<fuchsia_io::Directory> request) mutable {
                  zx_status_t status = composed_svc_dir.Serve(kServeFlags, std::move(request));
                  ZX_ASSERT_MSG(status == ZX_OK, "Serve failed: %s", zx_status_get_string(status));
                }));

        {
          zx_status_t status = fdio_ns_unbind(ns, root.c_str());
          ZX_ASSERT_MSG(status == ZX_OK, "fdio_ns_unbind(_, %s): %s", root.c_str(),
                        zx_status_get_string(status));
        }

        bind_to_ns(root.c_str(), composed_root_dir);
      } break;
      default:
        ZX_PANIC("fdio_ns_unbind(_, %s): %s", svc_dir_path.c_str(), zx_status_get_string(status));
    }
  }

  zx_status_t status = composed_dir_loop.StartThread();
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to start async loop thread: %s",
                zx_status_get_string(status));
}
