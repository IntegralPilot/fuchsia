// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/netsvc/netifc-discover.h"

#include <fcntl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/wait.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>
#include <lib/stdcompat/string_view.h>
#include <stdio.h>

#include <fbl/unique_fd.h>
#include <src/devices/lib/client/device_topology.h>

#include "src/bringup/bin/netsvc/match.h"
#include "src/lib/fsl/io/device_watcher.h"

namespace {

std::string_view SkipInstanceSigil(std::string_view v) {
  if (!v.empty() && v.at(0) == '@') {
    return v.substr(1);
  }
  return v;
}

struct Netdevice {
  struct Info {
    fidl::ClientEnd<fuchsia_hardware_network::Device> device;
    fidl::ClientEnd<fuchsia_hardware_network::PortWatcher> port_watcher;
  };
  static constexpr const char* kDirectory = "/class/network";

  static std::optional<Info> get_interface_if_matching(
      fidl::ClientEnd<fuchsia_hardware_network::Device> client_end, const std::string& filename) {
    auto [watcher_client_end, watcher_server_end] =
        fidl::Endpoints<fuchsia_hardware_network::PortWatcher>::Create();

    {
      fidl::Status result =
          fidl::WireCall(client_end)->GetPortWatcher(std::move(watcher_server_end));
      if (!result.ok()) {
        printf("netifc: failed to get port watcher %s: %s\n", filename.c_str(),
               result.status_string());
        return std::nullopt;
      }
    }

    return Info{
        .device = std::move(client_end),
        .port_watcher = std::move(watcher_client_end),
    };
  }

  static void Process(std::optional<NetdeviceInterface>& discovered, async_dispatcher_t* dispatcher,
                      Info info) {
    auto [device, watcher] = std::move(info);
    std::shared_ptr client_ptr =
        std::make_shared<fidl::WireClient<fuchsia_hardware_network::PortWatcher>>(
            std::move(watcher), dispatcher);

    Watch(discovered, client_ptr, std::move(device));
  }

