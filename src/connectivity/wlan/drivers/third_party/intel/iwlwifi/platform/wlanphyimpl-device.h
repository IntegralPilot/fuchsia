// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_WLANPHYIMPL_DEVICE_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_WLANPHYIMPL_DEVICE_H_

#include <fidl/fuchsia.wlan.phy/cpp/fidl.h>

#include "third_party/iwlwifi/platform/banjo/common.h"

struct iwl_mvm_vif;
struct iwl_trans;

namespace wlan::iwlwifi {

class WlanPhyDevice : public fidl::Server<fuchsia_wlan_phy::WlanPhy> {
 public:
  WlanPhyDevice(const WlanPhyDevice& device) = delete;
  WlanPhyDevice& operator=(const WlanPhyDevice& other) = delete;
  virtual ~WlanPhyDevice();
  explicit WlanPhyDevice();

  // Implemented by driver class(PcieIwlwifiDriver).
  virtual zx_status_t AddWlansoftmacDevice(uint16_t iface_id, struct iwl_mvm_vif* mvmvif) = 0;
  virtual zx_status_t RemoveWlansoftmacDevice(uint16_t iface_id) = 0;

  // State accessors.
  virtual iwl_trans* drvdata() = 0;
  virtual const iwl_trans* drvdata() const = 0;

  void Init(InitRequest& request, InitCompleter::Sync& completer) override;
  void GetSupportedMacRoles(GetSupportedMacRolesCompleter::Sync& completer) override;
  void CreateIface(CreateIfaceRequest& request, CreateIfaceCompleter::Sync& completer) override;
  void DestroyIface(DestroyIfaceRequest& request, DestroyIfaceCompleter::Sync& completer) override;
  void SetCountry(SetCountryRequest& request, SetCountryCompleter::Sync& completer) override;
  void ClearCountry(ClearCountryCompleter::Sync& completer) override;
  void GetCountry(GetCountryCompleter::Sync& completer) override;
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

  void ServiceConnectHandler(async_dispatcher_t* dispatcher,
                             fidl::ServerEnd<fuchsia_wlan_phy::WlanPhy> server_end);

 protected:
  fidl::ServerBindingGroup<fuchsia_wlan_phy::WlanPhy> bindings_;
  std::optional<fidl::ClientEnd<fuchsia_wlan_phy::WlanPhyNotify>> notify_client_;
};

}  // namespace wlan::iwlwifi

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_WLANPHYIMPL_DEVICE_H_
