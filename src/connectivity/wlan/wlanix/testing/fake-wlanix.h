// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_WLANIX_TESTING_FAKE_WLANIX_H_
#define SRC_CONNECTIVITY_WLAN_WLANIX_TESTING_FAKE_WLANIX_H_

#include <fidl/fuchsia.wlan.wlanix/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/zircon-internal/thread_annotations.h>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>

#include "fidl/fuchsia.wlan.wlanix/cpp/wire_types.h"

namespace wlanix_test {

enum class CommandTag {
  kWlanixGetWifi,
  kWlanixGetSupplicant,
  kWlanixGetNl80211,
  kWlanixGetWifiLegacyHal,
  kWlanixUnknownMethod,
  kWifiRegisterEventCallback,
  kWifiStart,
  kWifiStop,
  kWifiGetState,
  kWifiGetChipIds,
  kWifiGetChip,
  kWifiUnknownMethod,
  kWifiChipCreateStaIface,
  kWifiChipGetStaIfaceNames,
  kWifiChipGetStaIface,
  kWifiChipRemoveStaIface,
  kWifiChipSetCountryCode,
  kWifiChipGetSignalPollResults,
  kWifiChipGetAvailableModes,
  kWifiChipGetId,
  kWifiChipGetMode,
  kWifiChipGetCapabilities,
  kWifiChipUnknownMethod,
  kWifiChipTriggerSubsystemRestart,
  kWifiChipSelectTxPowerScenario,
  kWifiChipResetTxPowerScenario,
  kWifiStaIfaceGetName,
  kWifiStaIfaceSetScanOnlyMode,
  kWifiStaIfaceSetMacAddress,
  kWifiStaIfaceGetApfPacketFilterSupport,
  kWifiStaIfaceInstallApfPacketFilter,
  kWifiStaIfaceReadApfPacketFilterData,
  kWifiStaIfaceUnknownMethod,
  kSupplicantAddStaInterface,
  kSupplicantRemoveInterface,
  kSupplicantUnknownMethod,
  kSupplicantStaIfaceRegisterCallback,
  kSupplicantStaIfaceAddNetwork,
  kSupplicantStaIfaceDisconnect,
  kSupplicantStaIfaceGetMacAddress,
  kSupplicantStaIfaceGetFactoryMacAddress,
  kSupplicantStaIfaceSetBtCoexistenceMode,
  kSupplicantStaIfaceSetPowerSave,
  kSupplicantStaIfaceSetSuspendModeEnabled,
  kSupplicantStaIfaceSetStaCountryCode,
  kSupplicantStaIfaceUnknownMethod,
  kSupplicantStaNetworkSetBssid,
  kSupplicantStaNetworkClearBssid,
  kSupplicantStaNetworkSetSsid,
  kSupplicantStaNetworkSetKeyMgmt,
  kSupplicantStaNetworkSetPskPassphrase,
  kSupplicantStaNetworkSetSaePassword,
  kSupplicantStaNetworkSetWepKey,
  kSupplicantStaNetworkSetWepTxKeyIdx,
  kSupplicantStaNetworkSelect,
  kSupplicantStaNetworkUnknownMethod
};

struct Command {
  CommandTag tag;
  union {
    struct {
      uint32_t chip_id;
    } wifi_get_chip_args;
  } args;
};

class FakeWlanix : public fidl::WireServer<fuchsia_wlan_wlanix::Wlanix>,
                   public fidl::WireServer<fuchsia_wlan_wlanix::Wifi>,
                   public fidl::WireServer<fuchsia_wlan_wlanix::WifiChip>,
                   public fidl::WireServer<fuchsia_wlan_wlanix::WifiStaIface>,
                   public fidl::WireServer<fuchsia_wlan_wlanix::Supplicant>,
                   public fidl::WireServer<fuchsia_wlan_wlanix::SupplicantStaIface>,
                   public fidl::WireServer<fuchsia_wlan_wlanix::SupplicantStaNetwork> {
 public:
  void Connect(async_dispatcher_t* dispatcher,
               fidl::ServerEnd<fuchsia_wlan_wlanix::Wlanix> server_end);

  // Wlanix methods
  void GetWifi(fuchsia_wlan_wlanix::wire::WlanixGetWifiRequest* request,
               GetWifiCompleter::Sync& completer) override;
  void GetSupplicant(fuchsia_wlan_wlanix::wire::WlanixGetSupplicantRequest* request,
                     GetSupplicantCompleter::Sync& completer) override;
  void GetNl80211(fuchsia_wlan_wlanix::wire::WlanixGetNl80211Request* request,
                  GetNl80211Completer::Sync& completer) override;
  void GetWifiLegacyHal(fuchsia_wlan_wlanix::wire::WlanixGetWifiLegacyHalRequest* request,
                        GetWifiLegacyHalCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_wlan_wlanix::Wlanix> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // Wifi methods
  void RegisterEventCallback(fuchsia_wlan_wlanix::wire::WifiRegisterEventCallbackRequest* request,
                             RegisterEventCallbackCompleter::Sync& completer) override;
  void Start(StartCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void GetState(GetStateCompleter::Sync& completer) override;
  void GetChipIds(GetChipIdsCompleter::Sync& completer) override;
  void GetChip(fuchsia_wlan_wlanix::wire::WifiGetChipRequest* request,
               GetChipCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_wlan_wlanix::Wifi> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // WifiChip methods
  void CreateStaIface(fuchsia_wlan_wlanix::wire::WifiChipCreateStaIfaceRequest* request,
                      CreateStaIfaceCompleter::Sync& completer) override;
  void GetStaIfaceNames(GetStaIfaceNamesCompleter::Sync& completer) override;
  void GetStaIface(fuchsia_wlan_wlanix::wire::WifiChipGetStaIfaceRequest* request,
                   GetStaIfaceCompleter::Sync& completer) override;
  void RemoveStaIface(fuchsia_wlan_wlanix::wire::WifiChipRemoveStaIfaceRequest* request,
                      RemoveStaIfaceCompleter::Sync& completer) override;
  void SetCountryCode(fuchsia_wlan_wlanix::wire::WifiChipSetCountryCodeRequest* request,
                      SetCountryCodeCompleter::Sync& completer) override;
  void GetSignalPollResults(GetSignalPollResultsCompleter::Sync& completer) override;
  void GetAvailableModes(GetAvailableModesCompleter::Sync& completer) override;
  void GetId(GetIdCompleter::Sync& completer) override;
  void GetMode(GetModeCompleter::Sync& completer) override;
  void GetCapabilities(GetCapabilitiesCompleter::Sync& completer) override;
  void TriggerSubsystemRestart(TriggerSubsystemRestartCompleter::Sync& completer) override;
  void SelectTxPowerScenario(
      fuchsia_wlan_wlanix::wire::WifiChipSelectTxPowerScenarioRequest* request,
      SelectTxPowerScenarioCompleter::Sync& completer) override;
  void ResetTxPowerScenario(ResetTxPowerScenarioCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_wlan_wlanix::WifiChip> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // WifiStaIface methods
  void GetName(GetNameCompleter::Sync& completer) override;
  void SetScanOnlyMode(fuchsia_wlan_wlanix::wire::WifiStaIfaceSetScanOnlyModeRequest* request,
                       SetScanOnlyModeCompleter::Sync& completer) override;
  void SetMacAddress(fuchsia_wlan_wlanix::wire::WifiStaIfaceSetMacAddressRequest* request,
                     SetMacAddressCompleter::Sync& completer) override;
  void GetApfPacketFilterSupport(GetApfPacketFilterSupportCompleter::Sync& completer) override;
  void InstallApfPacketFilter(
      fuchsia_wlan_wlanix::wire::WifiStaIfaceInstallApfPacketFilterRequest* request,
      InstallApfPacketFilterCompleter::Sync& completer) override;
  void ReadApfPacketFilterData(ReadApfPacketFilterDataCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_wlan_wlanix::WifiStaIface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // Supplicant methods
  void AddStaInterface(fuchsia_wlan_wlanix::wire::SupplicantAddStaInterfaceRequest* request,
                       AddStaInterfaceCompleter::Sync& completer) override;
  void RemoveInterface(fuchsia_wlan_wlanix::wire::SupplicantRemoveInterfaceRequest* request,
                       RemoveInterfaceCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_wlan_wlanix::Supplicant> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // SupplicantStaIface methods
  void RegisterCallback(
      fuchsia_wlan_wlanix::wire::SupplicantStaIfaceRegisterCallbackRequest* request,
      RegisterCallbackCompleter::Sync& completer) override;
  void AddNetwork(fuchsia_wlan_wlanix::wire::SupplicantStaIfaceAddNetworkRequest* request,
                  AddNetworkCompleter::Sync& completer) override;
  void Disconnect(DisconnectCompleter::Sync& completer) override;
  void GetMacAddress(GetMacAddressCompleter::Sync& completer) override;
  void GetFactoryMacAddress(GetFactoryMacAddressCompleter::Sync& completer) override;
  void SetBtCoexistenceMode(
      fuchsia_wlan_wlanix::wire::SupplicantStaIfaceSetBtCoexistenceModeRequest* request,
      SetBtCoexistenceModeCompleter::Sync& completer) override;
  void SetPowerSave(fuchsia_wlan_wlanix::wire::SupplicantStaIfaceSetPowerSaveRequest* request,
                    SetPowerSaveCompleter::Sync& completer) override;
  void SetSuspendModeEnabled(
      fuchsia_wlan_wlanix::wire::SupplicantStaIfaceSetSuspendModeEnabledRequest* request,
      SetSuspendModeEnabledCompleter::Sync& completer) override;
  void SetStaCountryCode(
      fuchsia_wlan_wlanix::wire::SupplicantStaIfaceSetStaCountryCodeRequest* request,
      SetStaCountryCodeCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_wlan_wlanix::SupplicantStaIface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // SupplicantStaNetwork methods
  void SetBssid(fuchsia_wlan_wlanix::wire::SupplicantStaNetworkSetBssidRequest* request,
                SetBssidCompleter::Sync& completer) override;
  void ClearBssid(ClearBssidCompleter::Sync& completer) override;
  void SetSsid(fuchsia_wlan_wlanix::wire::SupplicantStaNetworkSetSsidRequest* request,
               SetSsidCompleter::Sync& completer) override;
  void SetKeyMgmt(fuchsia_wlan_wlanix::wire::SupplicantStaNetworkSetKeyMgmtRequest* request,
                  SetKeyMgmtCompleter::Sync& completer) override;
  void SetPskPassphrase(
      fuchsia_wlan_wlanix::wire::SupplicantStaNetworkSetPskPassphraseRequest* request,
      SetPskPassphraseCompleter::Sync& completer) override;
  void SetSaePassword(fuchsia_wlan_wlanix::wire::SupplicantStaNetworkSetSaePasswordRequest* request,
                      SetSaePasswordCompleter::Sync& completer) override;
  void SetWepKey(fuchsia_wlan_wlanix::wire::SupplicantStaNetworkSetWepKeyRequest* request,
                 SetWepKeyCompleter::Sync& completer) override;
  void SetWepTxKeyIdx(fuchsia_wlan_wlanix::wire::SupplicantStaNetworkSetWepTxKeyIdxRequest* request,
                      SetWepTxKeyIdxCompleter::Sync& completer) override;
  void Select(SelectCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_wlan_wlanix::SupplicantStaNetwork> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // test methods
  std::vector<Command> GetCommandTrace() {
    fbl::AutoLock al(&lock_);
    return command_trace_;
  }

 private:
  async_dispatcher_t* dispatcher_;
  mutable fbl::Mutex lock_;

  std::vector<Command> command_trace_ TA_GUARDED(lock_);
  void AppendCommand(Command cmd) {
    fbl::AutoLock al(&lock_);
    command_trace_.push_back(cmd);
  }
};

}  // namespace wlanix_test

#endif  // SRC_CONNECTIVITY_WLAN_WLANIX_TESTING_FAKE_WLANIX_H_
