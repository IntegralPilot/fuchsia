// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/wlan/ieee80211/cpp/fidl.h>
#include <fuchsia/wlan/stats/cpp/fidl.h>
#include <zircon/errors.h>

#include <wlan/common/channel.h>
#include <wlan/drivers/macaddr.h>
#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-fake-ap/sim-fake-ap.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/cfg80211.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/common.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/fweh.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/fwil.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"
#include "src/devices/lib/broadcom/commands.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {

namespace wlan_ieee80211 = wlan_ieee80211;

// Some default AP and association request values
constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kDefaultChannel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 9};
constexpr uint8_t kIes[] = {
    // SSID
    0x00, 0x0f, 'F', 'u', 'c', 'h', 's', 'i', 'a', ' ', 'F', 'a', 'k', 'e', ' ', 'A', 'P',
    // Supported rates
    0x01, 0x08, 0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c,
    // DS parameter set - channel 157
    0x03, 0x01, 0x9d,
    // DTIM
    0x05, 0x04, 0x00, 0x01, 0x00, 0x00,
    // Power constraint
    0x20, 0x01, 0x03,
    // HT capabilities
    0x2d, 0x1a, 0xef, 0x09, 0x1b, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // HT operation
    0x3d, 0x16, 0x9d, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // Overlapping BSS scan parameters
    0x4a, 0x0e, 0x14, 0x00, 0x0a, 0x00, 0x2c, 0x01, 0xc8, 0x00, 0x14, 0x00, 0x05, 0x00, 0x19, 0x00,
    // Extended capabilities
    0x7f, 0x08, 0x01, 0x00, 0x0f, 0x00, 0x00, 0x00, 0x00, 0x40,
    // VHT capabilities
    0xbf, 0x0c, 0xb2, 0x01, 0x80, 0x33, 0xea, 0xff, 0x00, 0x00, 0xea, 0xff, 0x00, 0x00,
    // VHT operation
    0xc0, 0x05, 0x01, 0x9b, 0x00, 0xfc, 0xff,
    // VHT Tx power envelope
    0xc3, 0x04, 0x02, 0xc4, 0xc4, 0xc4,
    // Vendor IE - WMM parameters
    0xdd, 0x18, 0x00, 0x50, 0xf2, 0x02, 0x01, 0x01, 0x80, 0x00, 0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4,
    0x00, 0x00, 0x42, 0x43, 0x5e, 0x00, 0x62, 0x32, 0x2f, 0x00,
    // Vendor IE - Atheros advanced capability
    0xdd, 0x09, 0x00, 0x03, 0x7f, 0x01, 0x01, 0x00, 0x00, 0xff, 0x7f,
    // RSN
    0x30, 0x14, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00,
    0x00, 0x0f, 0xac, 0x02, 0x00, 0x00,
    // Vendor IE - WPS
    0xdd, 0x1d, 0x00, 0x50, 0xf2, 0x04, 0x10, 0x4a, 0x00, 0x01, 0x10, 0x10, 0x44, 0x00, 0x01, 0x02,
    0x10, 0x3c, 0x00, 0x01, 0x03, 0x10, 0x49, 0x00, 0x06, 0x00, 0x37, 0x2a, 0x00, 0x01, 0x20};
const MacAddr kDefaultBssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc});
const MacAddr kMadeupClient({0xde, 0xad, 0xbe, 0xef, 0x00, 0x01});
constexpr auto kDefaultApDisassocReason = wlan_ieee80211::ReasonCode::kUnspecifiedReason;
constexpr auto kDefaultClientDisassocReason = wlan_ieee80211::ReasonCode::kUnspecifiedReason;
constexpr auto kDefaultApDeauthReason = wlan_ieee80211::ReasonCode::kInvalidAuthentication;
constexpr auto kDefaultClientDeauthReason = wlan_ieee80211::ReasonCode::kLeavingNetworkDisassoc;
// Sim firmware returns these values for SNR and RSSI.
const uint8_t kDefaultSimFwSnr = 40;
const int8_t kDefaultSimFwRssi = -20;

class ConnectTest;
class ConnectInterface : public SimInterface {
 public:
  void OnScanResult(OnScanResultRequestView request,
                    OnScanResultCompleter::Sync& completer) override;
  void OnScanEnd(OnScanEndRequestView request, OnScanEndCompleter::Sync& completer) override;
  void ConnectConf(ConnectConfRequestView request, ConnectConfCompleter::Sync& completer) override;
  void DisassocConf(DisassocConfRequestView request,
                    DisassocConfCompleter::Sync& completer) override;
  void DeauthConf(DeauthConfRequestView request, DeauthConfCompleter::Sync& completer) override;
  void DeauthInd(DeauthIndRequestView request, DeauthIndCompleter::Sync& completer) override;
  void DisassocInd(DisassocIndRequestView request, DisassocIndCompleter::Sync& completer) override;
  void SignalReport(SignalReportRequestView request,
                    SignalReportCompleter::Sync& completer) override;

  ConnectTest* test_;
};

class ConnectTest : public SimTest {
 public:
  // How long an individual test will run for. We need an end time because tests run until no more
  // events remain and so we need to stop aps from beaconing to drain the event queue.
  static constexpr zx::duration kTestDuration = zx::sec(100);

  void Init();

  void StartDisassoc();
  void DisassocFromAp();

  // Run through the connect flow
  void StartConnect();
  void StartReconnect();

  // Send bad association responses
  void SendBadResp();

  // Send repeated association responses
  void SendMultipleResp();

  // Send association response with WMM IE
  void SendAssocRespWithWmm();

  // Send one authentication response to help client passing through authentication process
  void SendOpenAuthResp();

  // Send Disassociate request to SIM FW
  void DisassocClient(const MacAddr& mac_addr);

  // Pretend to transmit Disassoc from AP
  void TxFakeDisassocReq();

  // Deauth routines
  void StartDeauth();
  void DeauthClient();
  void DeauthFromAp();

  void ConnectErrorInject();
  void ConnectErrorEventInject(brcmf_fweh_event_status_t ret_status,
                               wlan_ieee80211::StatusCode ret_reason);

  void GetIfaceStats(fuchsia_wlan_stats::wire::IfaceStats* out_stats);
  void GetIfaceHistogramStats(fuchsia_wlan_stats::wire::IfaceHistogramStats* out_stats);
  void GetSignalReport(fuchsia_wlan_stats::wire::SignalReport* out_signal_report);
  void DetailedHistogramErrorInject();

