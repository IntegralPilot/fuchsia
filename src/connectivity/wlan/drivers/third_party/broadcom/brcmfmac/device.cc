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

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/device.h"

#include <lib/fdf/dispatcher.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/status.h>

#include <wlan/common/ieee80211.h>
#include <wlan/drivers/macaddr.h>

#include "fidl/fuchsia.wlan.phy/cpp/wire_types.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/cfg80211.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/common.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/debug.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/feature.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/fwil.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/wlan_interface.h"

using ::wlan::common::MacAddr;

namespace wlan {
namespace brcmfmac {
namespace {

constexpr char kClientInterfaceName[] = "brcmfmac-wlan-fullmac-client";
constexpr uint8_t kClientInterfaceId = 0;
constexpr char kApInterfaceName[] = "brcmfmac-wlan-fullmac-ap";
constexpr uint8_t kApInterfaceId = 1;
constexpr uint8_t kMaxBufferParts = 1;
constexpr uint32_t kMaxBufferLength = 4096;
constexpr uint32_t kBufferAlignment = 4096;
constexpr char kNetDevDriverName[] = "brcmfmac-netdev";

struct InterfaceInfo {
  const char* const display_name = nullptr;
  const char* const interface_name = nullptr;
  const uint8_t interface_id = 0;
  const bool supported = false;
};

InterfaceInfo GetInterfaceInfoForRole(fuchsia_wlan_common::WlanMacRole role) {
  switch (role) {
    case fuchsia_wlan_common::WlanMacRole::kClient:
      return InterfaceInfo{"Client", kClientInterfaceName, kClientInterfaceId, true};
    case fuchsia_wlan_common::WlanMacRole::kAp:
      return InterfaceInfo{"AP", kApInterfaceName, kApInterfaceId, true};
    case fuchsia_wlan_common::WlanMacRole::kMesh:
      return InterfaceInfo{"Mesh"};
    default:
      return InterfaceInfo{"<unknown>"};
  }
}

fuchsia_wlan_common::WlanMacRole GetMacRoleForInterfaceId(uint16_t interface_id) {
  switch (interface_id) {
    case kClientInterfaceId:
      return fuchsia_wlan_common::WlanMacRole::kClient;
    case kApInterfaceId:
      return fuchsia_wlan_common::WlanMacRole::kAp;
    default:
      return fuchsia_wlan_common::WlanMacRole();
  }
}

}  // namespace

Device::Device()
    : brcmf_pub_(std::make_unique<brcmf_pub>()),
      network_device_(this),
      devfs_connector_(fit::bind_member<&Device::ServeFactory>(this)) {
  brcmf_pub_->device = this;
  for (auto& entry : brcmf_pub_->if2bss) {
    entry = BRCMF_BSSIDX_INVALID;
  }

  // Initialize the recovery trigger for driver, shared by all buses' devices.
  auto recovery_start_callback = std::make_shared<std::function<zx_status_t()>>();
  *recovery_start_callback = std::bind(&brcmf_schedule_recovery_worker, brcmf_pub_.get());
  brcmf_pub_->recovery_trigger =
      std::make_unique<wlan::brcmfmac::RecoveryTrigger>(recovery_start_callback);
}

Device::~Device() = default;

void Device::Shutdown(fit::callback<void()> on_shutdown_complete) {
  if (brcmf_pub_) {
    // Shut down the default WorkQueue here to ensure that its dispatcher is shutdown properly even
    // if the Device object's destructor is not called.
    brcmf_pub_->default_wq.Shutdown();
  }

  DestroyAllIfaces([on_shutdown_complete = std::move(on_shutdown_complete), this]() mutable {
    on_netdev_shutdown_complete_ = [on_shutdown_complete = std::move(on_shutdown_complete),
                                    this]() mutable {
      if (netdev_dispatcher_.get()) {
        netdev_dispatcher_.ShutdownAsync();
        netdev_dispatcher_shutdown_.Wait();
        netdev_dispatcher_.close();
      }
      on_shutdown_complete();
    };

    if (!network_device_.Remove()) {
      // No removal needed, immediately call on_netdev_shutdown_complete to signal we're done
      on_netdev_shutdown_complete_();
    }
  });
}

zx_status_t Device::AddWlanPhyService() {
  // Add the service contains Wlanphy protocol to outgoing directory.
  auto wlanphy = [this](fidl::ServerEnd<fuchsia_wlan_phy::WlanPhy> server_end) {
    // Note: The same dispatcher here is used for fullmac device, will it affect the data path
    // performance?
    ServiceConnectHandler(fdf_dispatcher_get_async_dispatcher(GetDriverDispatcher()),
                          std::move(server_end));
  };

  fuchsia_wlan_phy::Service::InstanceHandler wlanphy_service_handler(
      {.device = std::move(wlanphy)});

  auto status =
      Outgoing()->AddService<fuchsia_wlan_phy::Service>(std::move(wlanphy_service_handler));
  if (status.is_error()) {
    BRCMF_ERR("Failed to add service to outgoing directory: %s", status.status_string());
    return status.status_value();
  }

  return ZX_OK;
}

zx_status_t Device::InitWlanPhy() {
  fidl::Arena arena;
  fidl::VectorView<fuchsia_driver_framework::wire::Offer> offers(arena, 1);
  offers[0] = fdf::MakeOffer2<fuchsia_wlan_phy::Service>(arena);

  auto args =
      fdf::wire::NodeAddArgs::Builder(arena).name("brcmfmac-wlanphy").offers2(offers).Build();

  auto endpoints = fidl::CreateEndpoints<fdf::NodeController>();
  if (endpoints.is_error()) {
    BRCMF_ERR("CreateEndPoints failed: %s", endpoints.status_string());
    return endpoints.error_value();
  }

  // Adding wlanphy child node. Doing a sync version here to reduce chaos.
  auto result = GetParentNode().sync()->AddChild(std::move(args), std::move(endpoints->server), {});

  if (!result.ok()) {
    BRCMF_ERR("Add wlanphy node error due to FIDL error on protocol [Node]: %s",
              result.status_string());
    return result.status();
  }

  if (result->is_error()) {
    BRCMF_ERR("Add wlanphy node error: %u", static_cast<uint32_t>(result->error_value()));
    return result.status();
  }

  wlanphy_controller_client_.Bind(std::move(endpoints->client),
                                  fdf::Dispatcher::GetCurrent()->async_dispatcher(), this);

  return ZX_OK;
}

zx_status_t Device::InitDevice(fdf::OutgoingDirectory& outgoing,
                               const std::shared_ptr<fdf::Namespace>& incoming) {
  auto netdev_dispatcher = fdf::SynchronizedDispatcher::Create(
      {}, "brcmfmac-netdev", [this](fdf_dispatcher_t*) { netdev_dispatcher_shutdown_.Signal(); });
  if (netdev_dispatcher.is_error()) {
    BRCMF_ERR("Failed to create netdev dispatcher: %s", netdev_dispatcher.status_string());
    return netdev_dispatcher.status_value();
  }
  netdev_dispatcher_ = std::move(netdev_dispatcher.value());

  zx_status_t status = BusInit(incoming);
  if (status != ZX_OK) {
    BRCMF_ERR("Init failed: %s", zx_status_get_string(status));
    return status;
  }
  status = network_device_.Initialize(GetParentNode(), netdev_dispatcher_.get(), outgoing,
                                      kNetDevDriverName);
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to initialize network device %s", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

void Device::InitPhyDevice() {
  // Setup the WlanPhy Service
  zx_status_t status;

  if ((status = AddWlanPhyService()) != ZX_OK) {
    BRCMF_ERR("ServeRuntimeProtocolForV1Devices failed: %s", zx_status_get_string(status));
    NetDevInitReply(status);
    return;
  }
  if ((status = InitWlanPhy()) != ZX_OK) {
    BRCMF_ERR("Init WlanPhy failed: %s", zx_status_get_string(status));
    NetDevInitReply(status);
    return;
  }
  if (AddFactoryNode() != ZX_OK) {
    BRCMF_ERR("Unable to add Factory node");
    // Ignore this error as external tool support is a debug functionality. We do not want to
    // prevent the driver from loading because of this error.
  }
  NetDevInitReply(ZX_OK);
}

zx_status_t Device::AddFactoryNode() {
  zx::result connector =
      devfs_connector_.Bind(fdf_dispatcher_get_async_dispatcher(GetDriverDispatcher()));
  if (connector.is_error()) {
    BRCMF_ERR("Failed to bind devfs connecter to dispatcher: %s", connector.status_string());
    return connector.error_value();
  }

  fidl::Arena args_arena;
  auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(args_arena)
                   .connector(std::move(connector.value()))
                   .class_name("wlan-factory")
                   .Build();

  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(args_arena)
                  .name("factory-broadcom")
                  .devfs_args(devfs)
                  .Build();

  auto controller_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
  if (controller_endpoints.is_error()) {
    BRCMF_ERR("Create node controller end points failed: %s",
              zx_status_get_string(controller_endpoints.error_value()));
    return controller_endpoints.error_value();
  }

  // Create the endpoints of fuchsia_driver_framework::Node protocol for the child node, and hold
  // the client end of it, because no driver will bind to the child node.
  auto child_node_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::Node>();
  if (child_node_endpoints.is_error()) {
    BRCMF_ERR("Create child node end points failed: %s",
              zx_status_get_string(child_node_endpoints.error_value()));
    return child_node_endpoints.error_value();
  }

  // Add factory-broadcom child node.
  auto result =
      GetParentNode().sync()->AddChild(std::move(args), std::move(controller_endpoints->server),
                                       std::move(child_node_endpoints->server));
  if (!result.ok()) {
    BRCMF_ERR("Failed to add factory child, status: %s", result.status_string());
    return result.status();
  }
  factory_controller_node_.Bind(std::move(controller_endpoints->client));
  factory_node_.Bind(std::move(child_node_endpoints->client));
  return ZX_OK;
}

brcmf_pub* Device::drvr() { return brcmf_pub_.get(); }

const brcmf_pub* Device::drvr() const { return brcmf_pub_.get(); }

void Device::Init(InitRequest& request, InitCompleter::Sync& completer) {
  if (!request.notify_client().has_value()) {
    BRCMF_ERR("Failed to initialize WlanPhy server. notify_client client end not provided.");
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  phy_notify_client_.Bind(std::move(request.notify_client().value()),
                          fdf_dispatcher_get_async_dispatcher(GetDriverDispatcher()));
  completer.Reply(fit::ok(fuchsia_wlan_phy::WlanPhyInitResponse{}));
}

void Device::GetSupportedMacRoles(GetSupportedMacRolesCompleter::Sync& completer) {
  BRCMF_DBG(WLANPHY, "Received request for supported MAC roles from SME dfv2");
  if (!device_powered_on_) {
    BRCMF_ERR("Device is powered off");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }

  fuchsia_wlan_common::WlanMacRole
      supported_mac_roles_list[fuchsia_wlan_common::kMaxSupportedMacRoles] = {};
  uint8_t supported_mac_roles_count = 0;
  zx_status_t status = WlanInterface::GetSupportedMacRoles(
      brcmf_pub_.get(), supported_mac_roles_list, &supported_mac_roles_count);
  if (status != ZX_OK) {
    BRCMF_ERR("Device::GetSupportedMacRoles() failed to get supported mac roles: %s\n",
              zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }

  if (supported_mac_roles_count > fuchsia_wlan_common::kMaxSupportedMacRoles) {
    BRCMF_ERR(
        "Device::GetSupportedMacRoles() Too many mac roles returned from brcmfmac driver. Number "
        "of supported max roles got "
        "from driver is %u, but the limitation is: %u\n",
        supported_mac_roles_count, fuchsia_wlan_common::kMaxSupportedMacRoles);
    completer.Reply(zx::error(ZX_ERR_OUT_OF_RANGE));
    return;
  }

  std::vector<fuchsia_wlan_common::WlanMacRole> roles;
  for (uint8_t i = 0; i < supported_mac_roles_count; i++) {
    roles.push_back(supported_mac_roles_list[i]);
  }

  fuchsia_wlan_phy::WlanPhyGetSupportedMacRolesResponse response{
      {.supported_mac_roles = std::move(roles)}};
  completer.Reply(zx::ok(std::move(response)));
}

void Device::CreateIface(CreateIfaceRequest& request, CreateIfaceCompleter::Sync& completer) {
  if (!device_powered_on_) {
    BRCMF_ERR("Device is powered off");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (!request.role().has_value() || !request.mlme_channel().has_value()) {
    BRCMF_ERR("Device::CreateIface() missing information in role(%d), channel(%d)",
              request.role().has_value(), request.mlme_channel().has_value());
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (request.role().value().IsUnknown()) {
    BRCMF_ERR("Invalid MAC role %u", fidl::ToUnderlying(request.role().value()));
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  fuchsia_wlan_common::WlanMacRole role = request.role().value();
  zx::channel mlme_channel = std::move(request.mlme_channel().value());

  // If we are operating with manufacturing firmware ensure SoftAP IF is also not present
  if (brcmf_feat_is_enabled(brcmf_pub_.get(), BRCMF_FEAT_MFG)) {
    if (ap_interface_ || client_interface_) {
      // Either the interface we're trying to create exists or the other one exists. Neither is
      // supported in manufacturing FW.
      BRCMF_ERR("Simultaneous mode not supported in mfg FW - IF already exists");
      completer.Reply(zx::error(ZX_ERR_NO_RESOURCES));
      return;
    }
  }

  std::unique_ptr<WlanInterface>* interface = GetInterfaceForRole(role);
  if (!interface) {
    BRCMF_ERR("MAC role %u not supported", fidl::ToUnderlying(role));
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  InterfaceInfo info = GetInterfaceInfoForRole(role);
  if (*interface) {
    BRCMF_ERR("Device::CreateIface() %s interface already exists", info.display_name);
    completer.Reply(zx::error(ZX_ERR_NO_RESOURCES));
    return;
  }

  wireless_dev* wdev = nullptr;
  std::optional<MacAddr> mac_addr;
  if (request.init_sta_addr().has_value()) {
    MacAddr addr(request.init_sta_addr().value().data());
    if (!addr.IsZero()) {
      mac_addr = addr;
    }
  }

  const zx_status_t status =
      brcmf_cfg80211_add_iface(brcmf_pub_.get(), info.interface_name, nullptr, role, mac_addr,
                               std::move(mlme_channel), &wdev);
  if (status != ZX_OK) {
    BRCMF_ERR("Device::CreateIface() failed to create %s interface, %s", info.display_name,
              zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }

  WlanInterface::Create(
      this, info.interface_name, wdev, role, info.interface_id,
      [info, interface, wdev, this,
       completer = completer.ToAsync()](zx::result<std::unique_ptr<WlanInterface>> result) mutable {
        if (result.is_error()) {
          BRCMF_ERR("Failed to create WlanInterface: %s", result.status_string());
          completer.Reply(zx::error(result.status_value()));
          return;
        }
        {
          // Hold the lock while modifying iface_ptr which points to a member of Device.
          std::lock_guard<std::mutex> lock(lock_);
          *interface = std::move(result.value());
        }

        net_device* ndev = wdev->netdev;
        BRCMF_DBG(WLANPHY, "Created %s iface with netdev:%s id:%d", info.display_name, ndev->name,
                  info.interface_id);
#if !defined(NDEBUG)
        const uint8_t* mac_addr = ndev_to_if(ndev)->mac_addr;
        BRCMF_DBG(WLANPHY, "  address: " FMT_MAC, FMT_MAC_ARGS(mac_addr));
#endif /* !defined(NDEBUG) */

        fuchsia_wlan_phy::WlanPhyCreateIfaceResponse response{{.iface_id = info.interface_id}};
        completer.Reply(zx::ok(response));
      });
}

void Device::DestroyIface(DestroyIfaceRequest& request, DestroyIfaceCompleter::Sync& completer) {
  if (!request.iface_id().has_value()) {
    BRCMF_ERR("Device::DestroyIface() invoked without valid iface_id");
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  DestroyIface(request.iface_id().value(),
               [completer = completer.ToAsync()](zx_status_t status) mutable {
                 completer.Reply(zx::make_result(status));
               });
}

void Device::DestroyIface(uint16_t iface_id, fit::callback<void(zx_status_t)>&& on_complete) {
  std::lock_guard<std::mutex> lock(lock_);

  std::unique_ptr<WlanInterface>* iface_ptr = GetInterfaceForId(iface_id);
  if (!iface_ptr) {
    BRCMF_ERR("Device::DestroyIface() Unknown interface id: %d", iface_id);
    on_complete(ZX_ERR_NOT_FOUND);
    return;
  }

  const InterfaceInfo info = GetInterfaceInfoForRole(GetMacRoleForInterfaceId(iface_id));

  // First check if the interface is present
  WlanInterface* iface = iface_ptr->get();
  if (!iface) {
    // Check the pointer inside the pointer, the actual interface pointer.
    BRCMF_WARN("%s interface not found", info.display_name);
    on_complete(ZX_ERR_NOT_FOUND);
    return;
  }

  // Beyond this point interface deletion might involve accessing the hardware. So fail the request
  // if the device is powered off or in the middle of reset.
  if (!device_powered_on_) {
    BRCMF_ERR("Device is powered off");
    on_complete(ZX_ERR_BAD_STATE);
    return;
  }

  BRCMF_DBG(WLANPHY, "Destroying %s interface", info.display_name);
  iface->DestroyIface([this, iface_ptr, iface_id, info,
                       on_complete = std::move(on_complete)](zx_status_t status) mutable {
    if (status != ZX_OK) {
      // Don't reset iface_ptr here since we failed to delete it.
      BRCMF_ERR("Device::DestroyIface() Error destroying %s interface : %s", info.display_name,
                zx_status_get_string(status));
      on_complete(status);
      return;
    }
    BRCMF_DBG(WLANPHY, "%s interface %u destroyed successfully", info.display_name, iface_id);
    {
      // Hold the lock while modifying iface_ptr which points to a member of Device.
      std::lock_guard<std::mutex> lock(lock_);
      iface_ptr->reset();
    }
    on_complete(ZX_OK);
  });
}

void Device::SetCountry(SetCountryRequest& request, SetCountryCompleter::Sync& completer) {
  BRCMF_DBG(WLANPHY, "Setting country code dfv2");
  if (!device_powered_on_) {
    BRCMF_ERR("Device is powered off");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }

  zx_status_t status = WlanInterface::SetCountry(brcmf_pub_.get(), request.country());
  if (status != ZX_OK) {
    BRCMF_ERR("Device::SetCountry() Failed Set country : %s", zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }
  completer.Reply(zx::ok());
}

void Device::ClearCountry(ClearCountryCompleter::Sync& completer) {
  BRCMF_DBG(WLANPHY, "Clearing country dfv2");
  if (!device_powered_on_) {
    BRCMF_ERR("Device is powered off");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  zx_status_t status = WlanInterface::ClearCountry(brcmf_pub_.get());
  if (status != ZX_OK) {
    BRCMF_ERR("Device::ClearCountry() Failed Clear country : %s", zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }

  completer.Reply(zx::ok());
}

void Device::GetCountry(GetCountryCompleter::Sync& completer) {
  BRCMF_DBG(WLANPHY, "Received request for country from SME dfv2");
  if (!device_powered_on_) {
    BRCMF_ERR("Device is powered off");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  std::array<uint8_t, 2> country = {};

  zx_status_t status = WlanInterface::GetCountry(brcmf_pub_.get(), &country);
  if (status != ZX_OK) {
    BRCMF_ERR("Device::GetCountry() Failed Get country : %s", zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }
  BRCMF_INFO("Get country code: %c%c", country[0], country[1]);

  fuchsia_wlan_phy::WlanPhyGetCountryResponse response{{.country = country}};
  completer.Reply(zx::ok(response));
}

void Device::SetPowerSaveMode(SetPowerSaveModeRequest& request,
                              SetPowerSaveModeCompleter::Sync& completer) {
  BRCMF_DBG(WLANPHY, "Setting power save mode dfv2");
  if (!device_powered_on_) {
    BRCMF_ERR("Device is powered off");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (!request.ps_mode().has_value()) {
    BRCMF_ERR("Device::SetPowerSaveMode() invoked without ps_mode");
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  zx_status_t status = brcmf_set_power_save_mode(brcmf_pub_.get(), request.ps_mode().value());
  if (status != ZX_OK) {
    BRCMF_ERR("Device::SetPowerSaveMode() Set Power Save Mode failed");
    completer.Reply(zx::error(status));
    return;
  }
  completer.Reply(zx::ok());
}

void Device::GetPowerSaveMode(GetPowerSaveModeCompleter::Sync& completer) {
  BRCMF_DBG(WLANPHY, "Received request for PS mode from SME dfv2");
  if (!device_powered_on_) {
    BRCMF_ERR("Device is powered off");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  fuchsia_wlan_common::PowerSaveType ps_mode;
  zx_status_t status = brcmf_get_power_save_mode(brcmf_pub_.get(), &ps_mode);
  if (status != ZX_OK) {
    BRCMF_ERR("Device::GetPowerSaveMode() Get Power Save Mode failed");
    completer.Reply(zx::error(status));
    return;
  }

  fuchsia_wlan_phy::WlanPhyGetPowerSaveModeResponse response{{.ps_mode = ps_mode}};
  completer.Reply(zx::ok(response));
}

void Device::PowerDown(PowerDownCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void Device::PowerUp(PowerUpCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void Device::Reset(ResetCompleter::Sync& completer) {
  if (!brcmf_pub_) {
    BRCMF_ERR("brmcf_pub_ is null");
    completer.Reply(zx::error(ZX_ERR_INTERNAL));
    return;
  }

  bool expected = false;
  if (!brcmf_pub_->drvr_resetting.compare_exchange_strong(expected, true)) {
    BRCMF_WARN("Driver is already resetting. Crash recovery may be in progress.");
    completer.Reply(zx::error(ZX_ERR_UNAVAILABLE));
    return;
  }

  auto finish_reset = fit::defer([this]() { brcmf_pub_->drvr_resetting.store(false); });

  if (!device_powered_on_) {
    BRCMF_ERR("Device is powered off, possibly in the middle of Reset already?");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  device_powered_on_ = false;

  DestroyAllIfaces(
      [this, finish_reset = std::move(finish_reset), completer = completer.ToAsync()]() mutable {
        zx_status_t status = brcmf_suspend_chip(brcmf_pub_.get());
        if (status != ZX_OK) {
          BRCMF_ERR("Suspend chip failed: %s", zx_status_get_string(status));
          // Ignore the error and attempt to power up since it has reached a point of no return.
        }
        status = brcmf_resume_chip(brcmf_pub_.get());
        if (status != ZX_OK) {
          BRCMF_ERR("Powerup failed: %s", zx_status_get_string(status));
          completer.Reply(zx::error(status));
          return;
        }
        device_powered_on_ = true;
        completer.Reply(zx::ok());
      });
}

void Device::GetPowerState(GetPowerStateCompleter::Sync& completer) {
  fuchsia_wlan_phy::WlanPhyGetPowerStateResponse response{{.power_on = device_powered_on_}};
  completer.Reply(zx::ok(response));
}

void Device::SetBtCoexistenceMode(SetBtCoexistenceModeRequest& request,
                                  SetBtCoexistenceModeCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void Device::SetTxPowerScenario(SetTxPowerScenarioRequest& request,
                                SetTxPowerScenarioCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void Device::ResetTxPowerScenario(ResetTxPowerScenarioCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void Device::GetTxPowerScenario(GetTxPowerScenarioCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void Device::ServiceConnectHandler(async_dispatcher_t* dispatcher,
                                   fidl::ServerEnd<fuchsia_wlan_phy::WlanPhy> server_end) {
  bindings_.AddBinding(dispatcher, std::move(server_end), this, [](fidl::UnbindInfo info) {
    if (!info.is_user_initiated()) {
      BRCMF_ERR("WlanPhy binding unexpectedly closed: %s", info.lossy_description());
    }
  });
}

void Device::NetDevInitReply(zx_status_t status) {
  if (!netdev_init_txn_.has_value()) {
    BRCMF_ERR("NetDev Init Txn is not valid");
    return;
  }
  netdev_init_txn_.value().Reply(static_cast<int32_t>(status));
  netdev_init_txn_.reset();
}

void Device::NetDevInit(wlan::drivers::components::NetworkDevice::Callbacks::InitTxn txn) {
  netdev_init_txn_.emplace(std::move(txn));
  zx_status_t status = async::PostTask(fdf_dispatcher_get_async_dispatcher(GetDriverDispatcher()),
                                       [this] { InitPhyDevice(); });
  if (status != ZX_OK) {
    BRCMF_ERR("Async PostTask failed: %s", zx_status_get_string(status));
    NetDevInitReply(status);
  }
}

void Device::NetDevRelease() {
  // This will be called as the final step of the network device removal. We can now call the
  // shutdown handler to indicate that everything was shut down properly.
  if (on_netdev_shutdown_complete_) {
    on_netdev_shutdown_complete_();
  }
}

void Device::NetDevStart(wlan::drivers::components::NetworkDevice::Callbacks::StartTxn txn) {
  txn.Reply(ZX_OK);
}

void Device::NetDevStop(wlan::drivers::components::NetworkDevice::Callbacks::StopTxn txn) {
  // Flush all buffers in response to this call. They are no longer valid for use.
  brcmf_flush_buffers(drvr());
  txn.Reply();
}

void Device::NetDevGetInfo(fuchsia_hardware_network_driver::DeviceImplInfo* out_info) {
  std::lock_guard<std::mutex> lock(lock_);

  uint16_t tx_depth = 0;
  zx_status_t err = brcmf_get_tx_depth(drvr(), &tx_depth);
  ZX_ASSERT(err == ZX_OK);
  uint16_t rx_depth = 0;
  err = brcmf_get_rx_depth(drvr(), &rx_depth);
  ZX_ASSERT(err == ZX_OK);
  uint16_t tx_tail_length = 0;
  brcmf_get_tail_length(drvr(), &tx_tail_length);

  out_info->tx_depth() = tx_depth;
  out_info->rx_depth() = rx_depth;
  out_info->rx_threshold() = rx_depth / 3;
  out_info->max_buffer_parts() = kMaxBufferParts;
  out_info->max_buffer_length() = kMaxBufferLength;
  out_info->buffer_alignment() = kBufferAlignment;
  out_info->min_rx_buffer_length() = IEEE80211_MSDU_SIZE_MAX;
  out_info->tx_head_length() = drvr()->hdrlen;
  out_info->tx_tail_length() = tx_tail_length;
}

void Device::NetDevQueueTx(cpp20::span<wlan::drivers::components::Frame> frames) {
  brcmf_start_xmit(drvr(), frames);
}

void Device::NetDevQueueRxSpace(
    cpp20::span<const fuchsia_hardware_network_driver::wire::RxSpaceBuffer> buffers,
    uint8_t* vmo_addrs[]) {
  brcmf_queue_rx_space(drvr(), buffers, vmo_addrs);
}

zx_status_t Device::NetDevPrepareVmo(uint8_t vmo_id, zx::vmo vmo, uint8_t* mapped_address,
                                     size_t mapped_size) {
  return brcmf_prepare_vmo(drvr(), vmo_id, vmo.get(), mapped_address, mapped_size);
}

void Device::NetDevReleaseVmo(uint8_t vmo_id) { brcmf_release_vmo(drvr(), vmo_id); }

void Device::DestroyAllIfaces(fit::callback<void()>&& on_complete) {
  std::lock_guard<std::mutex> lock(lock_);

  // Pick an interface to start destroying. By moving the pointer we ensure that the next recursive
  // call to DestroyAllIfaces will not pick up that pointer again.
  std::unique_ptr<WlanInterface> interface =
      client_interface_ ? std::move(client_interface_) : std::move(ap_interface_);

  if (!interface) {
    // No interfaces left to destroy, call on_complete
    on_complete();
    return;
  }

  // Capture interface to keep it alive until destruction completes. Use a raw pointer to safely
  // call into it after moving it.
  WlanInterface* interface_ptr = interface.get();
  interface_ptr->DestroyIface([this, interface = std::move(interface),
                               on_complete = std::move(on_complete)](zx_status_t status) mutable {
    if (status != ZX_OK) {
      InterfaceInfo info = GetInterfaceInfoForRole(interface->Role());
      BRCMF_ERR("Device::DestroyAllIfaces() : Failed to destroy %s interface : %s",
                info.display_name, zx_status_get_string(status));
    }
    // The interface here is moved out of the Device object, it can no longer be accessed by any
    // other code so no locking is needed here.
    interface.reset();

    // Call DestroyAllIfaces again to destroy the next interface, if any.
    DestroyAllIfaces(std::move(on_complete));
  });
}

std::unique_ptr<WlanInterface>* Device::GetInterfaceForRole(fuchsia_wlan_common::WlanMacRole role) {
  switch (role) {
    case fuchsia_wlan_common::WlanMacRole::kClient:
      return &client_interface_;
    case fuchsia_wlan_common::WlanMacRole::kAp:
      return &ap_interface_;
    default:
      return nullptr;
  }
}

std::unique_ptr<WlanInterface>* Device::GetInterfaceForId(uint16_t interface_id) {
  switch (interface_id) {
    case kClientInterfaceId:
      return &client_interface_;
    case kApInterfaceId:
      return &ap_interface_;
    default:
      return nullptr;
  }
}

void Device::Get(GetRequestView request, GetCompleter::Sync& completer) {
  zx_status_t status =
      brcmf_send_cmd_to_firmware(brcmf_pub_.get(), request->iface_idx, request->cmd,
                                 (void*)request->request.data(), request->request.size(), false);
  if (status == ZX_OK) {
    completer.ReplySuccess(request->request);
  } else {
    completer.Reply(zx::error(status));
  }
}

void Device::Set(SetRequestView request, SetCompleter::Sync& completer) {
  zx_status_t status =
      brcmf_send_cmd_to_firmware(brcmf_pub_.get(), request->iface_idx, request->cmd,
                                 (void*)request->request.data(), request->request.size(), true);
  if (status == ZX_OK) {
    completer.ReplySuccess();
  } else {
    completer.Reply(zx::error(status));
  }
}

}  // namespace brcmfmac
}  // namespace wlan
