// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SIM_TEST_SIM_TEST_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SIM_TEST_SIM_TEST_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <zircon/types.h>

#include <map>

#include <wlan/drivers/macaddr.h>
#include <zxtest/zxtest.h>

#include "lib/fdf/cpp/dispatcher.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-env.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-fake-ap/sim-fake-ap.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_data_path.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_device.h"
#include "zircon/system/ulib/sync/include/lib/sync/cpp/completion.h"

namespace wlan_fullmac_wire = fuchsia_wlan_fullmac::wire;
namespace wlan_phy_wire = fuchsia_wlan_phy::wire;

namespace wlan::brcmfmac {

const fuchsia_wlan_ieee80211::Ssid kDefaultSsid = {'F', 'u', 'c', 'h', 's', 'i', 'a', ' ',
                                                   'F', 'a', 'k', 'e', ' ', 'A', 'P'};
const fuchsia_wlan_ieee80211::Ssid kDefaultSoftApSsid = {'F', 'u', 'c', 'h', 's', 'i', 'a', ' ',
                                                         'F', 'a', 'k', 'e', ' ', 'A', 'P'};
// This class represents an interface created on a simulated device, collecting all of the
// attributes related to that interface.
class SimInterface : public fidl::WireServer<fuchsia_wlan_fullmac::WlanFullmacImplIfc> {
 public:
  // Track state of association
  struct AssocContext {
    enum AssocState {
      kNone,
      kAssociating,
      kAssociated,
    } state = kNone;

    wlan::common::MacAddr bssid;
    std::vector<uint8_t> ies;
    fuchsia_wlan_ieee80211::wire::ChannelNumber primary;
  };

  struct SoftApContext {
    fuchsia_wlan_ieee80211::Ssid ssid;
  };

  // Useful statistics about operations performed
  struct Stats {
    size_t connect_attempts = 0;
    size_t connect_successes = 0;
    size_t roam_attempts = 0;
    size_t roam_successes = 0;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcConnectConfRequest> connect_results;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcAssocIndRequest> assoc_indications;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcAuthIndRequest> auth_indications;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcDeauthConfRequest> deauth_results;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcDisassocConfRequest> disassoc_results;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcDeauthIndRequest> deauth_indications;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcDisassocIndRequest> disassoc_indications;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcRoamResultIndRequest> roam_result_indications;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcRoamConfRequest> roam_confirmations;
    std::list<wlan_fullmac_wire::WlanFullmacChannelSwitchInfo> csa_indications;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcStartConfRequest> start_confirmations;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcStopConfRequest> stop_confirmations;
  };

  // Default scan options
  static const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber> kDefaultScanChannels;
  static constexpr uint32_t kDefaultActiveScanDwellTimeMs = 40;
  static constexpr uint32_t kDefaultPassiveScanDwellTimeMs = 120;

  // SoftAP defaults
  static constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kDefaultSoftApChannelNum = {
      .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 11};
  static constexpr uint32_t kDefaultSoftApBeaconPeriod = 100;
  static constexpr uint32_t kDefaultSoftApDtimPeriod = 100;

  SimInterface();
  SimInterface(const SimInterface&) = delete;
  ~SimInterface();

  zx_status_t Init(simulation::Environment* env, wlan_common::WlanMacRole role);
  void Reset();

  // This function establish connection between this object and WlanInterface instance.
  zx_status_t Connect(fidl::ClientEnd<fuchsia_wlan_fullmac::WlanFullmacImpl> client_end,
                      async_dispatcher_t* server_dispatcher);