  // Event handlers
  void OnConnectConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcConnectConfRequest* resp);
  void OnDisassocInd(const fuchsia_wlan_fullmac::WlanFullmacImplIfcDisassocIndRequest* ind);
  void OnDisassocConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcDisassocConfRequest* resp);
  void OnDeauthConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcDeauthConfRequest* resp);
  void OnDeauthInd(const fuchsia_wlan_fullmac::WlanFullmacImplIfcDeauthIndRequest* ind);
  void OnSignalReport(const fuchsia_wlan_fullmac::WlanFullmacImplIfcSignalReportRequest* req);

 protected:
  struct ConnectContext {
    // Information about the BSS we are attempting to associate with. Used to generate the
    // appropriate MLME calls (Join => Auth => Assoc).
    simulation::WlanTxInfo tx_info = {
        .primary_channel = kDefaultChannel,
        .bandwidth = wlan_ieee80211::ChannelBandwidth::kCbw20,
        .vht_secondary_80_channel = {.band = kDefaultChannel.band, .number = 0}};
    MacAddr bssid = kDefaultBssid;
    fuchsia_wlan_ieee80211::Ssid ssid = kDefaultSsid;
    std::vector<uint8_t> ies = std::vector<uint8_t>(kIes, kIes + sizeof(kIes));

    // There should be one result for each association response received
    std::list<wlan_ieee80211::StatusCode> expected_results;
    std::vector<uint8_t> expected_wmm_param;

    // An optional function to call when we see the association request go out.
    std::function<void()> on_assoc_req_callback;
    // An optional function to call when we see the authentication request go out.
    std::function<void()> on_auth_req_callback;

    // Track number of connection responses
    size_t connect_resp_count = 0;
    // Track number of disassociation confs (initiated from self)
    size_t disassoc_conf_count = 0;
    // Track number of disassoc indications (initiated from AP)
    size_t disassoc_ind_count = 0;
    // Track number of deauth indications (initiated from AP)
    size_t deauth_ind_count = 0;
    // Number of deauth confirmations (when initiated by self)
    size_t deauth_conf_count = 0;
    // Whether deauth/disassoc is locally initiated
    size_t ind_locally_initiated_count = 0;
    // Number of signal report indications (once client is assoc'd)
    size_t signal_ind_count = 0;
    // SNR seen in the signal report indication.
    int16_t signal_ind_snr = 0;
    // RSSI seen in the signal report indication.
    int16_t signal_ind_rssi = 0;
  };

  struct AssocRespInfo {
    fuchsia_wlan_ieee80211::wire::ChannelNumber primary;
    fuchsia_wlan_ieee80211::wire::ChannelBandwidth bandwidth;
    fuchsia_wlan_ieee80211::wire::ChannelNumber vht_secondary_80_channel;
    MacAddr src;
    MacAddr dst;
    wlan_ieee80211::StatusCode status;
  };

  // This is the interface we will use for our single client interface
  ConnectInterface client_ifc_;

  ConnectContext context_;

  // Keep track of the APs that are in operation so we can easily disable beaconing on all of them
  // at the end of each test.
  std::list<simulation::FakeAp*> aps_;

  // All of the association responses seen in the environment.
  std::list<AssocRespInfo> assoc_responses_;

  // All the reason codes of de-authentication frames seen in the environment.
  std::list<wlan_ieee80211::ReasonCode> deauth_frames_;

  // All the status codes of authentication responses seen in the environment.
  std::list<wlan_ieee80211::StatusCode> auth_resp_status_list_;

  // Trigger to start disassociation. If set to true, disassociation is started
  // soon after association completes.
  bool start_disassoc_ = false;
  // If disassoc_from_ap_ is set to true, the disassociation process is started
  // from the FakeAP else from the station itself.
  bool disassoc_from_ap_ = false;

  // This flag is checked only if disassoc_from_ap_ is false. If set to true, the
  // local client mac is used in disassoc_req else a fake mac address is used
  bool disassoc_self_ = true;

  // Indicates if deauth needs to be issued.
  bool start_deauth_ = false;
  // Indicates if deauth is from the AP or from self
  bool deauth_from_ap_ = false;
  // Indicates reconnect (via assoc) needs to be scheduled when disassoc_ind is received.
  bool start_reconnect_assoc_ = false;
  // Indicates reconnect (via assoc) needs to be issued right when disassoc_ind is received.
  bool start_reconnect_assoc_instant_ = false;

 private:
  // StationIfc overrides
  void Rx(std::shared_ptr<const simulation::SimFrame> frame,
          std::shared_ptr<const simulation::WlanRxInfo> info) override;
};

void ConnectInterface::OnScanResult(OnScanResultRequestView request,
                                    OnScanResultCompleter::Sync& completer) {
  // Ignore and reply.
  completer.Reply();
}
void ConnectInterface::OnScanEnd(OnScanEndRequestView request,
                                 OnScanEndCompleter::Sync& completer) {
  // Ignore and reply.
  completer.Reply();
}
void ConnectInterface::ConnectConf(ConnectConfRequestView request,
                                   ConnectConfCompleter::Sync& completer) {
  const auto connect_conf = fidl::ToNatural(*request);
  test_->OnConnectConf(&connect_conf);
  completer.Reply();
}
void ConnectInterface::DisassocConf(DisassocConfRequestView request,
                                    DisassocConfCompleter::Sync& completer) {
  const auto disassoc_conf = fidl::ToNatural(*request);
  test_->OnDisassocConf(&disassoc_conf);
  completer.Reply();
}
void ConnectInterface::DeauthConf(DeauthConfRequestView request,
                                  DeauthConfCompleter::Sync& completer) {
  const auto deauth_conf = fidl::ToNatural(*request);
  test_->OnDeauthConf(&deauth_conf);
  completer.Reply();
}
void ConnectInterface::DeauthInd(DeauthIndRequestView request,
                                 DeauthIndCompleter::Sync& completer) {
  const auto deauth_ind = fidl::ToNatural(*request);
  test_->OnDeauthInd(&deauth_ind);
  completer.Reply();
}
void ConnectInterface::DisassocInd(DisassocIndRequestView request,
                                   DisassocIndCompleter::Sync& completer) {
  const auto disassoc_ind = fidl::ToNatural(*request);
  test_->OnDisassocInd(&disassoc_ind);
  completer.Reply();
}
void ConnectInterface::SignalReport(SignalReportRequestView request,
                                    SignalReportCompleter::Sync& completer) {
  const auto signal_report = fidl::ToNatural(*request);
  test_->OnSignalReport(&signal_report);
  completer.Reply();
}

void ConnectTest::Rx(std::shared_ptr<const simulation::SimFrame> frame,
                     std::shared_ptr<const simulation::WlanRxInfo> info) {
  ASSERT_EQ(frame->FrameType(), simulation::SimFrame::FRAME_TYPE_MGMT);

  auto mgmt_frame = std::static_pointer_cast<const simulation::SimManagementFrame>(frame);
  // If a handler has been installed, call it
  if (mgmt_frame->MgmtFrameType() == simulation::SimManagementFrame::FRAME_TYPE_ASSOC_REQ) {
    if (context_.on_assoc_req_callback) {
      env_->ScheduleNotification(context_.on_assoc_req_callback, zx::msec(1));
    }
  }

  if (mgmt_frame->MgmtFrameType() == simulation::SimManagementFrame::FRAME_TYPE_ASSOC_RESP) {
    auto assoc_resp = std::static_pointer_cast<const simulation::SimAssocRespFrame>(mgmt_frame);
    AssocRespInfo resp_info = {.primary = info->primary,
                               .bandwidth = info->bandwidth,
                               .vht_secondary_80_channel = info->vht_secondary_80_channel,
                               .src = assoc_resp->src_addr_,
                               .dst = assoc_resp->dst_addr_,
                               .status = assoc_resp->status_};
    assoc_responses_.push_back(resp_info);
  }

  if (mgmt_frame->MgmtFrameType() == simulation::SimManagementFrame::FRAME_TYPE_AUTH) {
    auto auth_frame = std::static_pointer_cast<const simulation::SimAuthFrame>(mgmt_frame);
    // When we receive a authentication request, try to call the callback.
    if (auth_frame->seq_num_ == 1) {
      if (context_.on_auth_req_callback) {
        env_->ScheduleNotification(context_.on_auth_req_callback, zx::msec(1));
      }
      return;
    }

    if (auth_frame->seq_num_ == 2 || auth_frame->seq_num_ == 4)
      auth_resp_status_list_.push_back(auth_frame->status_);
  }

  if (mgmt_frame->MgmtFrameType() == simulation::SimManagementFrame::FRAME_TYPE_DEAUTH) {
    auto deauth_frame = std::static_pointer_cast<const simulation::SimDeauthFrame>(mgmt_frame);
    deauth_frames_.push_back(deauth_frame->reason_);
  }
}