  static void Watch(
      std::optional<NetdeviceInterface>& discovered,
      const std::shared_ptr<fidl::WireClient<fuchsia_hardware_network::PortWatcher>>& watcher,
      fidl::ClientEnd<fuchsia_hardware_network::Device> dev) {
    (*watcher)->Watch().ThenExactlyOnce(
        [&discovered, watcher, dev = std::move(dev)](
            fidl::WireUnownedResult<fuchsia_hardware_network::PortWatcher::Watch>& r) mutable {
          if (!r.ok()) {
            printf("netifc: failed to watch for netdevice ports: %s\n",
                   r.FormatDescription().c_str());
            return;
          }

          auto defer = fit::defer([&discovered, &watcher, &dev]() {
            // Watch next.
            Watch(discovered, watcher, std::move(dev));
          });

          const fuchsia_hardware_network::wire::DevicePortEvent& event = r.value().event;
          fuchsia_hardware_network::wire::PortId port_id;
          switch (event.Which()) {
            case fuchsia_hardware_network::wire::DevicePortEvent::Tag::kAdded:
              port_id = event.added();
              break;
            case fuchsia_hardware_network::wire::DevicePortEvent::Tag::kExisting:
              port_id = event.existing();
              break;
            case fuchsia_hardware_network::wire::DevicePortEvent::Tag::kIdle:
            case fuchsia_hardware_network::wire::DevicePortEvent::Tag::kRemoved:
              return;
          }

          auto [port_client_end, port_server_end] =
              fidl::Endpoints<fuchsia_hardware_network::Port>::Create();
          {
            fidl::Status result = fidl::WireCall(dev)->GetPort(port_id, std::move(port_server_end));
            if (!result.ok()) {
              printf("netifc: failed to get netdevice port (%d:%d): %s\n", port_id.base,
                     port_id.salt, result.FormatDescription().c_str());
              return;
            }
          }
          {
            fidl::WireResult result = fidl::WireCall(port_client_end)->GetInfo();
            if (!result.ok()) {
              printf("netifc: failed to get netdevice port info (%d:%d): %s\n", port_id.base,
                     port_id.salt, result.FormatDescription().c_str());
              return;
            }
            const fuchsia_hardware_network::wire::PortInfo& port_info = result.value().info;
            if (!(port_info.has_base_info() && port_info.base_info().has_port_class())) {
              printf("netifc: missing port class in netdevice port info (%d:%d): %s\n",
                     port_id.base, port_id.salt, result.FormatDescription().c_str());
              return;
            }
            switch (port_info.base_info().port_class()) {
              case fuchsia_hardware_network::wire::PortClass::kEthernet:
              case fuchsia_hardware_network::wire::PortClass::kVirtual:
                // NB: Historically this only netifc only accepts Ethernet
                // device. We allow virtual interfaces as well which are used in
                // testing.
                break;
              default:
                printf("netifc: ignoring netdevice port (%d:%d) with class %hu\n", port_id.base,
                       port_id.salt,
                       static_cast<unsigned short>(port_info.base_info().port_class()));
                return;
            }
          }

          // This is a good candidate port, but we need to retrieve the MAC
          // address.
          auto [mac_client_end, mac_server_end] =
              fidl::Endpoints<fuchsia_hardware_network::MacAddressing>::Create();
          {
            fidl::Status result =
                fidl::WireCall(port_client_end)->GetMac(std::move(mac_server_end));
            if (!result.ok()) {
              printf("netifc: failed to get mac addressing for port (%d:%d): %s\n", port_id.base,
                     port_id.salt, result.FormatDescription().c_str());
              return;
            }
          }

          fidl::WireResult result = fidl::WireCall(mac_client_end)->GetUnicastAddress();
          if (!result.ok()) {
            printf("netifc: failed to get mac address for port (%d:%d): %s\n", port_id.base,
                   port_id.salt, result.FormatDescription().c_str());
            return;
          }
          const fuchsia_net::wire::MacAddress& mac = result.value().address;

          // We have our device, store it and stop watching.
          NetdeviceInterface& discovered_interface = discovered.emplace(NetdeviceInterface{
              .device = std::move(dev),
              .port_id = port_id,
          });
          static_assert(sizeof(mac.octets) == sizeof(discovered_interface.mac.x));
          std::copy(mac.octets.begin(), mac.octets.end(), std::begin(discovered_interface.mac.x));
          defer.cancel();
        });
  }
};

std::optional<Netdevice::Info> netifc_evaluate(std::string_view topological_path,
                                               fidl::UnownedClientEnd<fuchsia_io::Directory> dir,
                                               const std::string& dirname,
                                               const std::string& filename) {
  printf("netifc: ? %s/%s\n", dirname.c_str(), filename.c_str());

  fidl::ClientEnd<fuchsia_hardware_network::Device> dev;
  {
    zx::result status = component::ConnectAt<fuchsia_hardware_network::Device>(dir, filename);
    if (status.is_error()) {
      printf("netifc: failed to connect to %s/%s: %s\n", dirname.c_str(), filename.c_str(),
             status.status_string());
      return std::nullopt;
    }
    dev = std::move(status.value());
  }

  // If an interface was specified, check the topological path of this device and reject it if it
  // doesn't match.
  if (!topological_path.empty()) {
    zx::result resp = fdf_topology::GetTopologicalPath(dir, filename);
    if (resp.is_error()) {
      printf("netifc: GetTopologicalPath returned error %s: %s\n", filename.c_str(),
             zx_status_get_string(resp.error_value()));
      return std::nullopt;
    }

    std::string_view topo_path = SkipInstanceSigil(resp.value());
    // Allow for limited wildcard matching to avoid coupling too tightly.
    if (!EndsWithWildcardMatch(topo_path, topological_path)) {
      return std::nullopt;
    }
  }

  std::optional result = Netdevice::get_interface_if_matching(std::move(dev), filename);
  if (result.has_value()) {
    printf("netsvc: using %s/%s\n", dirname.c_str(), filename.c_str());
  }
  return result;
}

zx::result<std::unique_ptr<fsl::DeviceWatcher>> CreateWatcher(
    async_dispatcher_t* dispatcher, std::optional<NetdeviceInterface>& selected_ifc,
    const std::string& devdir, std::string_view topological_path) {
  const std::string classdir = devdir + Netdevice::kDirectory;
  fbl::unique_fd dir(open(classdir.c_str(), O_DIRECTORY | O_RDONLY));
  if (!dir.is_valid()) {
    printf("failed to open %s: %s\n", classdir.c_str(), strerror(errno));
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  fdio_cpp::FdioCaller caller(std::move(dir));

  zx::result dir_channel = caller.take_directory();
  if (dir_channel.is_error()) {
    return dir_channel.take_error();
  }

  std::unique_ptr watcher = fsl::DeviceWatcher::Create(
      classdir,
      [dispatcher, classdir, topological_path, &selected_ifc](
          const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& filename) {
        std::optional r = netifc_evaluate(topological_path, dir, classdir, filename);
        if (r.has_value()) {
          Netdevice::Process(selected_ifc, dispatcher, std::move(r.value()));
        }
      },
      dispatcher);

  return zx::ok(std::move(watcher));
}

}  // namespace

zx::result<NetdeviceInterface> netifc_discover(const std::string& devdir,
                                               std::string_view topological_path) {
  topological_path = SkipInstanceSigil(topological_path);
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  std::optional<NetdeviceInterface> selected_ifc;

  zx::result netdevice_watcher =
      CreateWatcher(loop.dispatcher(), selected_ifc, devdir, topological_path);
  if (netdevice_watcher.is_error()) {
    return netdevice_watcher.take_error();
  }

  for (;;) {
    zx_status_t status = loop.Run(zx::time::infinite(), /* once */ true);
    if (status != ZX_OK) {
      printf("run loop error: %s\n", zx_status_get_string(status));
      return zx::error(status);
    }
    if (selected_ifc.has_value()) {
      return zx::ok(std::move(selected_ifc.value()));
    }
  }
}