  // Default SME Callbacks
  // Implementation of wlan_fullmac_wire::WlanFullmacImplIfc.
  void OnScanResult(OnScanResultRequestView request,
                    OnScanResultCompleter::Sync& completer) override;
  void OnScanEnd(OnScanEndRequestView request, OnScanEndCompleter::Sync& completer) override;
  void OnScheduledScanMatchesAvailable(
      OnScheduledScanMatchesAvailableRequestView request,
      OnScheduledScanMatchesAvailableCompleter::Sync& completer) override;
  void OnScheduledScanStoppedByFirmware(
      OnScheduledScanStoppedByFirmwareRequestView request,
      OnScheduledScanStoppedByFirmwareCompleter::Sync& completer) override;
  void ConnectConf(ConnectConfRequestView request, ConnectConfCompleter::Sync& completer) override;
  void RoamConf(RoamConfRequestView request, RoamConfCompleter::Sync& completer) override;
  void RoamStartInd(RoamStartIndRequestView request,
                    RoamStartIndCompleter::Sync& completer) override;
  void RoamResultInd(RoamResultIndRequestView request,
                     RoamResultIndCompleter::Sync& completer) override;
  void AuthInd(AuthIndRequestView request, AuthIndCompleter::Sync& completer) override;
  void DeauthConf(DeauthConfRequestView request, DeauthConfCompleter::Sync& completer) override;
  void DeauthInd(DeauthIndRequestView request, DeauthIndCompleter::Sync& completer) override;
  void AssocInd(AssocIndRequestView request, AssocIndCompleter::Sync& completer) override;
  void DisassocConf(DisassocConfRequestView request,
                    DisassocConfCompleter::Sync& completer) override;
  void DisassocInd(DisassocIndRequestView request, DisassocIndCompleter::Sync& completer) override;
  void StartConf(StartConfRequestView request, StartConfCompleter::Sync& completer) override;
  void StopConf(StopConfRequestView request, StopConfCompleter::Sync& completer) override;
  void EapolConf(EapolConfRequestView request, EapolConfCompleter::Sync& completer) override;
  void OnChannelSwitch(OnChannelSwitchRequestView request,
                       OnChannelSwitchCompleter::Sync& completer) override;
  void SignalReport(SignalReportRequestView request,
                    SignalReportCompleter::Sync& completer) override;
  void EapolInd(EapolIndRequestView request, EapolIndCompleter::Sync& completer) override;
  void OnPmkAvailable(OnPmkAvailableRequestView request,
                      OnPmkAvailableCompleter::Sync& completer) override;
  void SaeHandshakeInd(SaeHandshakeIndRequestView request,
                       SaeHandshakeIndCompleter::Sync& completer) override;
  void SaeFrameRx(SaeFrameRxRequestView request, SaeFrameRxCompleter::Sync& completer) override;
  void OnWmmStatusResp(OnWmmStatusRespRequestView request,
                       OnWmmStatusRespCompleter::Sync& completer) override;

  // Query an interface
  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse Query();

  // Query for security feature support on an interface
  void QuerySecuritySupport(wlan_common::SecuritySupport* out_resp);

  // Query for spectrum management support on an interface
  void QuerySpectrumManagementSupport(wlan_common::SpectrumManagementSupport* out_resp);

  // Query for telemetry support on an interface
  void QueryTelemetrySupport(fuchsia_wlan_stats::wire::TelemetrySupport* out_resp);

  // Get the Mac address of an interface
  void GetMacAddr(wlan::common::MacAddr* out_macaddr);

  // Start an assocation with a fake AP. We can use these for subsequent association events, but
  // not interleaved association events (which I doubt are terribly useful, anyway). Note that for
  // the moment only non-authenticated associations are supported.
  void StartConnect(const wlan::common::MacAddr& bssid, const fuchsia_wlan_ieee80211::Ssid& ssid,
                    const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel,
                    fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw,
                    const fuchsia_wlan_ieee80211::wire::ChannelNumber& secondary80);
  void AssociateWith(const simulation::FakeAp& ap,
                     std::optional<zx::duration> delay = std::nullopt);

  // Start a roam attempt with a fake AP. Note: like connect, only non-authenticated associations
  // are supported.
  void StartRoam(const wlan::common::MacAddr& bssid,
                 const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel,
                 fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw,
                 const fuchsia_wlan_ieee80211::wire::ChannelNumber& secondary80);

  void DisassociateFrom(const wlan::common::MacAddr& bssid, wlan_ieee80211::ReasonCode reason);
  void DeauthenticateFrom(const wlan::common::MacAddr& bssid, wlan_ieee80211::ReasonCode reason);

  // Scan operations
  void StartScan(
      uint64_t txn_id = 0, bool active = false,
      std::optional<const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber>> channels =
          std::optional<const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber>>{});
  std::optional<wlan_fullmac_wire::WlanScanResult> ScanResultCode(uint64_t txn_id);
  const std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcOnScanResultRequest>* ScanResultList(
      uint64_t txn_id);

  // SoftAP operation
  void StartSoftAp(
      const fuchsia_wlan_ieee80211::Ssid& ssid = kDefaultSoftApSsid,
      const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel = kDefaultSoftApChannelNum,
      uint32_t beacon_period = kDefaultSoftApBeaconPeriod,
      uint32_t dtim_period = kDefaultSoftApDtimPeriod);
  void StopSoftAp();

  zx_status_t SetMulticastPromisc(bool enable);

  zx::channel TakeMlme();

  simulation::Environment* env_;

  fidl::WireSyncClient<fuchsia_wlan_fullmac::WlanFullmacImpl> client_;

  // Unique identifier provided by the driver
  uint16_t iface_id_;

  // Handles for SME <=> MLME communication, required but never used for communication (since no
  // SME is present).
  zx::channel ch_sme_;   // SME-owned side
  zx::channel ch_mlme_;  // MLME-owned side

  // Current state of association
  AssocContext assoc_ctx_ = {};

  // Current state of soft AP
  SoftApContext soft_ap_ctx_ = {};

  // Allows us to track individual operations
  Stats stats_ = {};

  fdf::Arena test_arena_;

 private:
  async_dispatcher_t* server_dispatcher_ = nullptr;

  std::unique_ptr<fidl::ServerBinding<fuchsia_wlan_fullmac::WlanFullmacImplIfc>> server_binding_ =
      nullptr;

  wlan_common::WlanMacRole role_;