// Create our device instance and hook up the callbacks
void ConnectTest::Init() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);
  client_ifc_.test_ = this;
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
  context_.connect_resp_count = 0;
  context_.disassoc_conf_count = 0;
  context_.deauth_ind_count = 0;
  context_.disassoc_ind_count = 0;
  context_.ind_locally_initiated_count = 0;
  context_.signal_ind_count = 0;
  context_.signal_ind_rssi = 0;
  context_.signal_ind_snr = 0;

  // Reset all of these settings, which should be set in tests that need them.
  start_disassoc_ = false;
  disassoc_from_ap_ = false;
  disassoc_self_ = true;
  start_deauth_ = false;
  deauth_from_ap_ = false;
}

void ConnectTest::DisassocFromAp() {
  MacAddr my_mac;
  client_ifc_.GetMacAddr(&my_mac);

  // Disassoc the STA
  for (auto ap : aps_) {
    ap->DisassocSta(my_mac, kDefaultApDisassocReason);
  }
}

void ConnectTest::OnConnectConf(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcConnectConfRequest* resp) {
  context_.connect_resp_count++;
  ASSERT_TRUE(resp->result_code().has_value());
  EXPECT_EQ(resp->result_code(), context_.expected_results.front());

  if (!context_.expected_wmm_param.empty()) {
    ASSERT_TRUE(resp->association_ies().has_value());
    EXPECT_GT(resp->association_ies().value().size(), 0ul);
    bool contains_wmm_param = false;
    for (size_t offset = 0;
         offset <= resp->association_ies().value().size() - context_.expected_wmm_param.size();
         offset++) {
      if (memcmp(resp->association_ies().value().data() + offset,
                 context_.expected_wmm_param.data(), context_.expected_wmm_param.size()) == 0) {
        contains_wmm_param = true;
        break;
      }
    }
    EXPECT_TRUE(contains_wmm_param);
  }

  context_.expected_results.pop_front();
  context_.expected_wmm_param.clear();

  if (start_disassoc_) {
    env_->ScheduleNotification([this] { StartDisassoc(); }, zx::msec(200));
  } else if (start_deauth_) {
    env_->ScheduleNotification([this] { StartDeauth(); }, zx::msec(200));
  }
}

void ConnectTest::OnDisassocConf(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcDisassocConfRequest* resp) {
  if (resp->status().has_value() && resp->status().value() == ZX_OK) {
    context_.disassoc_conf_count++;
  }
}

void ConnectTest::OnDeauthConf(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcDeauthConfRequest* resp) {
  context_.deauth_conf_count++;
}

void ConnectTest::OnDeauthInd(const fuchsia_wlan_fullmac::WlanFullmacImplIfcDeauthIndRequest* ind) {
  context_.deauth_ind_count++;
  if (ind->locally_initiated().has_value() && ind->locally_initiated().value()) {
    context_.ind_locally_initiated_count++;
  }
  client_ifc_.stats_.deauth_indications.push_back(*ind);
}

void ConnectTest::OnDisassocInd(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcDisassocIndRequest* ind) {
  context_.disassoc_ind_count++;
  if (ind->locally_initiated().has_value() && ind->locally_initiated().value()) {
    context_.ind_locally_initiated_count++;
  }
  client_ifc_.stats_.disassoc_indications.push_back(*ind);
  if (start_reconnect_assoc_instant_) {
    StartReconnect();
  } else if (start_reconnect_assoc_) {
    env_->ScheduleNotification(std::bind(&ConnectTest::StartReconnect, this), zx::sec(3));
  }
}

void ConnectTest::OnSignalReport(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcSignalReportRequest* req) {
  context_.signal_ind_count++;
  context_.signal_ind_rssi = req->ind().rssi_dbm();
  context_.signal_ind_snr = req->ind().snr_db();
}

void ConnectTest::StartConnect() {
  // Send connect request
  auto builder = wlan_fullmac_wire::WlanFullmacImplConnectRequest::Builder(client_ifc_.test_arena_);
  fuchsia_wlan_ieee80211::wire::BssDescription bss;

  std::memcpy(bss.bssid.data(), context_.bssid.byte, ETH_ALEN);
  bss.ies = fidl::VectorView<uint8_t>(client_ifc_.test_arena_, context_.ies);
  bss.primary = context_.tx_info.primary_channel;
  builder.selected_bss(bss);
  builder.auth_type(wlan_fullmac_wire::WlanAuthType::kOpenSystem);
  builder.connect_failure_timeout(1000);  // ~1s (although value is ignored for now)
  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->Connect(builder.Build());
  EXPECT_TRUE(result.ok());
}

void ConnectTest::StartReconnect() {
  // Send reconnect request
  // This is what SME does on a disassoc ind.
  auto builder =
      wlan_fullmac_wire::WlanFullmacImplReconnectRequest::Builder(client_ifc_.test_arena_);
  ::fidl::Array<uint8_t, 6> peer_sta_address;
  std::memcpy(peer_sta_address.data(), context_.bssid.byte, ETH_ALEN);
  builder.peer_sta_address(peer_sta_address);
  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->Reconnect(builder.Build());
  EXPECT_TRUE(result.ok());
}

// Verify that we get a signal report when associated.
TEST_F(ConnectTest, SignalReportTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  EXPECT_EQ((int64_t)context_.signal_ind_count,
            kTestDuration.get() / BRCMF_SIGNAL_REPORT_TIMER_DUR_MS);
  // Verify the plumbing between the firmware and the signal report.
  EXPECT_EQ(context_.signal_ind_snr, kDefaultSimFwSnr);
  EXPECT_EQ(context_.signal_ind_rssi, kDefaultSimFwRssi);
}

void ConnectTest::GetIfaceStats(fuchsia_wlan_stats::wire::IfaceStats* out_stats) {
  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->GetIfaceStats();
  EXPECT_TRUE(result.ok());
  if (!result->is_error()) {
    *out_stats = result->value()->stats;
  }
}

void ConnectTest::GetIfaceHistogramStats(fuchsia_wlan_stats::wire::IfaceHistogramStats* out_stats) {
  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->GetIfaceHistogramStats();
  EXPECT_TRUE(result.ok());
  // Copy the pointers out, the data still exist in client_ifc_.test_arena_.
  if (!result->is_error()) {
    *out_stats = result->value()->stats;
  }
}

void ConnectTest::GetSignalReport(fuchsia_wlan_stats::wire::SignalReport* out_signal_report) {
  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->GetSignalReport();
  EXPECT_TRUE(result.ok());
  if (!result->is_error()) {
    *out_signal_report = *result->value();
  }
}

TEST_F(ConnectTest, GetSignalReportTest_NotConnected) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));
  ap.SetAssocHandling(simulation::FakeAp::ASSOC_REFUSED);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);
  fuchsia_wlan_stats::wire::SignalReport signal_report = {};

  env_->ScheduleNotification(std::bind(&ConnectTest::GetSignalReport, this, &signal_report),
                             zx::msec(5));
  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  ASSERT_FALSE(signal_report.has_connection_signal_report());
}

