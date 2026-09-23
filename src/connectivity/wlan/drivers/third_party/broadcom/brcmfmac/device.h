// Copyright (c) 2019 The Fuchsia Authors
//
// Permission to use, copy, modify, and/or distribute this software for any purpose with or without
// fee is hereby granted, provided that the above copyright notice and this permission notice
// appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
// SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
// AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
// NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE
// OF THIS SOFTWARE.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_DEVICE_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_DEVICE_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.factory.wlan/cpp/fidl.h>
#include <fidl/fuchsia.wlan.phy/cpp/fidl.h>
#include <fidl/fuchsia.wlan.phy/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/fdf/cpp/arena.h>
#include <lib/fdf/cpp/channel.h>
#include <lib/fdf/cpp/channel_read.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/fit/function.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <memory>
#include <mutex>

#include <wlan/drivers/components/network_device.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/core.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}
namespace wlan {
namespace brcmfmac {

class DeviceInspect;
class WlanInterface;
class Device : public fidl::Server<fuchsia_wlan_phy::WlanPhy>,
               public fidl::WireAsyncEventHandler<fdf::NodeController>,
               public fidl::WireServer<fuchsia_factory_wlan::Iovar>,
               public ::wlan::drivers::components::NetworkDevice::Callbacks {
 public:
  virtual ~Device();
  explicit Device();

  // Device Initialization
  zx_status_t InitServerDispatcher();
  zx_status_t InitWlanPhy();
  zx_status_t InitDevice(fdf::OutgoingDirectory& outgoing,
                         const std::shared_ptr<fdf::Namespace>& incoming);
  void InitPhyDevice();
  virtual zx_status_t BusInit(const std::shared_ptr<fdf::Namespace>& incoming) = 0;
  WlanInterface* GetClientInterface() { return client_interface_.get(); }
  WlanInterface* GetSoftApInterface() { return ap_interface_.get(); }

  // State accessors
  brcmf_pub* drvr();
  const brcmf_pub* drvr() const;
  ::wlan::drivers::components::NetworkDevice& NetDev() { return network_device_; }

  // Virtual state accessors
  virtual async_dispatcher_t* GetTimerDispatcher() = 0;
  virtual DeviceInspect* GetInspect() = 0;
  virtual fidl::WireClient<fdf::Node>& GetParentNode() = 0;
  virtual std::shared_ptr<fdf::OutgoingDirectory>& Outgoing() = 0;
  virtual fdf_dispatcher_t* GetDriverDispatcher() = 0;

  // WlanPhy interface implementation.
  void Init(InitRequest& request, InitCompleter::Sync& completer) override;
  void GetSupportedMacRoles(GetSupportedMacRolesCompleter::Sync& completer) override;
  void CreateIface(CreateIfaceRequest& request, CreateIfaceCompleter::Sync& completer) override;
  void DestroyIface(DestroyIfaceRequest& request, DestroyIfaceCompleter::Sync& completer) override;
  void SetCountry(SetCountryRequest& request, SetCountryCompleter::Sync& completer) override;
  void GetCountry(GetCountryCompleter::Sync& completer) override;
  void ClearCountry(ClearCountryCompleter::Sync& completer) override;
  void SetPowerSaveMode(SetPowerSaveModeRequest& request,
                        SetPowerSaveModeCompleter::Sync& completer) override;
  void GetPowerSaveMode(GetPowerSaveModeCompleter::Sync& completer) override;
  void PowerDown(PowerDownCompleter::Sync& completer) override;
  void PowerUp(PowerUpCompleter::Sync& completer) override;
  void Reset(ResetCompleter::Sync& completer) override;
  void GetPowerState(GetPowerStateCompleter::Sync& completer) override;
  void SetBtCoexistenceMode(SetBtCoexistenceModeRequest& request,
                            SetBtCoexistenceModeCompleter::Sync& completer) override;
  void SetTxPowerScenario(SetTxPowerScenarioRequest& request,
                          SetTxPowerScenarioCompleter::Sync& completer) override;
  void ResetTxPowerScenario(ResetTxPowerScenarioCompleter::Sync& completer) override;
  void GetTxPowerScenario(GetTxPowerScenarioCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_wlan_phy::WlanPhy> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  // NetworkDevice::Callbacks implementation
  void NetDevInit(wlan::drivers::components::NetworkDevice::Callbacks::InitTxn txn) override;
  void NetDevRelease() override;
  void NetDevStart(wlan::drivers::components::NetworkDevice::Callbacks::StartTxn txn) override;
  void NetDevStop(wlan::drivers::components::NetworkDevice::Callbacks::StopTxn txn) override;
  void NetDevGetInfo(fuchsia_hardware_network_driver::DeviceImplInfo* out_info) override;
  void NetDevQueueTx(cpp20::span<wlan::drivers::components::Frame> frames) override;
  void NetDevQueueRxSpace(
      cpp20::span<const fuchsia_hardware_network_driver::wire::RxSpaceBuffer> buffers_list,
      uint8_t* vmo_addrs[]) override;
  zx_status_t NetDevPrepareVmo(uint8_t vmo_id, zx::vmo vmo, uint8_t* mapped_address,
                               size_t mapped_size) override;
  void NetDevReleaseVmo(uint8_t vmo_id) override;

  virtual zx_status_t LoadFirmware(const char* path, zx_handle_t* fw, size_t* size) = 0;
  virtual zx::result<fuchsia_wlan_broadcom::WifiConfig> GetWifiConfig() = 0;

  virtual zx::result<std::vector<uint8_t>> DeviceGetPersistedMetadata(
      std::string_view metadata_serializable_name) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  // This is intended for implementations that want to perform additional actions when the driver's
  // recovery worker has finished.
  virtual void OnRecoveryComplete() {}

  // Fidl error handlers
  void on_fidl_error(fidl::UnbindInfo error) override {
    BRCMF_WARN("Fidl Error: %s", error.FormatDescription().c_str());
  }
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_driver_framework::NodeController> metadata) override {
    BRCMF_WARN("Received unknown event: event_ordinal(%lu)", metadata.event_ordinal);
  }
  // Helper functions
  void DestroyAllIfaces(fit::callback<void()>&& on_complete);
  void DestroyIface(uint16_t iface_id, fit::callback<void(zx_status_t)>&& on_complete);
  // fidl::WireServer<fuchsia_factory_wlan_iovar::Iovar> Implementation
  void Get(GetRequestView request, GetCompleter::Sync& _completer) override;
  void Set(SetRequestView request, SetCompleter::Sync& _completer) override;

 protected:
  // This should be called by bus implementations when the driver is being shut down, for example
  // during a reboot, power off or suspend. Because this Device class is not Resumable there is no
  // need to worry about coming back from a shutdown state, it's irreversible.
  void Shutdown(fit::callback<void()> on_shutdown_complete);

 private:
  // Helpers
  zx_status_t AddWlanPhyService();
  void ServiceConnectHandler(async_dispatcher_t* dispatcher,
                             fidl::ServerEnd<fuchsia_wlan_phy::WlanPhy> server_end);
  void NetDevInitReply(zx_status_t status);
  std::unique_ptr<WlanInterface>* GetInterfaceForRole(fuchsia_wlan_common::wire::WlanMacRole role);
  std::unique_ptr<WlanInterface>* GetInterfaceForId(uint16_t interface_id);
  void ServeFactory(fidl::ServerEnd<fuchsia_factory_wlan::Iovar> server) {
    factory_bindings_.AddBinding(fdf_dispatcher_get_async_dispatcher(GetDriverDispatcher()),
                                 std::move(server), this, fidl::kIgnoreBindingClosure);
  }
  zx_status_t AddFactoryNode();

  std::unique_ptr<brcmf_bus> brcmf_bus_;
  std::unique_ptr<brcmf_pub> brcmf_pub_;
  std::mutex lock_;

  // FIDL client of the node that this driver binds to.
  fidl::WireClient<fdf::NodeController> wlanphy_controller_client_;
  fidl::WireClient<fdf::NodeController> wlanfullmac_controller_client_;
  fidl::WireClient<fdf::NodeController> wlanfullmac_controller_softap_;

  fdf::Dispatcher netdev_dispatcher_;
  libsync::Completion netdev_dispatcher_shutdown_;
  fit::callback<void()> on_netdev_shutdown_complete_;

  ::wlan::drivers::components::NetworkDevice network_device_;
  // Two fixed interfaces supported;  the default interface as a client, and a second one as an AP.
  std::unique_ptr<WlanInterface> client_interface_;
  std::unique_ptr<WlanInterface> ap_interface_;
  fidl::ServerBindingGroup<fuchsia_wlan_phy::WlanPhy> bindings_;
  fidl::ServerBindingGroup<fuchsia_factory_wlan::Iovar> factory_bindings_;
  std::optional<wlan::drivers::components::NetworkDevice::Callbacks::InitTxn> netdev_init_txn_;
  driver_devfs::Connector<fuchsia_factory_wlan::Iovar> devfs_connector_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> factory_controller_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> factory_node_;
  bool device_powered_on_ = true;
  fidl::Client<fuchsia_wlan_phy::WlanPhyNotify> phy_notify_client_;
};

}  // namespace brcmfmac
}  // namespace wlan

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_DEVICE_H_