  // Track scan results
  struct ScanStatus {
    // If not present, indicates that the scan has not completed yet
    std::optional<wlan_fullmac_wire::WlanScanResult> result_code = std::nullopt;
    std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcOnScanResultRequest> result_list;
  };
  // One entry per scan started
  std::map<uint64_t, ScanStatus> scan_results_;
  // BSS's IEs are raw pointers. Store the IEs here so we don't have dangling pointers
  std::vector<std::vector<uint8_t>> scan_results_ies_;
};

class SimEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    return zx::ok();
  }

  async_dispatcher_t* dispatcher() const { return dispatcher_; }

 private:
  async_dispatcher_t* dispatcher_ = nullptr;
};

class TestConfig final {
 public:
  using DriverType = brcmfmac::SimDevice;
  using EnvironmentType = SimEnvironment;
};

// A base class that can be used for creating simulation tests. It provides functionality that
// should be common to most tests (like creating a new device instance and setting up and plugging
// into the environment). It also provides a factory method for creating a new interface on the
// simulated device.
class SimTest : public ::zxtest::Test, public simulation::StationIfc {
 public:
  SimTest();
  ~SimTest();

  // In some cases (like error injection that affects the initialization) we want to work with
  // an uninitialized device. This method will allocate, but not initialize the device. To complete
  // initialization, the Init() function can be called after PreInit().
  zx_status_t PreInit();

  // Allocate device (if it hasn't already been allocated) and initialize it. This function doesn't
  // require PreInit() to be called first.
  zx_status_t Init();

 protected:
  // Create a new interface on the simulated device, providing the specified role and function
  // callbacks
  zx_status_t StartInterface(wlan_common::WlanMacRole role, SimInterface* sim_ifc,
                             std::optional<wlan::common::MacAddr> mac_addr = std::nullopt);

  // Stop and delete a SimInterface
  zx_status_t DeleteInterface(SimInterface* ifc);

  // To notify simulator that an interface was destroyed.
  // e.g. when going through crash recovery.
  zx_status_t InterfaceDestroyed(SimInterface* sim_ifc);

  uint32_t DeviceCount();
  uint32_t DeviceCountWithProperty(const fuchsia_driver_framework::NodeProperty2& property);

  // We don't have a good mechanism to synchronize the Remove call from
  // brcmfmac::Device with node_server_, so these functions repeatedly check the device count and
  // sleep until the device count matches the expected value.
  // The result is a timeout if it doesn't work instead of immediately failing, but the upside is
  // that we're no longer relying on the timing of the Remove call.
  void WaitForDeviceCount(uint32_t expected);
  void WaitForDeviceCountWithProperty(const fuchsia_driver_framework::NodeProperty2& property,
                                      uint32_t expected);

  // Wait for recovery to complete on the calling thread. Recovery happens on the driver dispatcher,
  // so this should never be called on the driver dispatcher. Doing so will deadlock the test.
  void WaitForRecoveryComplete();

  // Provides synchronous access to the brcmfmac::SimDevice instance via a callback. The callback
  // is posted to the SimDevice's dispatcher (i.e., driver_dispatcher_).
  //
  // This can only be called after PreInit().
  //
  // Note that there is a risk of deadlock here: if SimDevice makes a sync call to
  // WlanFullmacImplIfc (which blocks driver_dispatcher_ until the call is complete), and we try to
  // call WithSimDevice from the WlanFullmacImplIfc handler, it will deadlock because
  // driver_dispatcher_ is blocked from the original sync call from SimDevice.
  void WithSimDevice(fit::function<void(brcmfmac::SimDevice*)>);

  fidl::ClientEnd<fuchsia_io::Directory> CreateDriverSvcClient();

  async_dispatcher_t* df_env_dispatcher() {
    async_dispatcher_t* dispatcher = nullptr;
    driver_test().RunInEnvironmentTypeContext(
        [&](SimEnvironment& env) { dispatcher = env.dispatcher(); });
    return dispatcher;
  }

  fdf_testing::DriverRuntime& runtime() { return driver_test_.runtime(); }
  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }
  zx_status_t CreateFactoryClient();

  std::unique_ptr<simulation::Environment> env_;

  // Keep track of the ifaces we created during test by iface id.
  std::map<uint16_t, SimInterface*> ifaces_;

  fidl::SyncClient<fuchsia_wlan_phy::WlanPhy> client_;
  fidl::WireSyncClient<fuchsia_factory_wlan::Iovar> factory_client_;
  fdf::Arena test_arena_;

 private:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  fidl::ClientEnd<fuchsia_io::Directory> sim_outgoing_client_;

  bool driver_created_{false};

  // StationIfc methods - by default, do nothing. These can/will be overridden by superclasses.
  void Rx(std::shared_ptr<const simulation::SimFrame> frame,
          std::shared_ptr<const simulation::WlanRxInfo> info) override {}
};

}  // namespace wlan::brcmfmac

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SIM_TEST_SIM_TEST_H_