TEST_F(ConnectTest, GetSignalReportTest_Connected) {
  Init();

  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  fuchsia_wlan_stats::wire::SignalReport signal_report = {};

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::GetSignalReport, this, &signal_report),
                             zx::msec(30));

  env_->Run(kTestDuration);

  ASSERT_TRUE(signal_report.has_connection_signal_report());
  auto conn_report = signal_report.connection_signal_report();
  EXPECT_EQ(conn_report.rssi_dbm(), kDefaultSimFwRssi);
  EXPECT_EQ(conn_report.snr_db(), kDefaultSimFwSnr);
  EXPECT_EQ(conn_report.primary().number, kDefaultChannel.number);
}

// This test is to verify that GetIfaceStats still returns a response even when the
// client is not connected, just that `connection_stats` is not set.
TEST_F(ConnectTest, GetIfaceStatsTest_NotConnected) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));
  ap.SetAssocHandling(simulation::FakeAp::ASSOC_REFUSED);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);
  fuchsia_wlan_stats::wire::IfaceStats stats_1 = {};
  fuchsia_wlan_stats::wire::IfaceStats stats_2 = {};

  env_->ScheduleNotification(std::bind(&ConnectTest::GetIfaceStats, this, &stats_1), zx::msec(5));
  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::GetIfaceStats, this, &stats_2), zx::msec(30));

  env_->Run(kTestDuration);

  ASSERT_FALSE(stats_1.has_connection_stats());
  ASSERT_FALSE(stats_2.has_connection_stats());
}

TEST_F(ConnectTest, GetIfaceStatsTest_Connected) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  fuchsia_wlan_stats::wire::IfaceStats stats = {};

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::GetIfaceStats, this, &stats), zx::msec(30));

  env_->Run(kTestDuration);

  // Sim firmware returns these fake values for packet counters.
  const uint64_t fw_rx_good = 5;
  const uint64_t fw_rx_bad = 4;
  const uint64_t fw_rx_multicast = 1;
  const uint64_t fw_tx_good = 3;
  const uint64_t fw_tx_bad = 2;
  ASSERT_TRUE(stats.has_connection_stats());
  auto connection_stats = stats.connection_stats();
  ASSERT_TRUE(connection_stats.has_connection_id());
  EXPECT_EQ(connection_stats.connection_id(), 1);
  ASSERT_TRUE(connection_stats.has_rx_unicast_total());
  EXPECT_EQ(connection_stats.rx_unicast_total(), fw_rx_good + fw_rx_bad);
  ASSERT_TRUE(connection_stats.has_rx_unicast_drop());
  EXPECT_EQ(connection_stats.rx_unicast_drop(), fw_rx_bad);
  ASSERT_TRUE(connection_stats.has_rx_multicast());
  EXPECT_EQ(connection_stats.rx_multicast(), fw_rx_multicast);
  ASSERT_TRUE(connection_stats.has_tx_total());
  EXPECT_EQ(connection_stats.tx_total(), fw_tx_good + fw_tx_bad);
  ASSERT_TRUE(connection_stats.has_tx_drop());
  EXPECT_EQ(connection_stats.tx_drop(), fw_tx_bad);
}

// Test that on a new connection, the connection ID returned by GetIfaceStats is updated.
TEST_F(ConnectTest, GetIfaceStatsTest_Reconnected) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  fuchsia_wlan_stats::wire::IfaceStats stats_1 = {};
  fuchsia_wlan_stats::wire::IfaceStats stats_2 = {};

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::GetIfaceStats, this, &stats_1), zx::msec(20));
  env_->ScheduleNotification(std::bind(&ConnectTest::DisassocFromAp, this), zx::sec(2));
  env_->ScheduleNotification(std::bind(&ConnectTest::StartReconnect, this), zx::sec(3));
  env_->ScheduleNotification(std::bind(&ConnectTest::GetIfaceStats, this, &stats_2), zx::sec(4));

  env_->Run(kTestDuration);

  ASSERT_TRUE(stats_1.has_connection_stats());
  auto connection_stats_1 = stats_1.connection_stats();
  ASSERT_TRUE(connection_stats_1.has_connection_id());
  EXPECT_EQ(connection_stats_1.connection_id(), 1);

  ASSERT_TRUE(stats_2.has_connection_stats());
  auto connection_stats_2 = stats_2.connection_stats();
  ASSERT_TRUE(connection_stats_2.has_connection_id());
  EXPECT_EQ(connection_stats_2.connection_id(), 2);
}

TEST_F(ConnectTest, GetIfaceHistogramStatsTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  fuchsia_wlan_stats::wire::IfaceHistogramStats stats;

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::GetIfaceHistogramStats, this, &stats),
                             zx::msec(30));

  env_->Run(kTestDuration);

  // Sim firmware returns these fake values for per-antenna histograms.
  const auto& expected_hist_scope = fuchsia_wlan_stats::wire::HistScope::kPerAntenna;
  const auto& expected_antenna_freq = fuchsia_wlan_stats::wire::AntennaFreq::kAntenna2G;
  const uint8_t expected_antenna_index = 0;
  const uint8_t expected_snr_index = 60;
  const uint8_t expected_snr_num_frames = 50;
  // TODO(https://fxbug.dev/42104477): Test all bucket values when sim firmware fully supports
  // wstats_counters. Sim firmware populates only SNR buckets, probably due to the discrepancies
  // between the iovar get handling between real and sim firmware (e.g. fxr/404141). When
  // wstats_counters is fully supported in sim firmware we can test for the expected noise floor,
  // RSSI, and rate buckets.

  ASSERT_EQ(stats.noise_floor_histograms().size(), 1U);
  EXPECT_EQ(stats.noise_floor_histograms().data()[0].hist_scope, expected_hist_scope);
  EXPECT_EQ(stats.noise_floor_histograms().data()[0].antenna_id->freq, expected_antenna_freq);
  EXPECT_EQ(stats.noise_floor_histograms().data()[0].antenna_id->index, expected_antenna_index);

  ASSERT_EQ(stats.rssi_histograms().size(), 1U);
  EXPECT_EQ(stats.rssi_histograms().data()[0].hist_scope, expected_hist_scope);
  EXPECT_EQ(stats.rssi_histograms().data()[0].antenna_id->freq, expected_antenna_freq);
  EXPECT_EQ(stats.rssi_histograms().data()[0].antenna_id->index, expected_antenna_index);

  ASSERT_EQ(stats.rx_rate_index_histograms().size(), 1U);
  EXPECT_EQ(stats.rx_rate_index_histograms().data()[0].hist_scope, expected_hist_scope);
  EXPECT_EQ(stats.rx_rate_index_histograms().data()[0].antenna_id->freq, expected_antenna_freq);
  EXPECT_EQ(stats.rx_rate_index_histograms().data()[0].antenna_id->index, expected_antenna_index);

  ASSERT_EQ(stats.snr_histograms().size(), 1U);
  EXPECT_EQ(stats.snr_histograms().data()[0].hist_scope, expected_hist_scope);
  EXPECT_EQ(stats.snr_histograms().data()[0].antenna_id->freq, expected_antenna_freq);
  EXPECT_EQ(stats.snr_histograms().data()[0].antenna_id->index, expected_antenna_index);
  uint64_t snr_samples_count = 0;
  uint64_t empty_snr_samples_count = 0;
  uint64_t snr_bucket_index = 0;
  uint64_t snr_bucket_num_samples = 0;
  for (uint64_t i = 0; i < stats.snr_histograms().data()[0].snr_samples.size(); ++i) {
    if (stats.snr_histograms().data()[0].snr_samples.data()[i].num_samples > 0) {
      ++snr_samples_count;
      snr_bucket_index = stats.snr_histograms().data()[0].snr_samples.data()[i].bucket_index;
      snr_bucket_num_samples = stats.snr_histograms().data()[0].snr_samples.data()[i].num_samples;
    } else {
      ++empty_snr_samples_count;
    }
  }

  EXPECT_EQ(snr_samples_count, 1U);
  EXPECT_EQ(snr_bucket_index, expected_snr_index);
  EXPECT_EQ(snr_bucket_num_samples, expected_snr_num_frames);
  EXPECT_EQ(empty_snr_samples_count, 0U);
}

