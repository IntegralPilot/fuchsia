// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.nand/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/directory.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <utility>

#include <fbl/string_buffer.h>
#include <fbl/unique_fd.h>
#include <ramdevice-client/ramnand.h>

namespace ramdevice_client {

__EXPORT
zx::result<RamNand> RamNand::Create(fuchsia_hardware_nand::wire::RamNandInfo config) {
  zx::result ctl = component::Connect<fuchsia_hardware_nand::RamNandCtl>(kBasePath);
  if (ctl.is_error()) {
    fprintf(stderr, "could not connect to RamNandCtl: %s\n", ctl.status_string());
    return ctl.take_error();
  }

  const fidl::WireResult result = fidl::WireCall(ctl.value())->CreateDevice(std::move(config));
  if (!result.ok()) {
    fprintf(stderr, "could not create ram_nand device: %s\n", result.status_string());
    return zx::error(result.status());
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    fprintf(stderr, "could not create ram_nand device: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }
  const std::string name(response.name.get());

  fbl::unique_fd ram_nand_ctl;
  if (zx_status_t status = fdio_open3_fd(
          kBasePath,
          uint64_t{fuchsia_io::wire::kPermReadable | fuchsia_io::Flags::kProtocolDirectory},
          ram_nand_ctl.reset_and_get_address());
      status != ZX_OK) {
    fprintf(stderr, "Could not open ram_nand_ctl: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }

  std::string controller_path = name + "/device_controller";
  zx::result controller =
      device_watcher::RecursiveWaitForFile(ram_nand_ctl.get(), controller_path.c_str());
  if (controller.is_error()) {
    fprintf(stderr, "could not open ram_nand controller at '%s': %s\n", name.c_str(),
            controller.status_string());
    return controller.take_error();
  }

  zx::result ram_nand =
      Create(fidl::ClientEnd<fuchsia_device::Controller>(std::move(controller.value())),
             fbl::String::Concat({kBasePath, "/", name}), fbl::String(name.c_str()));
  if (ram_nand.is_error()) {
    return ram_nand.take_error();
  }
  return ram_nand;
}

__EXPORT
zx::result<RamNand> RamNand::Create(fidl::ClientEnd<fuchsia_device::Controller> controller,
                                    std::optional<fbl::String> path,
                                    std::optional<fbl::String> filename) {
  if (!controller) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  auto [client, server] = fidl::Endpoints<fuchsia_hardware_nand::RamNand>::Create();
  const fidl::OneWayStatus connect_result =
      fidl::WireCall(controller)->ConnectToDeviceFidl(server.TakeChannel());
  if (!connect_result.ok()) {
    fprintf(stderr, "Failed to connect to device FIDL: %s\n",
            connect_result.FormatDescription().c_str());
    return zx::error(connect_result.status());
  }
  return zx::ok(RamNand(std::move(controller), std::move(client), path, filename));
}

RamNand::RamNand(fidl::ClientEnd<fuchsia_device::Controller> controller,
                 fidl::ClientEnd<fuchsia_hardware_nand::RamNand> ram_nand,
                 std::optional<fbl::String> path, std::optional<fbl::String> filename)
    : controller_(std::move(controller)),
      ram_nand_(std::move(ram_nand)),
      path_(path),
      filename_(filename) {}

__EXPORT
RamNand::~RamNand() {
  if (unbind && controller_) {
    const fidl::WireResult result = fidl::WireCall(ram_nand_)->Unlink();
    if (!result.ok()) {
      fprintf(stderr, "Could not call unlink ram_nand: %s\n", result.FormatDescription().c_str());
      return;
    }
    if (zx_status_t status = result.value().status; status != ZX_OK) {
      fprintf(stderr, "Could not unlink ram_nand: %s\n", zx_status_get_string(status));
    }
    zx_signals_t pending;
    zx_status_t status =
        ram_nand_.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &pending);
    if (status != ZX_OK) {
      fprintf(stderr, "Failed to wait for ram_nand to unlink: %s\n",
              zx_status_get_string(status));
    }
  }
}

}  // namespace ramdevice_client
