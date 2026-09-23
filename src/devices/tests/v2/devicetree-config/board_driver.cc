// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devicetree/manager/manager.h>
#include <lib/driver/devicetree/manager/publisher-dev.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/fuchsia-config/fuchsia-config.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <vector>

namespace devicetree_config {

class BoardDriver final : public fdf::DriverBase2 {
 public:
  BoardDriver() : fdf::DriverBase2("devicetree-config-board") {}

  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override {
    node_.Bind(take_node());

    auto incoming = context.take_incoming();

    auto client_end = incoming->Open<fuchsia_io::File>("/pkg/test-data/devicetree-config.dtb",
                                                       fuchsia_io::Flags::kPermReadBytes);
    if (client_end.is_error()) {
      FDF_LOG(ERROR, "Failed to open devicetree blob: %s", client_end.status_string());
      completer(client_end.take_error());
      return;
    }

    fidl::SyncClient file(std::move(client_end.value()));
    auto result =
        file->GetBackingMemory(fuchsia_io::VmoFlags::kRead | fuchsia_io::VmoFlags::kPrivateClone);
    if (result.is_error()) {
      FDF_LOG(ERROR, "Failed to get backing memory: %s",
              result.error_value().FormatDescription().c_str());
      completer(zx::error(ZX_ERR_INTERNAL));
      return;
    }

    zx::vmo vmo = std::move(result->vmo());
    size_t size;
    if (zx_status_t status = vmo.get_size(&size); status != ZX_OK) {
      FDF_LOG(ERROR, "Failed to get VMO size: %s", zx_status_get_string(status));
      completer(zx::error(status));
      return;
    }

    std::vector<uint8_t> blob(size);
    if (zx_status_t status = vmo.read(blob.data(), 0, size); status != ZX_OK) {
      FDF_LOG(ERROR, "Failed to read VMO: %s", zx_status_get_string(status));
      completer(zx::error(status));
      return;
    }

    fdf_devicetree::Manager manager(std::move(blob));

    fdf_devicetree::VisitorRegistry registry;
    auto reg_status = registry.RegisterVisitor<fdf_devicetree::BindPropertyVisitor>();
    if (reg_status.is_error()) {
      FDF_LOG(ERROR, "Failed to register BindPropertyVisitor: %s", reg_status.status_string());
      completer(reg_status.take_error());
      return;
    }

    reg_status = registry.RegisterVisitor<fdf_devicetree::FuchsiaConfigVisitor>();
    if (reg_status.is_error()) {
      FDF_LOG(ERROR, "Failed to register FuchsiaConfig visitor: %s", reg_status.status_string());
      completer(reg_status.take_error());
      return;
    }

    auto walk_status = manager.Walk(registry);
    if (walk_status.is_error()) {
      FDF_LOG(ERROR, "Failed to walk the device tree: %s", walk_status.status_string());
      completer(walk_status.take_error());
      return;
    }

    async::PostTask(dispatcher(), [this, incoming = std::move(incoming),
                                   manager = std::move(manager),
                                   completer = std::move(completer)]() mutable {
      zx::result pbus = incoming->Connect<fuchsia_hardware_platform_bus::Service::PlatformBus>();
      if (pbus.is_error() || !pbus->is_valid()) {
        FDF_LOG(ERROR, "Failed to connect to platform bus: %s", pbus.status_string());
        completer(pbus.take_error());
        return;
      }

      zx::result group_manager =
          incoming->Connect<fuchsia_driver_framework::CompositeNodeManager>();
      if (group_manager.is_error()) {
        FDF_LOG(ERROR, "Failed to connect to device group manager: %s",
                group_manager.status_string());
        completer(group_manager.take_error());
        return;
      }

      auto pbus_client = fdf::WireSyncClient(std::move(pbus.value()));
      auto mgr_client = fidl::SyncClient(std::move(group_manager.value()));

      fdf_devicetree::PublisherDev publisher(pbus_client, mgr_client, node_);
      auto publish_status = manager.PublishDevices(publisher);
      if (publish_status.is_error()) {
        FDF_LOG(ERROR, "Failed to publish devices: %s", publish_status.status_string());
        completer(publish_status.take_error());
        return;
      }
      FDF_LOG(INFO, "Successfully published devices asynchronously");
      completer(zx::ok());
    });
  }

 private:
  fidl::SyncClient<fuchsia_driver_framework::Node> node_;
};

}  // namespace devicetree_config

FUCHSIA_DRIVER_EXPORT2(devicetree_config::BoardDriver);