TEST_F(ConnectTest, GetIfaceHistogramStatsNotSupportedTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  fuchsia_wlan_stats::wire::IfaceHistogramStats stats = {};

  DetailedHistogramErrorInject();
  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::GetIfaceHistogramStats, this, &stats),
                             zx::msec(30));

  env_->Run(kTestDuration);

  EXPECT_FALSE(stats.has_noise_floor_histograms());
  EXPECT_FALSE(stats.has_rssi_histograms());
  EXPECT_FALSE(stats.has_rx_rate_index_histograms());
  EXPECT_FALSE(stats.has_snr_histograms());
}

void ConnectTest::ConnectErrorInject() {
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjCmd(BRCMF_C_SET_SSID, ZX_OK, BCME_OK, client_ifc_.iface_id_);
  });
}

void ConnectTest::ConnectErrorEventInject(brcmf_fweh_event_status_t ret_status,
                                          wlan_ieee80211::StatusCode ret_reason) {
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrEventInjCmd(BRCMF_C_SET_SSID, BRCMF_E_ASSOC, ret_status, ret_reason,
                                            0, client_ifc_.iface_id_);
  });
}

void ConnectTest::StartDisassoc() {
  // Send disassoc request
  if (!disassoc_from_ap_) {
    if (disassoc_self_) {
      DisassocClient(context_.bssid);
    } else {
      DisassocClient(kMadeupClient);
    }
  } else {
    DisassocFromAp();
  }
}

void ConnectTest::StartDeauth() {
  // Send deauth request
  if (!deauth_from_ap_) {
    DeauthClient();
  } else {
    // Send deauth frame
    DeauthFromAp();
  }
}

void ConnectTest::DisassocClient(const MacAddr& mac_addr) {
  auto builder =
      fuchsia_wlan_fullmac::wire::WlanFullmacImplDisassocRequest::Builder(client_ifc_.test_arena_);

  ::fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
  std::memcpy(peer_sta_address.data(), mac_addr.byte, ETH_ALEN);
  builder.peer_sta_address(peer_sta_address);
  builder.reason_code(kDefaultClientDisassocReason);

  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->Disassoc(builder.Build());
  EXPECT_TRUE(result.ok());
}

void ConnectTest::DeauthClient() {
  auto builder =
      fuchsia_wlan_fullmac::wire::WlanFullmacImplDeauthRequest::Builder(client_ifc_.test_arena_);

  ::fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
  std::memcpy(peer_sta_address.data(), context_.bssid.byte, ETH_ALEN);
  builder.peer_sta_address(peer_sta_address);
  builder.reason_code(kDefaultClientDeauthReason);

  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->Deauth(builder.Build());
  EXPECT_TRUE(result.ok());
}

void ConnectTest::DeauthFromAp() {
  // Figure out our own MAC
  MacAddr my_mac;
  client_ifc_.GetMacAddr(&my_mac);

  // Send a Deauth to our STA
  simulation::SimDeauthFrame deauth_frame(context_.bssid, my_mac, kDefaultApDeauthReason);
  env_->Tx(deauth_frame, context_.tx_info, this);
}

void ConnectTest::TxFakeDisassocReq() {
  // Figure out our own MAC
  MacAddr my_mac;
  client_ifc_.GetMacAddr(&my_mac);

  // Send a Disassoc Req to our STA (which is not associated)
  simulation::SimDisassocReqFrame not_associated_frame(context_.bssid, my_mac,
                                                       kDefaultApDisassocReason);
  env_->Tx(not_associated_frame, context_.tx_info, this);

  // Send a Disassoc Req from the wrong bss
  MacAddr wrong_src(context_.bssid);
  wrong_src.byte[ETH_ALEN - 1]++;
  simulation::SimDisassocReqFrame wrong_bss_frame(wrong_src, my_mac, kDefaultApDisassocReason);
  env_->Tx(wrong_bss_frame, context_.tx_info, this);

  // Send a Disassoc Req to a different STA
  MacAddr wrong_dst(my_mac);
  wrong_dst.byte[ETH_ALEN - 1]++;
  simulation::SimDisassocReqFrame wrong_sta_frame(context_.bssid, wrong_dst,
                                                  kDefaultApDisassocReason);
  env_->Tx(wrong_sta_frame, context_.tx_info, this);
}

void ConnectTest::DetailedHistogramErrorInject() {
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjIovar("wstats_counters", ZX_ERR_NOT_SUPPORTED, BCME_OK,
                                         client_ifc_.iface_id_);
  });
}

// For this test, we want the pre-assoc scan test to fail because no APs are found.
TEST_F(ConnectTest, NoAps) {
  // Create our device instance
  Init();

  const MacAddr kBssid({0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc});
  context_.bssid = kBssid;
  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRejectedSequenceTimeout);
  context_.ssid = {'T', 'e', 's', 't', 'A', 'P'};
  context_.tx_info.primary_channel = {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz,
                                      .number = 9};
  context_.tx_info.bandwidth = wlan_ieee80211::ChannelBandwidth::kCbw20;
  context_.tx_info.vht_secondary_80_channel = {.band = context_.tx_info.primary_channel.band,
                                               .number = 0};

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
}

// Verify that we can successfully associate to a fake AP
TEST_F(ConnectTest, SimpleTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
  EXPECT_EQ((int64_t)context_.signal_ind_count,
            kTestDuration.get() / BRCMF_SIGNAL_REPORT_TIMER_DUR_MS);
}

// Verify that we can successfully associate to a fake AP using the new connect API.
TEST_F(ConnectTest, SimpleConnectTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification([this] { StartConnect(); }, zx::msec(10));

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
  EXPECT_EQ((int64_t)context_.signal_ind_count,
            kTestDuration.get() / BRCMF_SIGNAL_REPORT_TIMER_DUR_MS);
}

// Verify that we can associate using only SSID, not BSSID
TEST_F(ConnectTest, SsidTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
}

// Verify that APs with incorrect SSIDs or BSSIDs are ignored
TEST_F(ConnectTest, WrongIds) {
  // Create our device instance
  Init();

  constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kWrongChannel = {
      .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 8};
  ASSERT_NE(kDefaultChannel.number, kWrongChannel.number);
  const fuchsia_wlan_ieee80211::Ssid kWrongSsid = {'F', 'u', 'c', 'h', 's', 'i', 'a',
                                                   ' ', 'F', 'a', 'k', 'e', ' ', 'A'};
  ASSERT_NE(kDefaultSsid.size(), kWrongSsid.size());
  const MacAddr kWrongBssid({0x12, 0x34, 0x56, 0x78, 0x9b, 0xbc});
  ASSERT_NE(kDefaultBssid, kWrongBssid);

  // Start up fake APs
  simulation::FakeAp ap1(env_.get(), kDefaultBssid, kDefaultSsid, kWrongChannel,
                         fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap1);
  simulation::FakeAp ap2(env_.get(), kWrongBssid, kDefaultSsid, kDefaultChannel,
                         fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap2);
  simulation::FakeAp ap3(env_.get(), kDefaultBssid, kWrongSsid, kDefaultChannel,
                         fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap3);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  // The APs aren't giving us a response, but the driver is telling us that the operation failed
  // because it couldn't find a matching AP.
  EXPECT_EQ(context_.connect_resp_count, 1U);
}

