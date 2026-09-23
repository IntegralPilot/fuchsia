// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "third_party/iwlwifi/platform/wlanphyimpl-device.h"

#include <lib/sync/completion.h>
#include <zircon/status.h>

#include <algorithm>
#include <memory>

#include <wlan/drivers/macaddr.h>

#include "third_party/iwlwifi/platform/kernel.h"

extern "C" {
#include "third_party/iwlwifi/mvm/mvm.h"
}  // extern "C"

#include "third_party/iwlwifi/platform/mvm-mlme.h"
#include "third_party/iwlwifi/platform/wlansoftmac-device.h"

using ::wlan::common::MacAddr;

namespace wlan::iwlwifi {

WlanPhyDevice::WlanPhyDevice() = default;

WlanPhyDevice::~WlanPhyDevice() = default;

void WlanPhyDevice::GetSupportedMacRoles(GetSupportedMacRolesCompleter::Sync& completer) {
  fuchsia_wlan_common::WlanMacRole
      supported_mac_roles_list[fuchsia_wlan_common::kMaxSupportedMacRoles] = {};
  uint8_t supported_mac_roles_count = 0;
  zx_status_t status =
      phy_get_supported_mac_roles(drvdata(), supported_mac_roles_list, &supported_mac_roles_count);
  if (status != ZX_OK) {
    IWL_ERR(this, "failed get supported mac roles: %s", zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }

  if (supported_mac_roles_count > fuchsia_wlan_common::kMaxSupportedMacRoles) {
    IWL_ERR(this, "Too many mac roles returned");
    completer.Reply(zx::error(ZX_ERR_OUT_OF_RANGE));
    return;
  }

  std::vector<fuchsia_wlan_common::WlanMacRole> roles(
      supported_mac_roles_list, supported_mac_roles_list + supported_mac_roles_count);
  fuchsia_wlan_phy::WlanPhyGetSupportedMacRolesResponse response{{.supported_mac_roles = roles}};
  completer.Reply(zx::ok(std::move(response)));
}

void WlanPhyDevice::CreateIface(CreateIfaceRequest& request,
                                CreateIfaceCompleter::Sync& completer) {
  zx_status_t status = ZX_OK;
  if (!request.role().has_value() || !request.mlme_channel().has_value()) {
    IWL_ERR(this, "missing info in request");
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  uint16_t out_iface_id;
  wlan_phy_impl_create_iface_req_t create_iface_req{};

  if (request.init_sta_addr().has_value() && !MacAddr(*request.init_sta_addr()).IsZero()) {
    create_iface_req.has_init_sta_addr = true;
    std::copy(request.init_sta_addr()->begin(), request.init_sta_addr()->end(),
              create_iface_req.init_sta_addr);
  }

  switch (request.role().value()) {
    case fuchsia_wlan_common::WlanMacRole::kClient:
      create_iface_req.role = WLAN_MAC_ROLE_CLIENT;
      break;
    case fuchsia_wlan_common::WlanMacRole::kAp:
      create_iface_req.role = WLAN_MAC_ROLE_AP;
      break;
    case fuchsia_wlan_common::WlanMacRole::kMesh:
      create_iface_req.role = WLAN_MAC_ROLE_MESH;
      break;
    default:
      IWL_ERR(this, "Unrecognized WlanMacRole type");
      completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
      return;
  }

  create_iface_req.mlme_channel = request.mlme_channel().value().release();
  if ((status = phy_create_iface(drvdata(), &create_iface_req, &out_iface_id)) != ZX_OK) {
    IWL_ERR(this, "failed phy create: %s", zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }

  struct iwl_mvm* mvm = iwl_trans_get_mvm(drvdata());
  struct iwl_mvm_vif* mvmvif = mvm->mvmvif[out_iface_id];

  if ((status = AddWlansoftmacDevice(out_iface_id, mvmvif)) != ZX_OK) {
    IWL_ERR(this, "%s() failed mac device add: %s\n", __func__, zx_status_get_string(status));
    phy_create_iface_undo(drvdata(), out_iface_id);
    completer.Reply(zx::error(status));
    return;
  }
  IWL_INFO(this, "%s() created iface %u\n", __func__, out_iface_id);

  fuchsia_wlan_phy::WlanPhyCreateIfaceResponse response{{.iface_id = out_iface_id}};
  completer.Reply(zx::ok(std::move(response)));
}

void WlanPhyDevice::DestroyIface(DestroyIfaceRequest& request,
                                 DestroyIfaceCompleter::Sync& completer) {
  if (!request.iface_id().has_value()) {
    IWL_ERR(this, "invoked without valid iface id");
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  IWL_INFO(this, "destroying iface %u", request.iface_id().value());
  const int mac_id = 0;  // Assume only one interface is created.
  const auto iwl_trans = reinterpret_cast<struct iwl_trans*>(drvdata());
  struct iwl_mvm* mvm = iwl_trans_get_mvm(iwl_trans);
  struct iwl_mvm_vif* mvmvif = mvm->mvmvif[mac_id];

  mac_stop(mvmvif);
  zx_status_t status = phy_destroy_iface(drvdata(), request.iface_id().value());
  if (status != ZX_OK) {
    IWL_ERR(this, "failed destroy iface: %s", zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }
  if ((status = RemoveWlansoftmacDevice(request.iface_id().value())) != ZX_OK) {
    IWL_ERR(this, "%s() failed mac device remove: %s\n", __func__, zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }
  completer.Reply(zx::ok());
}

void WlanPhyDevice::SetCountry(SetCountryRequest& request, SetCountryCompleter::Sync& completer) {
  wlan_phy_country_t country;
  memcpy(&country.alpha2[0], request.country().data(), fuchsia_wlan_internal::kCountryCodeLen);
  zx_status_t status = phy_set_country(drvdata(), &country);
  if (status != ZX_OK) {
    IWL_ERR(this, "failed set country: %s", zx_status_get_string(status));
    completer.Reply(zx::error(status));
    return;
  }

  completer.Reply(zx::ok());
}

void WlanPhyDevice::ClearCountry(ClearCountryCompleter::Sync& completer) {
  IWL_ERR(this, "%s() not implemented ...\n", __func__);
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::GetCountry(GetCountryCompleter::Sync& completer) {
  std::array<uint8_t, fuchsia_wlan_internal::kCountryCodeLen> alpha2;

  wlan_phy_country_t country;
  zx_status_t status = phy_get_country(drvdata(), &country);
  if (status != ZX_OK) {
    completer.Reply(zx::error(status));
    IWL_ERR(this, "failed get country: %s", zx_status_get_string(status));
    return;
  }

  memcpy(alpha2.data(), country.alpha2, fuchsia_wlan_internal::kCountryCodeLen);

  completer.Reply(zx::ok(alpha2));
}

void WlanPhyDevice::SetPowerSaveMode(SetPowerSaveModeRequest& request,
                                     SetPowerSaveModeCompleter::Sync& completer) {
  IWL_ERR(this, "%s() not implemented ...\n", __func__);
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::GetPowerSaveMode(GetPowerSaveModeCompleter::Sync& completer) {
  IWL_ERR(this, "%s() not implemented ...\n", __func__);
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::Init(InitRequest& request, InitCompleter::Sync& completer) {
  if (!request.notify_client().has_value()) {
    IWL_ERR(this, "Failed to initialize WlanPhy server. notify_client client end not provided.");
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  notify_client_ = std::move(request.notify_client().value());
  completer.Reply(zx::ok(fuchsia_wlan_phy::WlanPhyInitResponse{}));
}

void WlanPhyDevice::PowerDown(PowerDownCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::PowerUp(PowerUpCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::Reset(ResetCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::GetPowerState(GetPowerStateCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::SetBtCoexistenceMode(SetBtCoexistenceModeRequest& request,
                                         SetBtCoexistenceModeCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::SetTxPowerScenario(SetTxPowerScenarioRequest& request,
                                       SetTxPowerScenarioCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::ResetTxPowerScenario(ResetTxPowerScenarioCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::GetTxPowerScenario(GetTxPowerScenarioCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void WlanPhyDevice::ServiceConnectHandler(async_dispatcher_t* dispatcher,
                                          fidl::ServerEnd<fuchsia_wlan_phy::WlanPhy> server_end) {
  bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
}

}  // namespace wlan::iwlwifi
