// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BUS_DRIVERS_PLATFORM_PLATFORM_BUS_H_
#define SRC_DEVICES_BUS_DRIVERS_PLATFORM_PLATFORM_BUS_H_

#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.hardware.interrupt/cpp/fidl.h>
#include <fidl/fuchsia.hardware.interrupt/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.sysinfo/cpp/wire.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/node/cpp/add_child.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/zbi-format/board.h>
#include <lib/zx/iommu.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <map>
#include <vector>

#include <fbl/array.h>

#include "platform-device.h"

namespace platform_bus {

class PlatformBus;

// This is the main class for the platform bus driver.
class PlatformBus : public fdf::DriverBase2,
                    public fdf::WireServer<fuchsia_hardware_platform_bus::PlatformBus>,
                    public fdf::WireServer<fuchsia_hardware_platform_bus::Firmware>,
                    public fidl::Server<fuchsia_hardware_platform_bus::InterruptAttributor>,
                    public fidl::WireServer<fuchsia_sysinfo::SysInfo> {
 public:
  explicit PlatformBus() : fdf::DriverBase2("platform-bus") {}

  ~PlatformBus() { fdf::info("~PlatformBus()"); }

  using fdf::DriverBase2::dispatcher;
  using fdf::DriverBase2::driver_dispatcher;
  using fdf::DriverBase2::outgoing;

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // fuchsia.hardware.platform.bus.PlatformBus implementation.
  void NodeAdd(NodeAddRequestView request, fdf::Arena& arena,
               NodeAddCompleter::Sync& completer) override;

  void GetBoardInfo(fdf::Arena& arena, GetBoardInfoCompleter::Sync& completer) override;
  void SetBoardInfo(SetBoardInfoRequestView request, fdf::Arena& arena,
                    SetBoardInfoCompleter::Sync& completer) override;
  void SetBootloaderInfo(SetBootloaderInfoRequestView request, fdf::Arena& arena,
                         SetBootloaderInfoCompleter::Sync& completer) override;

  void RegisterSysSuspendCallback(RegisterSysSuspendCallbackRequestView request, fdf::Arena& arena,
                                  RegisterSysSuspendCallbackCompleter::Sync& completer) override;
  void AddCompositeNodeSpec(AddCompositeNodeSpecRequestView request, fdf::Arena& arena,
                            AddCompositeNodeSpecCompleter::Sync& completer) override;
  void RegisterIommu(RegisterIommuRequestView request, fdf::Arena& arena,
                     RegisterIommuCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_platform_bus::PlatformBus> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // fuchsia.hardware.platform.bus.Firmware implementation.
  void GetFirmware(GetFirmwareRequestView request, fdf::Arena& arena,
                   GetFirmwareCompleter::Sync& completer) override;

  // InterruptAttributor protocol implementation.
  void GetInterruptInfo(GetInterruptInfoRequest& request,
                        GetInterruptInfoCompleter::Sync& completer) override;

  // SysInfo protocol implementation.
  void GetBoardName(GetBoardNameCompleter::Sync& completer) override;
  void GetBoardRevision(GetBoardRevisionCompleter::Sync& completer) override;
  void GetBootloaderVendor(GetBootloaderVendorCompleter::Sync& completer) override;
  void GetInterruptControllerInfo(GetInterruptControllerInfoCompleter::Sync& completer) override;
  void GetSerialNumber(GetSerialNumberCompleter::Sync& completer) override;

  zx::result<zx::bti> GetBti(uint32_t iommu_id, uint32_t bti_id, std::string_view name);

  zx::result<> RegisterInterruptController(
      uint32_t id, fidl::ClientEnd<fuchsia_hardware_interrupt::Controller> controller);
  void RegisterInterrupt(const fuchsia_hardware_platform_bus::UserspaceIrq& irq, uint32_t flags,
                         zx::interrupt interrupt, PlatformDevice::GetInterruptCallback callback);

  zx::unowned_resource GetIrqResource() const;
  zx::unowned_resource GetMmioResource() const;
  zx::unowned_resource GetSmcResource() const;
  zx::unowned_resource GetIommuResource() const;

  struct BootItemResult {
    zx::vmo vmo;
    uint32_t length;
  };
  // Returns ZX_ERR_NOT_FOUND when boot item wasn't found.
  zx::result<std::vector<BootItemResult>> GetBootItem(uint32_t type, std::optional<uint32_t> extra);
  zx::result<fbl::Array<uint8_t>> GetBootItemArray(uint32_t type, std::optional<uint32_t> extra);

  fidl::WireClient<fuchsia_hardware_platform_bus::SysSuspend>& suspend_cb() { return suspend_cb_; }

  fuchsia_hardware_platform_bus::TemporaryBoardInfo board_info() { return board_info_; }

  fdf::ServerBindingGroup<fuchsia_hardware_platform_bus::PlatformBus>& bindings() {
    return bindings_;
  }

  fdf::ServerBindingGroup<fuchsia_hardware_platform_bus::Firmware>& fw_bindings() {
    return fw_bindings_;
  }

  fidl::ServerBindingGroup<fuchsia_hardware_platform_bus::InterruptAttributor>&
  interrupt_bindings() {
    return interrupt_bindings_;
  }

  fidl::ServerBindingGroup<fuchsia_sysinfo::SysInfo>& sysinfo_bindings() {
    return sysinfo_bindings_;
  }

  bool suspend_enabled() const { return suspend_enabled_; }

  fidl::UnownedClientEnd<fuchsia_driver_framework::Node> platform_node() const {
    return platform_node_.node_.borrow();
  }

 protected:
  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }

 private:
  struct PendingInterruptRequest {
    fuchsia_hardware_platform_bus::UserspaceIrq irq;
    uint32_t flags;
    zx::interrupt interrupt;
    PlatformDevice::GetInterruptCallback callback;
  };

  class ControllerEventHandler
      : public fidl::WireAsyncEventHandler<fuchsia_hardware_interrupt::Controller> {
   public:
    ControllerEventHandler(PlatformBus* bus, uint32_t id) : bus_(bus), id_(id) {}
    void on_fidl_error(fidl::UnbindInfo info) override;
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_hardware_interrupt::Controller> metadata) override;

   private:
    PlatformBus* const bus_;
    const uint32_t id_;
  };