// Attempt to associate while already associated
TEST_F(ConnectTest, RepeatedConnectTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  // The associations at 11ms and 12ms should be immediately refused (because there is already an
  // association in progress), and eventually the association that was in progress should succeed.
  context_.expected_results.push_back(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);
  context_.expected_results.push_back(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);
  context_.expected_results.push_back(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(11));
  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(12));

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 3U);
}

// Verify that if an AP does not respond to an association response we return a failure
TEST_F(ConnectTest, ApIgnoredRequest) {
  // Create our device instance
  Init();

  // Start up fake APs
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.SetAssocHandling(simulation::FakeAp::ASSOC_IGNORED);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRejectedSequenceTimeout);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  // Make sure no responses were sent back from the fake AP
  EXPECT_EQ(assoc_responses_.size(), 0U);

  // But we still got our response from the driver
  EXPECT_EQ(context_.connect_resp_count, 1U);
}

// Verify that if an AP refuses an association request we return a temporary refusal.  The driver
// will also send a BRCMF_C_SCB_DEAUTHENTICATE_FOR_REASON to clear AP state each time when refused.
TEST_F(ConnectTest, ApTemporarilyRefusedRequest) {
  // Create our device instance
  Init();

  // Start up fake APs
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.SetAssocHandling(simulation::FakeAp::ASSOC_REFUSED_TEMPORARILY);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRefusedTemporarily);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  uint32_t max_assoc_retries;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    struct brcmf_if* ifp = brcmf_get_ifp(sim->drvr, client_ifc_.iface_id_);
    zx_status_t status =
        brcmf_fil_iovar_int_get(ifp, "assoc_retry_max", &max_assoc_retries, nullptr);
    EXPECT_EQ(status, ZX_OK);
    ASSERT_EQ(max_assoc_retries, kMaxAssocRetries);
  });

  // We should have gotten a refusal from the fake AP
  EXPECT_EQ(assoc_responses_.size(), max_assoc_retries + 1);
  EXPECT_EQ(assoc_responses_.front().status, wlan_ieee80211::StatusCode::kRefusedTemporarily);

  // Make sure we got our response from the driver
  EXPECT_EQ(context_.connect_resp_count, 1U);
}

// Verify that if an AP refuses an association request we return a failure.  The driver will also
// send a BRCMF_C_SCB_DEAUTHENTICATE_FOR_REASON to clear AP state each time when refused.
TEST_F(ConnectTest, ApRefusedRequest) {
  // Create our device instance
  Init();

  // Start up fake APs
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.SetAssocHandling(simulation::FakeAp::ASSOC_REFUSED);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  uint32_t max_assoc_retries;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    struct brcmf_if* ifp = brcmf_get_ifp(sim->drvr, client_ifc_.iface_id_);
    zx_status_t status =
        brcmf_fil_iovar_int_get(ifp, "assoc_retry_max", &max_assoc_retries, nullptr);
    EXPECT_EQ(status, ZX_OK);
    ASSERT_EQ(max_assoc_retries, kMaxAssocRetries);
  });

  // We should have gotten a refusal from the fake AP.
  EXPECT_EQ(assoc_responses_.size(), max_assoc_retries + 1);
  EXPECT_EQ(assoc_responses_.front().status, wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);

  // The AP should have received 1 deauth, no matter there were how many firmware assoc retries.
  EXPECT_EQ(deauth_frames_.size(), 1U);
  EXPECT_EQ(deauth_frames_.front(), wlan_ieee80211::ReasonCode::kStaLeaving);
  // Make sure we got our response from the driver
  EXPECT_EQ(context_.connect_resp_count, 1U);
}

// SIM FW ignore client assoc request. Note that currently there is no timeout
// mechanism in the driver to handle this situation. It is currently being
// worked on.
TEST_F(ConnectTest, SimFwIgnoreConnectReq) {
  // Create our device instance
  Init();

  // Start up fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  context_.expected_results.push_back(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);

  ConnectErrorInject();
  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(50));
  env_->Run(kTestDuration);

  // We should not have received a assoc response from SIM FW
  EXPECT_EQ(context_.connect_resp_count, 1U);
}

void ConnectTest::SendBadResp() {
  // Figure out our own MAC
  MacAddr my_mac;
  client_ifc_.GetMacAddr(&my_mac);

  // Send a response from the wrong bss
  MacAddr wrong_src(context_.bssid);
  wrong_src.byte[ETH_ALEN - 1]++;
  simulation::SimAssocRespFrame wrong_bss_frame(wrong_src, my_mac,
                                                wlan_ieee80211::StatusCode::kSuccess);
  env_->Tx(wrong_bss_frame, context_.tx_info, this);

  // Send a response to a different STA
  MacAddr wrong_dst(my_mac);
  wrong_dst.byte[ETH_ALEN - 1]++;
  simulation::SimAssocRespFrame wrong_dst_frame(context_.bssid, wrong_dst,
                                                wlan_ieee80211::StatusCode::kSuccess);
  env_->Tx(wrong_dst_frame, context_.tx_info, this);
}

// Verify that any non-applicable association responses (i.e., sent to or from the wrong MAC)
// are ignored
TEST_F(ConnectTest, IgnoreRespMismatch) {
  // Create our device instance
  Init();

  // Start up fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  // We want the association request to be ignored so we can inject responses and verify that
  // they are being ignored.
  ap.SetAssocHandling(simulation::FakeAp::ASSOC_IGNORED);

  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRejectedSequenceTimeout);
  context_.on_assoc_req_callback = std::bind(&ConnectTest::SendBadResp, this);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  // Make sure that the firmware/driver ignored bad responses and sent back its own (failure)
  EXPECT_EQ(context_.connect_resp_count, 1U);
}

void ConnectTest::SendMultipleResp() {
  constexpr unsigned kRespCount = 100;

  // Figure out our own MAC
  MacAddr my_mac;
  client_ifc_.GetMacAddr(&my_mac);
  simulation::SimAssocRespFrame multiple_resp_frame(context_.bssid, my_mac,
                                                    wlan_ieee80211::StatusCode::kSuccess);
  for (unsigned i = 0; i < kRespCount; i++) {
    env_->Tx(multiple_resp_frame, context_.tx_info, this);
  }
}

void ConnectTest::SendAssocRespWithWmm() {
  uint8_t mac_buf[ETH_ALEN];
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    struct brcmf_if* ifp = brcmf_get_ifp(sim->drvr, client_ifc_.iface_id_);
    zx_status_t status = brcmf_fil_iovar_data_get(ifp, "cur_etheraddr", mac_buf, ETH_ALEN, nullptr);
    EXPECT_EQ(status, ZX_OK);
  });
  MacAddr my_mac(mac_buf);
  simulation::SimAssocRespFrame assoc_resp_frame(context_.bssid, my_mac,
                                                 wlan_ieee80211::StatusCode::kSuccess);

  uint8_t raw_ies[] = {
      // WMM param
      0xdd, 0x18, 0x00, 0x50, 0xf2, 0x02, 0x01, 0x01,  // WMM header
      0x80,                                            // Qos Info - U-ASPD enabled
      0x00,                                            // reserved
      0x03, 0xa4, 0x00, 0x00,                          // Best effort AC params
      0x27, 0xa4, 0x00, 0x00,                          // Background AC params
      0x42, 0x43, 0x5e, 0x00,                          // Video AC params
      0x62, 0x32, 0x2f, 0x00,                          // Voice AC params
  };
  assoc_resp_frame.AddRawIes(cpp20::span(raw_ies, sizeof(raw_ies)));

  env_->Tx(assoc_resp_frame, context_.tx_info, this);
}

