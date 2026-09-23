// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "third_party/iwlwifi/platform/pcie-iwlwifi-driver.h"

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pci/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <memory>

#include <wlan/drivers/log_instance.h>

#include "third_party/iwlwifi/platform/kernel.h"
#include "third_party/iwlwifi/platform/wlansoftmac-device.h"

extern "C" {
#include "third_party/iwlwifi/iwl-debug.h"
#include "third_party/iwlwifi/iwl-drv.h"
#include "third_party/iwlwifi/mvm/mvm.h"
#include "third_party/iwlwifi/pcie/entry.h"
}
#include "third_party/iwlwifi/platform/driver-inspector.h"
#include "third_party/iwlwifi/platform/mvm-mlme.h"
#include "third_party/iwlwifi/platform/pci-fidl.h"
#include "third_party/iwlwifi/platform/rcu-manager.h"

#if !CPTCFG_IWLMVM
#error "PcieDevice requires support for MVM firmwares."
#endif  // CPTCFG_IWLMVM

namespace wlan {
namespace iwlwifi {

PcieIwlwifiDriver::PcieIwlwifiDriver() : WlanPhyDevice(), fdf::DriverBase2("iwlwifi") {
  pci_dev_ = {};
}

PcieIwlwifiDriver::~PcieIwlwifiDriver() {
  // Release the logger instance.
  wlan::drivers::log::Instance::Reset();
}

constexpr auto kOpenFlags =
    fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolFile;

zx_status_t PcieIwlwifiDriver::LoadFirmware(const char* name, zx_handle_t* vmo, size_t* size) {
  std::string full_filename = "/pkg/lib/firmware/";
  full_filename.append(name);
  auto client = incoming_->Open<fuchsia_io::File>(full_filename.c_str(), kOpenFlags);
  if (client.is_error()) {
    IWL_WARN(nullptr, "Open firmware file failed: %s", zx_status_get_string(client.error_value()));
    return client.error_value();
  }

  fidl::WireResult backing_memory_result =
      fidl::WireCall(*client)->GetBackingMemory(fuchsia_io::wire::VmoFlags::kRead);
  if (!backing_memory_result.ok()) {
    if (backing_memory_result.is_peer_closed()) {
      IWL_WARN(nullptr, "Failed to get backing memory: Peer closed");
      return ZX_ERR_NOT_FOUND;
    }
    IWL_WARN(nullptr, "Failed to get backing memory: %s",
             zx_status_get_string(backing_memory_result.status()));
    return backing_memory_result.status();
  }

  const auto* backing_memory = backing_memory_result.Unwrap();
  if (backing_memory->is_error()) {
    IWL_WARN(nullptr, "Failed to get backing memory: %s",
             zx_status_get_string(backing_memory->error_value()));
    return backing_memory->error_value();
  }

  zx::vmo& backing_vmo = backing_memory->value()->vmo;
  if (zx_status_t status = backing_vmo.get_prop_content_size(size); status != ZX_OK) {
    IWL_WARN(nullptr, "Failed to get vmo size: %s", zx_status_get_string(status));
    return status;
  }
  *vmo = backing_vmo.release();
  return ZX_OK;
}

void PcieIwlwifiDriver::on_fidl_error(fidl::UnbindInfo error) {
  if (error.status() != ZX_OK) {
    IWL_ERR(nullptr, "Unexpected fidl channel close from child node: %s",
            zx_status_get_string(error.status()));
    return;
  }
  IWL_INFO(nullptr, "Child node removed successfully.");
}

void PcieIwlwifiDriver::handle_unknown_event(
    fidl::UnknownEventMetadata<fdf::NodeController> metadata) {}

zx_status_t PcieIwlwifiDriver::AddWlanphyChild() {
  fidl::Arena arena;
  std::vector offers = {
      fdf::MakeOffer2<fuchsia_wlan_phy::Service>(arena, "default"),
  };

  auto args = fdf::wire::NodeAddArgs::Builder(arena)
                  .name("iwlwifi-wlanphy")
                  .offers2(arena, std::move(offers))
                  .Build();

  auto endpoints = fidl::CreateEndpoints<fdf::NodeController>();
  if (endpoints.is_error()) {
    return endpoints.error_value();
  }

  // Adding wlanphy child node. Doing a sync version here to reduce chaos.
  auto result = node_client_.sync()->AddChild(std::move(args), std::move(endpoints->server), {});

  if (!result.ok()) {
    IWL_ERR(nullptr, "Add wlanphy node error due to FIDL error on protocol [Node]: %s",
            result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    IWL_ERR(nullptr, "Add wlanphy node error: %u", static_cast<uint32_t>(result->error_value()));
    return ZX_ERR_INTERNAL;
  }

  wlanphy_controller_client_.Bind(std::move(endpoints->client), dispatcher(), this);

  return ZX_OK;
}

zx_status_t PcieIwlwifiDriver::AddWlansoftmacDevice(uint16_t iface_id, struct iwl_mvm_vif* mvmvif) {
  wlan_softmac_device_ = std::make_unique<WlanSoftmacDevice>(pci_dev_.drvdata, iface_id, mvmvif);

  auto wlansoftmac = [this](fdf::ServerEnd<fuchsia_wlan_softmac::WlanSoftmac> server_end) {
    wlan_softmac_device_->ServiceConnectHandler(driver_dispatcher()->get(), std::move(server_end));
  };

  // Add the service contains WlanSoftmac protocol to outgoing directory.
  fuchsia_wlan_softmac::Service::InstanceHandler wlansoftmac_service_handler(
      {.wlan_softmac = wlansoftmac});

  auto status =
      outgoing()->AddService<fuchsia_wlan_softmac::Service>(std::move(wlansoftmac_service_handler));
  if (status.is_error()) {
    IWL_ERR(nullptr, "Failed to add service to outgoing directory: %s", status.status_string());
    return status.status_value();
  }

  fidl::Arena arena;
  std::vector offers = {
      fdf::MakeOffer2<fuchsia_wlan_softmac::Service>(arena, "default"),
  };

  auto args = fdf::wire::NodeAddArgs::Builder(arena)
                  .name("iwlwifi-wlansoftmac")
                  .offers2(arena, std::move(offers))
                  .Build();

  auto endpoints = fidl::CreateEndpoints<fdf::NodeController>();
  if (endpoints.is_error()) {
    return endpoints.status_value();
  }

  wlansoftmac_controller_client_ = std::make_optional<fidl::WireClient<fdf::NodeController>>();
  wlansoftmac_controller_client_.value().Bind(std::move(endpoints->client), dispatcher(), this);

  // Add wlansoftmac child node for the node that this driver is binding to. Doing a sync version
  // here to reduce chaos.
  auto result = node_client_.sync()->AddChild(std::move(args), std::move(endpoints->server), {});

  if (!result.ok()) {
    IWL_ERR(nullptr, "Add wlansoftmac node error due to FIDL error on protocol [Node]: %s",
            result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    IWL_ERR(nullptr, "Add wlansoftmac node error: %u",
            static_cast<uint32_t>(result->error_value()));
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

zx_status_t PcieIwlwifiDriver::RemoveWlansoftmacDevice(uint16_t iface_id) {
  // The iface_id passed in is not used now, will apply it when iwlwifi driver starts supporting
  // more than one wlansoftmac iface.
  auto result = wlansoftmac_controller_client_.value()->Remove();
  if (!result.ok()) {
    IWL_ERR(nullptr, "Softmac child remove failed, FIDL error: %s", result.status_string());
    return result.status();
  }

  wlan_softmac_device_.reset();
  auto remove_result = outgoing()->RemoveService<fuchsia_wlan_softmac::Service>();
  if (remove_result.is_error()) {
    IWL_ERR(nullptr, "Failed to remove wlansoftmac service from outgoing directory: %s.",
            remove_result.status_string());
    return remove_result.status_value();
  }
  return ZX_OK;
}

zx::result<> PcieIwlwifiDriver::Start(fdf::DriverContext context) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  node_client_ = fidl::WireClient(take_node(), dispatcher());

  // Take over the logger lifecycle management from DriverBase.
  wlan::drivers::log::Instance::Init(0, &logger());

  zx_status_t status = AddWlanPhyService();
  if (status != ZX_OK) {
    IWL_ERR(nullptr, "ServeRuntimeProtocolForV1Devices failed: %s", zx_status_get_string(status));
    return zx::error(status);
  }
  // Initialize the driver.
  Initialize();

  return zx::ok();
}

void PcieIwlwifiDriver::Stop(fdf::StopCompleter completer) {
  if (drvdata()) {
    // TODO(b/306684649): remove the below workaround once the issue is fixed.
    const int mac_id = 0;  // Assume only one interface is created.
    const auto iwl_trans = reinterpret_cast<struct iwl_trans*>(drvdata());
    struct iwl_mvm* mvm = iwl_trans_get_mvm(iwl_trans);
    struct iwl_mvm_vif* mvmvif = mvm->mvmvif[mac_id];

    mac_stop(mvmvif);
    phy_destroy_iface(drvdata(), mac_id);
  }

  // wlan_softmac_device_ destruction uses rcu_manager, so do it before iwl_pci_remove(), otherwise
  // it will fail to access rcu_manager.
  wlan_softmac_device_.reset();
  iwl_pci_remove(&pci_dev_);
  zx_handle_close(pci_dev_.dev.bti);
  pci_dev_.dev.bti = ZX_HANDLE_INVALID;
  ZX_DEBUG_ASSERT(pci_dev_.drvdata == nullptr);
  completer(zx::ok());
}

iwl_trans* PcieIwlwifiDriver::drvdata() { return pci_dev_.drvdata; }

const iwl_trans* PcieIwlwifiDriver::drvdata() const { return pci_dev_.drvdata; }

zx_status_t load_firmware_callback_entry(void* ctx, const char* name, zx_handle_t* vmo,
                                         size_t* size) {
  return static_cast<PcieIwlwifiDriver*>(ctx)->LoadFirmware(name, vmo, size);
}

zx_status_t PcieIwlwifiDriver::Initialize() {
  zx_status_t status = ZX_OK;

  driver_inspector_ = std::make_unique<DriverInspector>(
      dispatcher(), outgoing()->component(), DriverInspectorOptions{.root_name = "iwlwifi"});

  rcu_manager_ = std::make_unique<RcuManager>(dispatcher());
  rcu_manager_->InitForThread();

  // Fill in the relevant Fuchsia-specific fields in our driver interface struct.
  pci_dev_.dev.load_firmware_ctx = (void*)this;
  pci_dev_.dev.load_firmware_callback =
      &load_firmware_callback_entry;  // Save the namespace address to a C void pointer.
  pci_dev_.dev.task_dispatcher = dispatcher();
  pci_dev_.dev.irq_dispatcher = dispatcher();
  pci_dev_.dev.rcu_manager = static_cast<struct rcu_manager*>(rcu_manager_.get());
  pci_dev_.dev.inspector = static_cast<struct driver_inspector*>(driver_inspector_.get());

  auto pci_client_end = incoming_->Connect<fuchsia_hardware_pci::Service::Device>();
  if (pci_client_end.is_error()) {
    IWL_ERR(nullptr, "Failed to connect to PCI service: %s", pci_client_end.status_string());
    return pci_client_end.status_value();
  }
  iwl_pci_connect_fragment_protocol_with_client(std::move(pci_client_end.value()), &pci_dev_.fidl);

  if ((status = StartPci()) != ZX_OK) {
    IWL_ERR(nullptr, "Failed to Start PCI: %s", zx_status_get_string(status));
    return status;
  }

  ZX_ASSERT_MSG(AddWlanphyChild() == ZX_OK, "AddWlanphyChild failed.");

  return ZX_OK;
}

zx_status_t PcieIwlwifiDriver::StartPci() {
  zx_status_t status;

  if ((status = iwl_pci_get_bti(pci_dev_.fidl, /*index*/ 0, &pci_dev_.dev.bti)) != ZX_OK) {
    IWL_ERR(nullptr, "Failed to get PCI BTI: %s", zx_status_get_string(status));
    return status;
  }

  pci_device_info_t pci_info = {};
  if ((status = iwl_pci_get_device_info(pci_dev_.fidl, &pci_info)) != ZX_OK) {
    return status;
  }
  uint16_t subsystem_device_id = 0;
  if ((status = iwl_pci_read_config16(
           pci_dev_.fidl, fidl::ToUnderlying(fuchsia_hardware_pci::Config::kSubsystemId),
           &subsystem_device_id)) != ZX_OK) {
    IWL_ERR(nullptr, "Failed to read PCI subsystem device ID: %s", zx_status_get_string(status));
    return status;
  }

  IWL_INFO(nullptr, "Device ID: %04x Subsystem Device ID: %04x", pci_info.device_id,
           subsystem_device_id);
  pci_dev_.device = pci_info.device_id;
  pci_dev_.subsystem_device = subsystem_device_id;

  // Do iwl_drv_init() before iwl_pci_probe.
  if ((status = iwl_drv_init()) != ZX_OK) {
    IWL_ERR(nullptr, "Failed to init driver: %s\n", zx_status_get_string(status));
    return status;
  }

  const iwl_pci_device_id* id = nullptr;
  if ((status = iwl_pci_find_device_id(pci_info.device_id, subsystem_device_id, &id)) != ZX_OK) {
    IWL_ERR(nullptr, "Failed to find PCI config: %s  device_id=0x%04x  subsys_did=0x%04x",
            zx_status_get_string(status), pci_info.device_id, subsystem_device_id);
    return status;
  }

  if ((status = iwl_pci_probe(&pci_dev_, id)) != ZX_OK) {
    IWL_ERR(nullptr, "Failed to probe PCI device: %s", zx_status_get_string(status));
    // No return here to pass initilization process for tests, add return back after fining the
    // test environment.
  }

  return ZX_OK;
}

zx_status_t PcieIwlwifiDriver::AddWlanPhyService() {
  // Add the service contains Wlanphy protocol to outgoing directory.
  auto wlanphy = [this](fidl::ServerEnd<fuchsia_wlan_phy::WlanPhy> server_end) {
    // Note: The same dispatcher here is used for softmac device, will it affect the data path
    // performance?
    ServiceConnectHandler(dispatcher(), std::move(server_end));
  };

  fuchsia_wlan_phy::Service::InstanceHandler wlanphy_service_handler(
      {.device = std::move(wlanphy)});

  auto status =
      outgoing()->AddService<fuchsia_wlan_phy::Service>(std::move(wlanphy_service_handler));
  if (status.is_error()) {
    IWL_ERR(nullptr, "Failed to add service to outgoing directory: %s", status.status_string());
    return status.status_value();
  }

  return ZX_OK;
}

}  // namespace iwlwifi
}  // namespace wlan

FUCHSIA_DRIVER_EXPORT2(::wlan::iwlwifi::PcieIwlwifiDriver);