  struct InterruptController {
    fidl::WireClient<fuchsia_hardware_interrupt::Controller> client;
    std::unique_ptr<ControllerEventHandler> event_handler;
  };

  template <typename Protocol>
  zx::unowned_resource GetResource() const {
    static zx::resource resource;
    if (!resource.is_valid()) {
      zx::result client = incoming()->Connect<Protocol>();
      if (client.is_ok()) {
        fidl::Result result = fidl::Call(*client)->Get();
        if (result.is_ok()) {
          resource = std::move(result.value().resource());
        }
      }
    }
    return resource.borrow();
  }

  fidl::WireClient<fuchsia_hardware_platform_bus::SysSuspend> suspend_cb_;

  DISALLOW_COPY_ASSIGN_AND_MOVE(PlatformBus);

  zx::result<zbi_board_info_t> GetBoardInfo();

  zx::result<> NodeAddInternal(fuchsia_hardware_platform_bus::Node& node);

  static void RegisterInterruptWithController(
      const fidl::WireClient<fuchsia_hardware_interrupt::Controller>& controller,
      const fuchsia_hardware_platform_bus::UserspaceIrq& irq, uint32_t flags,
      zx::interrupt interrupt, PlatformDevice::GetInterruptCallback callback);

  void OnControllerDisconnect(uint32_t controller_id);

  fidl::ClientEnd<fuchsia_boot::Items> items_svc_;

  fuchsia_hardware_platform_bus::TemporaryBoardInfo board_info_ = {};
  // List to cache requests when board_name is not yet set.
  std::vector<GetBoardNameCompleter::Async> board_name_completer_;

  fuchsia_hardware_platform_bus::BootloaderInfo bootloader_info_ = {};
  // List to cache requests when vendor is not yet set.
  std::vector<GetBootloaderVendorCompleter::Async> bootloader_vendor_completer_;

  fuchsia_sysinfo::wire::InterruptControllerType interrupt_controller_type_ =
      fuchsia_sysinfo::wire::InterruptControllerType::kUnknown;

  // Map of iommu ids to iommu handles.
  std::map<uint32_t, zx::iommu> iommu_handles_;

  // Maps interrupt controller IDs to FIDL clients.
  std::map<uint32_t, InterruptController> interrupt_controllers_;

  std::map<uint32_t, std::vector<PendingInterruptRequest>> pending_interrupts_;

  std::map<std::pair<uint32_t, uint32_t>, zx::bti> cached_btis_;

  fdf::OwnedChildNode sys_node_;
  fdf::OwnedChildNode platform_node_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> pt_node_;

  std::shared_ptr<fdf::Namespace> incoming_;

  std::optional<inspect::ComponentInspector> component_inspector_;
  std::vector<std::unique_ptr<PlatformDevice>> devices_;

  compat::DeviceServer device_server_;

  fdf::ServerBindingGroup<fuchsia_hardware_platform_bus::PlatformBus> bindings_;
  fdf::ServerBindingGroup<fuchsia_hardware_platform_bus::Firmware> fw_bindings_;
  fidl::ServerBindingGroup<fuchsia_hardware_platform_bus::InterruptAttributor> interrupt_bindings_;
  fidl::ServerBindingGroup<fuchsia_sysinfo::SysInfo> sysinfo_bindings_;

  bool suspend_enabled_ = false;
};

}  // namespace platform_bus

#endif  // SRC_DEVICES_BUS_DRIVERS_PLATFORM_PLATFORM_BUS_H_