void ConnectTest::SendOpenAuthResp() {
  MacAddr my_mac;
  client_ifc_.GetMacAddr(&my_mac);
  simulation::SimAuthFrame auth_resp(context_.bssid, my_mac, 2, simulation::AUTH_TYPE_OPEN,
                                     wlan_ieee80211::StatusCode::kSuccess);
  env_->Tx(auth_resp, context_.tx_info, this);
}

// Verify that responses after association are ignored
TEST_F(ConnectTest, IgnoreExtraResp) {
  // Create our device instance
  Init();

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  context_.on_assoc_req_callback = std::bind(&ConnectTest::SendMultipleResp, this);
  context_.on_auth_req_callback = std::bind(&ConnectTest::SendOpenAuthResp, this);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  // Make sure that the firmware/driver only responded to the first response
  EXPECT_EQ(context_.connect_resp_count, 1U);
}

// Attempt to associate while a scan is in-progress
TEST_F(ConnectTest, AssocWhileScanning) {
  // Create our device instance
  Init();

  // Start up fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  context_.on_assoc_req_callback = std::bind(&ConnectTest::SendMultipleResp, this);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  const fuchsia_wlan_ieee80211::wire::ChannelNumber channels_list[] = {
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 1},
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 2},
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 3},
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 4},
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 5},
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 6},
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 7},
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 8},
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 9},
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 10},
      {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 11},
  };
  auto builder =
      wlan_fullmac_wire::WlanFullmacImplStartScanRequest::Builder(client_ifc_.test_arena_);

  builder.txn_id(42);
  builder.scan_type(wlan_fullmac_wire::WlanScanType::kPassive);
  auto channels = std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber>(
      channels_list, channels_list + std::size(channels_list));
  builder.channels(fidl::VectorView<fuchsia_wlan_ieee80211::wire::ChannelNumber>(
      client_ifc_.test_arena_, channels));
  builder.min_channel_time(0);
  builder.max_channel_time(100);

  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->StartScan(builder.Build());
  EXPECT_TRUE(result.ok());

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
}

TEST_F(ConnectTest, AssocWithWmm) {
  // Create our device instance
  Init();

  uint8_t expected_wmm_param[] = {0x80, 0x00, 0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4, 0x00,
                                  0x00, 0x42, 0x43, 0x5e, 0x00, 0x62, 0x32, 0x2f, 0x00};
  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  context_.expected_wmm_param.insert(context_.expected_wmm_param.end(), expected_wmm_param,
                                     expected_wmm_param + sizeof(expected_wmm_param));
  context_.on_assoc_req_callback = std::bind(&ConnectTest::SendAssocRespWithWmm, this);
  context_.on_auth_req_callback = std::bind(&ConnectTest::SendOpenAuthResp, this);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
}

TEST_F(ConnectTest, AssocStatusAndReasonCodeMismatchHandling) {
  // Create our device instance
  Init();

  ConnectErrorEventInject(BRCMF_E_STATUS_NO_ACK, wlan_ieee80211::StatusCode::kSuccess);
  context_.expected_results.push_back(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(50));
  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
}

// Verify that we can successfully associate to a fake AP & disassociate
TEST_F(ConnectTest, DisassocFromSelfTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  start_disassoc_ = true;

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
  EXPECT_EQ(context_.disassoc_conf_count, 1U);
}

// Verify that disassoc from fake AP fails when not associated. Also check
// disassoc meant for a different STA, different BSS or when not associated
// is not accepted by the current STA.
TEST_F(ConnectTest, DisassocWithoutConnectTest) {
  // Create our device instance
  Init();
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  // Attempt to disassociate. In this case client is not associated. AP
  // will not transmit the disassoc request
  env_->ScheduleNotification(std::bind(&ConnectTest::StartDisassoc, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::TxFakeDisassocReq, this), zx::msec(50));

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 0U);
  EXPECT_EQ(context_.disassoc_conf_count, 0U);
}

// Verify that disassociate for a different client is ignored
TEST_F(ConnectTest, DisassocNotSelfTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  start_disassoc_ = true;
  disassoc_self_ = false;

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
  EXPECT_EQ(context_.disassoc_conf_count, 0U);
}

// After association, send disassoc from the AP
TEST_F(ConnectTest, DisassocFromAPTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  disassoc_from_ap_ = true;
  start_disassoc_ = true;

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
  EXPECT_EQ(context_.disassoc_ind_count, 1U);
  EXPECT_EQ(context_.ind_locally_initiated_count, 0U);

  ASSERT_EQ(client_ifc_.stats_.disassoc_indications.size(), 1U);
  const auto& disassoc_ind = client_ifc_.stats_.disassoc_indications.front();
  ASSERT_TRUE(disassoc_ind.locally_initiated().has_value());
  EXPECT_FALSE(disassoc_ind.locally_initiated().value());
}

// After assoc & disassoc, send disassoc again to test event handling
TEST_F(ConnectTest, LinkEventTest) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  disassoc_from_ap_ = true;
  start_disassoc_ = true;

  env_->Run(kTestDuration);

  // Send Deauth frame after disassociation
  DeauthFromAp();
  EXPECT_EQ(context_.connect_resp_count, 1U);
  EXPECT_EQ(context_.disassoc_ind_count, 1U);
  EXPECT_EQ(context_.ind_locally_initiated_count, 0U);
}

// After assoc, send a deauth from ap - client should disassociate
TEST_F(ConnectTest, deauth_from_ap) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  deauth_from_ap_ = true;
  start_deauth_ = true;

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
  EXPECT_EQ(context_.deauth_conf_count, 0U);
  EXPECT_EQ(context_.deauth_ind_count, 1U);
  EXPECT_EQ(context_.disassoc_conf_count, 0U);
  EXPECT_EQ(context_.disassoc_ind_count, 0U);
  EXPECT_EQ(context_.ind_locally_initiated_count, 0U);
}

// After assoc, send a deauth from client - client should disassociate
TEST_F(ConnectTest, deauth_from_self) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  deauth_from_ap_ = false;
  start_deauth_ = true;

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 1U);
  EXPECT_EQ(context_.deauth_conf_count, 1U);
  EXPECT_EQ(context_.deauth_ind_count, 0U);
  EXPECT_EQ(context_.disassoc_conf_count, 0U);
  EXPECT_EQ(context_.disassoc_ind_count, 0U);
  EXPECT_EQ(context_.ind_locally_initiated_count, 0U);
}

// Associate, send a deauth from client, associate again, then send deauth from AP.
TEST_F(ConnectTest, deauth_from_self_then_from_ap) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.EnableBeacon(zx::msec(100));
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::DeauthClient, this), zx::sec(1));
  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::sec(2));
  env_->ScheduleNotification(std::bind(&ConnectTest::DeauthFromAp, this), zx::sec(3));

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 2U);
  EXPECT_EQ(context_.deauth_conf_count, 1U);
  EXPECT_EQ(context_.deauth_ind_count, 1U);
  EXPECT_EQ(context_.disassoc_conf_count, 0U);
  EXPECT_EQ(context_.disassoc_ind_count, 0U);
  EXPECT_EQ(context_.ind_locally_initiated_count, 0U);
}

