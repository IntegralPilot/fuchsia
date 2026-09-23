// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BUS_DRIVERS_PLATFORM_PLATFORM_DEVICE_H_
#define SRC_DEVICES_BUS_DRIVERS_PLATFORM_PLATFORM_DEVICE_H_

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.interrupt/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/fit/function.h>
#include <lib/fpromise/promise.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/zx/bti.h>

#include "src/devices/bus/drivers/platform/platform-interrupt.h"

namespace platform_bus {

class PlatformBus;

// This class represents a platform device attached to the platform bus.
// Instances of this class are created by PlatformBus at boot time when the board driver
// calls the platform bus protocol method pbus_device_add().

class PlatformDevice : public fidl::WireServer<fuchsia_hardware_platform_device::Device>,
                       public fidl::WireServer<fuchsia_hardware_interrupt::ControllerRegistry> {
 public:
  using GetInterruptCallback = fit::callback<void(zx::result<zx::interrupt>)>;

  template <typename T>
  static GetInterruptCallback MakeGetInterruptCallback(T& completer) {
    return [completer = completer.ToAsync()](zx::result<zx::interrupt> irq) mutable {
      if (irq.is_error()) {
        completer.ReplyError(irq.status_value());
      } else {
        completer.ReplySuccess(std::move(irq.value()));
      }
    };
  }

  // Creates a new PlatformDevice instance.
  // *flags* contains zero or more PDEV_ADD_* flags from the platform bus protocol.
  static zx::result<std::unique_ptr<PlatformDevice>> Create(
      fuchsia_hardware_platform_bus::Node node, PlatformBus* bus,
      inspect::ComponentInspector& inspector);

  explicit PlatformDevice(PlatformBus* bus, inspect::Node inspect_node,
                          fuchsia_hardware_platform_bus::Node node);

  const std::string& name() const { return name_; }
  uint32_t vid() const { return vid_; }
  uint32_t pid() const { return pid_; }
  uint32_t did() const { return did_; }
  uint32_t instance_id() const { return instance_id_; }

  PlatformBus* bus() { return bus_; }
  const PlatformBus* bus() const { return bus_; }

  zx::result<> CreateNode();
  zx::result<> Init();

  struct Mmio {
    zx_off_t offset;
    uint64_t size;
    zx_handle_t vmo;
  };

  struct DeviceInfo {
    uint32_t vid;
    uint32_t pid;
    uint32_t did;
    uint32_t mmio_count;
    uint32_t irq_count;
    uint32_t bti_count;
    uint32_t smc_count;
    uint32_t metadata_count;
    uint32_t reserved[8];
    std::string name;
  };

  struct BoardInfo {
    uint32_t vid;
    uint32_t pid;
    uint32_t board_revision;
    std::string board_name;
  };

  zx::result<Mmio> GetMmio(uint32_t index) const;
  void GetInterrupt(uint32_t index, uint32_t flags, GetInterruptCallback callback);
  zx::result<zx::bti> GetBti(uint32_t index) const;
  zx::result<zx::resource> GetSmc(uint32_t index) const;
  DeviceInfo GetDeviceInfo() const;
  BoardInfo GetBoardInfo() const;

  // Platform device protocol FIDL implementation.
  void GetMmioById(GetMmioByIdRequestView request, GetMmioByIdCompleter::Sync& completer) override;
  void GetMmioByName(GetMmioByNameRequestView request,
                     GetMmioByNameCompleter::Sync& completer) override;
  void GetInterruptById(GetInterruptByIdRequestView request,
                        GetInterruptByIdCompleter::Sync& completer) override;
  void GetInterruptByName(GetInterruptByNameRequestView request,
                          GetInterruptByNameCompleter::Sync& completer) override;
  void GetBtiById(GetBtiByIdRequestView request, GetBtiByIdCompleter::Sync& completer) override;
  void GetBtiByName(GetBtiByNameRequestView request,
                    GetBtiByNameCompleter::Sync& completer) override;
  void GetSmcById(GetSmcByIdRequestView request, GetSmcByIdCompleter::Sync& completer) override;
  void GetSmcByName(GetSmcByNameRequestView request,
                    GetSmcByNameCompleter::Sync& completer) override;
  void GetPowerConfiguration(GetPowerConfigurationCompleter::Sync& completer) override;
  void GetNodeDeviceInfo(GetNodeDeviceInfoCompleter::Sync& completer) override;
  void GetBoardInfo(GetBoardInfoCompleter::Sync& completer) override;
  void GetMetadata(GetMetadataRequestView request, GetMetadataCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_platform_device::Device> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // fuchsia.hardware.interrupt.ControllerRegistry implementation.
  void RegisterController(RegisterControllerRequestView request,
                          RegisterControllerCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_interrupt::ControllerRegistry> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  bool HasInterruptVector(uint32_t vector) const { return interrupt_vectors_.contains(vector); }
  bool HasInterruptKoid(zx_koid_t koid) const { return interrupt_koids_.contains(koid); }

  zx::event node_token() const {
    if (node_token_.has_value()) {
      zx::event duplicate;
      ZX_ASSERT(node_token_->duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate) == ZX_OK);
      return duplicate;
    }
    return zx::event();
  }

 private:
  // Allows for `std::string_view`'s to be used when searching an unordered map that uses
  // `std::string` as its key.
  struct MetadataIdHash {
    using hash_type = std::hash<std::string_view>;
    using is_transparent = void;

    std::size_t operator()(const char* str) const { return hash_type{}(str); }
    std::size_t operator()(std::string_view str) const { return hash_type{}(str); }
    std::size_t operator()(std::string const& str) const { return hash_type{}(str); }
  };

  fpromise::promise<inspect::Inspector> InspectNodeCallback() const;

  PlatformBus* bus_;
  std::string name_;
  const uint32_t vid_;
  const uint32_t pid_;
  const uint32_t did_;
  const uint32_t instance_id_;

  fidl::Client<fuchsia_driver_framework::NodeController> node_controller_;

  fuchsia_hardware_platform_bus::Node node_;
  fdf::ServerBindingGroup<fuchsia_hardware_platform_bus::PlatformBus> bus_bindings_;
  fidl::ServerBindingGroup<fuchsia_hardware_platform_device::Device> device_bindings_;
  fidl::ServerBindingGroup<fuchsia_hardware_interrupt::ControllerRegistry> controller_bindings_;
  std::unordered_map<std::string, std::vector<uint8_t>, MetadataIdHash, std::equal_to<>> metadata_;
  std::vector<std::unique_ptr<PlatformInterruptFragment>> fragments_;

  // Contains the vectors used when creating interrupts. `interrupt_vectors_`
  // must be above `inspector_`. This is to ensure that `interrupt_vectors_` is
  // not destructed before `inspector_` is destructed. When `inspector_`
  // destructs, it executes a callback that references `interrupt_vectors_`.
  std::unordered_set<uint32_t> interrupt_vectors_;
  std::unordered_set<zx_koid_t> interrupt_koids_;

  // When the node is bound, a node_token_ representing the driver component which bound to the node
  // is generated. This can be used to for introspection.
  std::optional<zx::event> node_token_;

  inspect::Node inspect_node_;

  std::unique_ptr<compat::DeviceServer> device_server_;

  std::unique_ptr<fdf_metadata::MetadataServer<fuchsia_boot_metadata::SerialNumberMetadata>>
      serial_number_metadata_server_;
  std::unique_ptr<fdf_metadata::MetadataServer<fuchsia_boot_metadata::PartitionMap>>
      partition_map_metadata_server_;
  std::unique_ptr<fdf_metadata::MetadataServer<fuchsia_boot_metadata::MacAddressMetadata>>
      mac_address_metadata_server_;
};

}  // namespace platform_bus

#endif  // SRC_DEVICES_BUS_DRIVERS_PLATFORM_PLATFORM_DEVICE_H_