TEST_F(ConnectTest, simple_reconnect_via_assoc) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::DisassocFromAp, this), zx::sec(2));
  env_->ScheduleNotification(std::bind(&ConnectTest::StartReconnect, this), zx::sec(3));

  env_->Run(kTestDuration);

  EXPECT_EQ(context_.connect_resp_count, 2U);
  EXPECT_EQ(context_.disassoc_ind_count, 1U);
  EXPECT_EQ(context_.ind_locally_initiated_count, 0U);
}

// Reconnect via assoc should succeed since the attempt is made after disassoc completes.
TEST_F(ConnectTest, reconnect_via_assoc_success) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  // Schedule reconnect via assoc when disassoc notification is received at SME.
  start_reconnect_assoc_ = true;

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::DisassocFromAp, this), zx::sec(2));

  env_->Run(kTestDuration);

  // Since reconnect via assoc occurs after disassoc is completed it succeeds.
  EXPECT_EQ(context_.connect_resp_count, 2U);
  EXPECT_EQ(context_.disassoc_ind_count, 1U);
  EXPECT_EQ(context_.ind_locally_initiated_count, 0U);
}

// Reconnect via assoc should fail since the attempt is made soon after SME is notified but before
// disassoc completes.
TEST_F(ConnectTest, reconnect_assoc_fails) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  // Issue reconnect assoc soon after disassoc notification is received at SME.
  start_reconnect_assoc_instant_ = true;

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::DisassocFromAp, this), zx::sec(2));

  env_->Run(kTestDuration);

  // Although we attempted to reconnect assoc, it fails because disconnect is in progress
  EXPECT_EQ(context_.connect_resp_count, 1U);
  EXPECT_EQ(context_.disassoc_ind_count, 1U);
  EXPECT_EQ(context_.ind_locally_initiated_count, 0U);
}

TEST_F(ConnectTest, deauth_during_reconnect_via_assoc) {
  // Create our device instance
  Init();

  // Start up our fake AP
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);
  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kSuccess);

  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&ConnectTest::DisassocFromAp, this), zx::sec(2));
  env_->ScheduleNotification(std::bind(&ConnectTest::StartReconnect, this), zx::sec(3));
  // Schedule a deauth immediately, before the above assoc can complete.
  env_->ScheduleNotification(std::bind(&ConnectTest::DeauthClient, this),
                             zx::sec(3) + zx::usec(500));

  env_->Run(kTestDuration);

  // If the deauth is successful, we will not get the second assoc response.
  // If it fails for some reason (e.g. profile->bssid mismatch), then the
  // assoc response count will be 2.
  EXPECT_EQ(context_.connect_resp_count, 1U);
  EXPECT_EQ(context_.disassoc_ind_count, 1U);
  EXPECT_EQ(context_.ind_locally_initiated_count, 0U);
  EXPECT_EQ(context_.deauth_conf_count, 1U);
}

// Verify that association is retried as per the setting
TEST_F(ConnectTest, AssocMaxRetries) {
  // Create our device instance
  Init();

  // Start up fake APs
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.SetAssocHandling(simulation::FakeAp::ASSOC_REFUSED);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);

  zx_status_t status;
  uint32_t max_assoc_retries = 5;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    struct brcmf_if* ifp = brcmf_get_ifp(sim->drvr, client_ifc_.iface_id_);
    status = brcmf_fil_iovar_int_set(ifp, "assoc_retry_max", max_assoc_retries, nullptr);
    EXPECT_EQ(status, ZX_OK);
  });
  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  uint32_t assoc_retries;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    struct brcmf_if* ifp = brcmf_get_ifp(sim->drvr, client_ifc_.iface_id_);
    status = brcmf_fil_iovar_int_get(ifp, "assoc_retry_max", &assoc_retries, nullptr);
    EXPECT_EQ(status, ZX_OK);
    ASSERT_EQ(max_assoc_retries, assoc_retries);
  });
  // Should have received as many refusals as the configured # of retries.
  EXPECT_EQ(assoc_responses_.size(), max_assoc_retries + 1);
  EXPECT_EQ(assoc_responses_.front().status, wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);

  // The AP should have received 1 deauth, no matter there were how many firmware assoc retries.
  EXPECT_EQ(deauth_frames_.size(), 1U);
  EXPECT_EQ(deauth_frames_.front(), wlan_ieee80211::ReasonCode::kStaLeaving);
  // Make sure we got our response from the driver
  EXPECT_EQ(context_.connect_resp_count, 1U);
}

// Verify that association is retried as per the setting
TEST_F(ConnectTest, AssocMaxRetriesWhenTimedOut) {
  // Create our device instance
  Init();

  // Start up fake APs
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.SetAssocHandling(simulation::FakeAp::ASSOC_IGNORED);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);

  uint32_t max_assoc_retries = 5;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    struct brcmf_if* ifp = brcmf_get_ifp(sim->drvr, client_ifc_.iface_id_);
    zx_status_t status =
        brcmf_fil_iovar_int_set(ifp, "assoc_retry_max", max_assoc_retries, nullptr);
    EXPECT_EQ(status, ZX_OK);
  });
  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  // Should have not received any responses
  EXPECT_EQ(assoc_responses_.size(), 0U);
  // Make sure we got our response from the driver
  EXPECT_EQ(context_.connect_resp_count, 1U);
}

// Verify that association is attempted when retries is set to zero
TEST_F(ConnectTest, AssocNoRetries) {
  // Create our device instance
  Init();

  // Start up fake APs
  simulation::FakeAp ap(env_.get(), kDefaultBssid, kDefaultSsid, kDefaultChannel,
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap.SetAssocHandling(simulation::FakeAp::ASSOC_REFUSED);
  aps_.push_back(&ap);

  context_.expected_results.push_front(wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);

  zx_status_t status;
  uint32_t max_assoc_retries = 0;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    struct brcmf_if* ifp = brcmf_get_ifp(sim->drvr, client_ifc_.iface_id_);
    status = brcmf_fil_iovar_int_set(ifp, "assoc_retry_max", max_assoc_retries, nullptr);
    EXPECT_EQ(status, ZX_OK);
  });
  env_->ScheduleNotification(std::bind(&ConnectTest::StartConnect, this), zx::msec(10));

  env_->Run(kTestDuration);

  uint32_t assoc_retries;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    struct brcmf_if* ifp = brcmf_get_ifp(sim->drvr, client_ifc_.iface_id_);
    status = brcmf_fil_iovar_int_get(ifp, "assoc_retry_max", &assoc_retries, nullptr);
    EXPECT_EQ(status, ZX_OK);
    ASSERT_EQ(max_assoc_retries, assoc_retries);
  });

  // We should have gotten a refusal from the fake AP.
  EXPECT_EQ(assoc_responses_.size(), 1U);
  EXPECT_EQ(assoc_responses_.front().status, wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);

  // The AP should have received 1 deauth, no matter there were how many firmware assoc retries.
  EXPECT_EQ(deauth_frames_.size(), 1U);
  EXPECT_EQ(deauth_frames_.front(), wlan_ieee80211::ReasonCode::kStaLeaving);

  // Make sure we got our response from the driver
  EXPECT_EQ(context_.connect_resp_count, 1U);
}
}  // namespace wlan::brcmfmac
