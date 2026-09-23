// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/errors.h>

#include <memory>

#include <wlan/drivers/macaddr.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-frame.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-fake-ap/sim-fake-ap.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/feature.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"
#include "zxtest/zxtest.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {

// Some default AP and association request values

constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kAp0Channel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 9};
constexpr fuchsia_wlan_ieee80211::wire::ChannelNumber kAp1Channel = {
    .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 11};
const uint8_t kAp1OperatingClass = 101;

const MacAddr kAp0Bssid("12:34:56:78:9a:bc");
const MacAddr kAp1Bssid("ff:ee:dd:cc:bb:aa");

class WnmTest : public SimTest {
 public:
  // How long an individual test will run for. We need an end time because tests run until no more
  // events remain and so we need to stop aps from beaconing to drain the event queue.
  static constexpr zx::duration kTestDuration = zx::sec(100);

  // If a test needs firmware BTM enabled in the driver, this PreInit must be called.
  void PreInit();

  void Init();

  // Schedule a future BSS Transition Management request event.
  void ScheduleBtmReq(const simulation::SimBtmReqFrame& btm_req, zx::duration when);

 protected:
  // This is the interface we will use for our single client interface
  SimInterface client_ifc_;
  std::list<simulation::FakeAp*> aps_;
  // If set to true, the driver will be initialized with firmware BTM features.
  bool setup_btm_firmware_support_ = false;

  size_t btm_req_frame_count_ = 0;

 private:
  // Stationifc override
  void Rx(std::shared_ptr<const simulation::SimFrame> frame,
          std::shared_ptr<const simulation::WlanRxInfo> info) override;
};

// Set up the driver feature flags before the device is created.
void WnmTest::PreInit() {
  ASSERT_EQ(SimTest::PreInit(), ZX_OK);
  WithSimDevice([this](brcmfmac::SimDevice* device) {
    if (setup_btm_firmware_support_) {
      device->GetSim()->drvr->feat_flags |= BIT(BRCMF_FEAT_WNM_BTM);
    } else {
      device->GetSim()->drvr->feat_flags &= !(BIT(BRCMF_FEAT_WNM_BTM));
    }
  });
  // Set to false here to prevent this from being enabled inadvertently in future tests.
  setup_btm_firmware_support_ = false;
}

// Create our device instance and hook up the callbacks
void WnmTest::Init() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);

  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
  btm_req_frame_count_ = 0;
}

// This function schedules a BTM req frame sent from the first AP.
void WnmTest::ScheduleBtmReq(const simulation::SimBtmReqFrame& btm_req, zx::duration when) {
  BRCMF_INFO("Scheduling BTM");
  ZX_ASSERT_MSG(!aps_.empty(), "Cannot send BTM req because there is no AP\n");
  env_->ScheduleNotification([this, btm_req] { aps_.front()->SendBtmReq(btm_req); }, when);
}

void WnmTest::Rx(std::shared_ptr<const simulation::SimFrame> frame,
                 std::shared_ptr<const simulation::WlanRxInfo> info) {
  ASSERT_EQ(frame->FrameType(), simulation::SimFrame::FRAME_TYPE_MGMT);
  auto mgmt_frame = std::static_pointer_cast<const simulation::SimManagementFrame>(frame);

  if (mgmt_frame->MgmtFrameType() != simulation::SimManagementFrame::FRAME_TYPE_ACTION) {
    return;
  }
  auto action_frame = std::static_pointer_cast<const simulation::SimActionFrame>(mgmt_frame);
  if (action_frame->ActionCategory() != simulation::SimActionFrame::SimActionCategory::WNM) {
    return;
  }
  auto wnm_action_frame =
      std::static_pointer_cast<const simulation::SimWnmActionFrame>(action_frame);
  if (wnm_action_frame->WnmAction() !=
      simulation::SimWnmActionFrame::SimWnmAction::BSS_TRANSITION_MANAGEMENT_REQUEST) {
    return;
  }
  ++btm_req_frame_count_;
}

TEST_F(WnmTest, IgnoreBtmReqWhenBtmUnsupported) {
  Init();

  simulation::FakeAp ap_0(env_.get(), kAp0Bssid, kDefaultSsid, kAp0Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  simulation::FakeAp ap_1(env_.get(), kAp1Bssid, kDefaultSsid, kAp1Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap_0.EnableBeacon(zx::msec(60));
  ap_1.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap_0);
  aps_.push_back(&ap_1);

  client_ifc_.AssociateWith(ap_0, zx::msec(10));

  MacAddr client_mac;
  client_ifc_.GetMacAddr(&client_mac);
  const simulation::SimBtmReqMode req_mode{.preferred_candidate_list_included = true};
  const simulation::SimNeighborReportElement neighbor{
      .bssid = kAp1Bssid,
      .operating_class = kAp1OperatingClass,
      .channel_number = kAp1Channel.number,
  };
  const std::vector<simulation::SimNeighborReportElement> candidates({neighbor});
  const simulation::SimBtmReqFrame btm_req(kAp0Bssid, client_mac, req_mode, candidates);
  ScheduleBtmReq(btm_req, zx::sec(1));
  env_->Run(kTestDuration);

  EXPECT_EQ(SimInterface::AssocContext::kAssociated, client_ifc_.assoc_ctx_.state);
  EXPECT_EQ(kAp0Bssid, client_ifc_.assoc_ctx_.bssid);
  EXPECT_EQ(1U, client_ifc_.stats_.connect_attempts);
  EXPECT_EQ(1U, ap_0.GetNumAssociatedClient());
  EXPECT_EQ(0U, ap_1.GetNumAssociatedClient());
  EXPECT_EQ(1U, btm_req_frame_count_);
}

// DUT is configured to roam when AP sends a BTM request. In this test the target
// AP will respond with reassociation success and the DUT should roam successfully.
TEST_F(WnmTest, RoamOnBtmReqWhenConfiguredToRoam) {
  setup_btm_firmware_support_ = true;
  PreInit();
  Init();

  simulation::FakeAp ap_0(env_.get(), kAp0Bssid, kDefaultSsid, kAp0Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  simulation::FakeAp ap_1(env_.get(), kAp1Bssid, kDefaultSsid, kAp1Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap_0.EnableBeacon(zx::msec(60));
  ap_1.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap_0);
  aps_.push_back(&ap_1);

  client_ifc_.AssociateWith(ap_0, zx::msec(10));

  MacAddr client_mac;
  client_ifc_.GetMacAddr(&client_mac);
  const simulation::SimBtmReqMode req_mode{.preferred_candidate_list_included = true};
  const simulation::SimNeighborReportElement neighbor{
      .bssid = kAp1Bssid,
      .operating_class = kAp1OperatingClass,
      .channel_number = kAp1Channel.number,
  };
  const std::vector<simulation::SimNeighborReportElement> candidates({neighbor});
  const simulation::SimBtmReqFrame btm_req(kAp0Bssid, client_mac, req_mode, candidates);
  ScheduleBtmReq(btm_req, zx::sec(1));
  env_->Run(kTestDuration);

  EXPECT_EQ(SimInterface::AssocContext::kAssociated, client_ifc_.assoc_ctx_.state);
  EXPECT_EQ(kAp1Bssid, client_ifc_.assoc_ctx_.bssid);
  EXPECT_EQ(1U, client_ifc_.stats_.connect_attempts);
  // We don't check that the STA is not associated with the previous Sim AP,
  // because Sim AP will maintain the association indefinitely.
  EXPECT_EQ(1U, ap_1.GetNumAssociatedClient());
  EXPECT_EQ(1U, btm_req_frame_count_);
}

// DUT is configured to roam when AP sends a BTM request, but the target AP will ignore the
// reassociation. This test exercises the roam timeout functionality in the driver.
TEST_F(WnmTest, RoamOnBtmReqButTargetApIgnoresReassoc) {
  setup_btm_firmware_support_ = true;
  PreInit();
  Init();

  simulation::FakeAp ap_0(env_.get(), kAp0Bssid, kDefaultSsid, kAp0Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  simulation::FakeAp ap_1(env_.get(), kAp1Bssid, kDefaultSsid, kAp1Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap_0.EnableBeacon(zx::msec(60));
  ap_1.EnableBeacon(zx::msec(60));
  // This AP will ignore reassociation, to test roam failure.
  ap_1.SetAssocHandling(simulation::FakeAp::ASSOC_IGNORED);
  aps_.push_back(&ap_0);
  aps_.push_back(&ap_1);

  client_ifc_.AssociateWith(ap_0, zx::msec(10));

  MacAddr client_mac;
  client_ifc_.GetMacAddr(&client_mac);
  const simulation::SimBtmReqMode req_mode{.preferred_candidate_list_included = true};
  const simulation::SimNeighborReportElement neighbor{
      .bssid = kAp1Bssid,
      .operating_class = kAp1OperatingClass,
      .channel_number = kAp1Channel.number,
  };
  const std::vector<simulation::SimNeighborReportElement> candidates({neighbor});
  const simulation::SimBtmReqFrame btm_req(kAp0Bssid, client_mac, req_mode, candidates);
  ScheduleBtmReq(btm_req, zx::sec(1));
  env_->Run(kTestDuration);

  // Roam failure will incur disconnect from original AP.
  EXPECT_EQ(SimInterface::AssocContext::kNone, client_ifc_.assoc_ctx_.state);
  EXPECT_EQ(kAp0Bssid, client_ifc_.assoc_ctx_.bssid);
  EXPECT_EQ(1U, client_ifc_.stats_.connect_attempts);
  // STA should not have associated with the target AP.
  EXPECT_EQ(0U, ap_1.GetNumAssociatedClient());
  EXPECT_EQ(1U, btm_req_frame_count_);

  // Roam result should have been sent.
  ASSERT_EQ(client_ifc_.stats_.roam_result_indications.size(), 1U);
  const auto& roam_result_ind = client_ifc_.stats_.roam_result_indications.front();
  ASSERT_TRUE(roam_result_ind.status_code().has_value());
  EXPECT_FALSE(roam_result_ind.original_association_maintained().value());
  ASSERT_TRUE(roam_result_ind.target_bss_authenticated().has_value());
  // In this case, target AP has allowed authentication but ignored association.
  EXPECT_TRUE(roam_result_ind.target_bss_authenticated().value());
  ASSERT_TRUE(roam_result_ind.status_code().has_value());
  EXPECT_NE(wlan_ieee80211::StatusCode::kSuccess, roam_result_ind.status_code().value());

  // No other SME notifications about disconnects should have been sent.
  EXPECT_EQ(client_ifc_.stats_.disassoc_indications.size(), 0U);
  EXPECT_EQ(client_ifc_.stats_.deauth_indications.size(), 0U);
  EXPECT_EQ(client_ifc_.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc_.stats_.deauth_results.size(), 0U);
}

// DUT is configured to roam when AP sends a BTM request, but while waiting for the target
// BSS to respond, SME sends a deauth for the target BSS which causes the roam to fail.
TEST_F(WnmTest, RoamOnBtmReqButSmeDeauthForTargetInterruptsRoam) {
  setup_btm_firmware_support_ = true;
  PreInit();
  Init();

  simulation::FakeAp ap_0(env_.get(), kAp0Bssid, kDefaultSsid, kAp0Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  simulation::FakeAp ap_1(env_.get(), kAp1Bssid, kDefaultSsid, kAp1Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap_0.EnableBeacon(zx::msec(60));
  ap_1.EnableBeacon(zx::msec(60));
  // This AP will ignore reassociation, to give this test time to send SME Disassoc.
  ap_1.SetAssocHandling(simulation::FakeAp::ASSOC_IGNORED);
  aps_.push_back(&ap_0);
  aps_.push_back(&ap_1);

  client_ifc_.AssociateWith(ap_0, zx::msec(10));

  MacAddr client_mac;
  client_ifc_.GetMacAddr(&client_mac);
  const simulation::SimBtmReqMode req_mode{.preferred_candidate_list_included = true};
  const simulation::SimNeighborReportElement neighbor{
      .bssid = kAp1Bssid,
      .operating_class = kAp1OperatingClass,
      .channel_number = kAp1Channel.number,
  };
  const std::vector<simulation::SimNeighborReportElement> candidates({neighbor});
  const simulation::SimBtmReqFrame btm_req(kAp0Bssid, client_mac, req_mode, candidates);
  ScheduleBtmReq(btm_req, zx::sec(1));

  constexpr wlan_ieee80211::ReasonCode deauth_reason =
      wlan_ieee80211::ReasonCode::kLeavingNetworkDisassoc;
  // Schedule a deauth from SME for the target BSS, just after the roam starts.
  env_->ScheduleNotification([&] { client_ifc_.DeauthenticateFrom(kAp1Bssid, deauth_reason); },
                             zx::sec(1) + zx::msec(100));

  env_->Run(kTestDuration);

  // Any SME deauth will incur disconnect.
  EXPECT_EQ(SimInterface::AssocContext::kNone, client_ifc_.assoc_ctx_.state);
  EXPECT_EQ(kAp0Bssid, client_ifc_.assoc_ctx_.bssid);
  EXPECT_EQ(1U, client_ifc_.stats_.connect_attempts);
  // STA should not have associated with the target AP.
  EXPECT_EQ(0U, ap_1.GetNumAssociatedClient());
  EXPECT_EQ(1U, btm_req_frame_count_);

  // Also verify that we got only the right disconnect.
  ASSERT_EQ(client_ifc_.stats_.deauth_results.size(), 1U);
  const auto& deauth_conf = client_ifc_.stats_.deauth_results.front();
  ASSERT_TRUE(deauth_conf.peer_sta_address().has_value());
  ASSERT_EQ(ETH_ALEN, deauth_conf.peer_sta_address()->size());
  EXPECT_BYTES_EQ(deauth_conf.peer_sta_address()->data(), kAp1Bssid.byte, ETH_ALEN);

  EXPECT_EQ(client_ifc_.stats_.deauth_indications.size(), 0U);
  EXPECT_EQ(client_ifc_.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc_.stats_.disassoc_indications.size(), 0U);
}

// DUT is configured to roam when AP sends a BTM request, but while waiting for the target
// BSS to respond, SME sends a disassoc which causes the roam to fail.
TEST_F(WnmTest, RoamOnBtmReqButSmeDisassocInterruptsRoam) {
  setup_btm_firmware_support_ = true;
  PreInit();
  Init();

  simulation::FakeAp ap_0(env_.get(), kAp0Bssid, kDefaultSsid, kAp0Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  simulation::FakeAp ap_1(env_.get(), kAp1Bssid, kDefaultSsid, kAp1Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap_0.EnableBeacon(zx::msec(60));
  ap_1.EnableBeacon(zx::msec(60));
  // This AP will ignore reassociation, to give this test time to send SME Disassoc.
  ap_1.SetAssocHandling(simulation::FakeAp::ASSOC_IGNORED);
  aps_.push_back(&ap_0);
  aps_.push_back(&ap_1);

  client_ifc_.AssociateWith(ap_0, zx::msec(10));

  MacAddr client_mac;
  client_ifc_.GetMacAddr(&client_mac);
  const simulation::SimBtmReqMode req_mode{.preferred_candidate_list_included = true};
  const simulation::SimNeighborReportElement neighbor{
      .bssid = kAp1Bssid,
      .operating_class = kAp1OperatingClass,
      .channel_number = kAp1Channel.number,
  };
  const std::vector<simulation::SimNeighborReportElement> candidates({neighbor});
  const simulation::SimBtmReqFrame btm_req(kAp0Bssid, client_mac, req_mode, candidates);
  ScheduleBtmReq(btm_req, zx::sec(1));

  constexpr auto disassoc_reason = wlan_ieee80211::ReasonCode::kLeavingNetworkDisassoc;
  // Schedule a disassoc from SME, just after the roam starts.
  env_->ScheduleNotification([&] { client_ifc_.DisassociateFrom(kAp0Bssid, disassoc_reason); },
                             zx::sec(1) + zx::msec(100));

  env_->Run(kTestDuration);

  // SME disassoc will incur disconnect.
  EXPECT_EQ(SimInterface::AssocContext::kNone, client_ifc_.assoc_ctx_.state);
  EXPECT_EQ(kAp0Bssid, client_ifc_.assoc_ctx_.bssid);
  EXPECT_EQ(1U, client_ifc_.stats_.connect_attempts);
  // STA should not have associated with the target AP.
  EXPECT_EQ(0U, ap_1.GetNumAssociatedClient());
  EXPECT_EQ(1U, btm_req_frame_count_);

  // Also verify that we got only the right disconnect.
  EXPECT_EQ(client_ifc_.stats_.disassoc_results.size(), 1U);
  EXPECT_EQ(client_ifc_.stats_.deauth_indications.size(), 0U);
  EXPECT_EQ(client_ifc_.stats_.disassoc_indications.size(), 0U);
  EXPECT_EQ(client_ifc_.stats_.deauth_results.size(), 0U);
}

// DUT is configured to roam when AP sends a BTM request, but while waiting for the target
// BSS to respond, SME sends a deauth for the current BSS which causes the roam to fail.
TEST_F(WnmTest, RoamOnBtmReqButSmeDeauthInterruptsRoam) {
  setup_btm_firmware_support_ = true;
  PreInit();
  Init();

  simulation::FakeAp ap_0(env_.get(), kAp0Bssid, kDefaultSsid, kAp0Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  simulation::FakeAp ap_1(env_.get(), kAp1Bssid, kDefaultSsid, kAp1Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap_0.EnableBeacon(zx::msec(60));
  ap_1.EnableBeacon(zx::msec(60));
  // This AP will ignore reassociation, to give this test time to send SME deauth.
  ap_1.SetAssocHandling(simulation::FakeAp::ASSOC_IGNORED);
  aps_.push_back(&ap_0);
  aps_.push_back(&ap_1);

  client_ifc_.AssociateWith(ap_0, zx::msec(10));

  MacAddr client_mac;
  client_ifc_.GetMacAddr(&client_mac);
  const simulation::SimBtmReqMode req_mode{.preferred_candidate_list_included = true};
  const simulation::SimNeighborReportElement neighbor{
      .bssid = kAp1Bssid,
      .operating_class = kAp1OperatingClass,
      .channel_number = kAp1Channel.number,
  };
  const std::vector<simulation::SimNeighborReportElement> candidates({neighbor});
  const simulation::SimBtmReqFrame btm_req(kAp0Bssid, client_mac, req_mode, candidates);
  ScheduleBtmReq(btm_req, zx::sec(1));

  constexpr wlan_ieee80211::ReasonCode deauth_reason =
      wlan_ieee80211::ReasonCode::kLeavingNetworkDeauth;
  // Schedule a deauth from SME, just after the roam starts.
  env_->ScheduleNotification([&] { client_ifc_.DeauthenticateFrom(kAp0Bssid, deauth_reason); },
                             zx::sec(1) + zx::msec(100));

  env_->Run(kTestDuration);

  // SME deauth will incur disconnect.
  EXPECT_EQ(SimInterface::AssocContext::kNone, client_ifc_.assoc_ctx_.state);
  EXPECT_EQ(kAp0Bssid, client_ifc_.assoc_ctx_.bssid);
  EXPECT_EQ(1U, client_ifc_.stats_.connect_attempts);
  // STA should not have associated with the target AP.
  EXPECT_EQ(0U, ap_1.GetNumAssociatedClient());
  EXPECT_EQ(1U, btm_req_frame_count_);

  // Also verify that we got only the right disconnect.
  ASSERT_EQ(client_ifc_.stats_.deauth_results.size(), 1U);
  const auto& deauth_conf = client_ifc_.stats_.deauth_results.front();
  ASSERT_TRUE(deauth_conf.peer_sta_address().has_value());
  EXPECT_BYTES_EQ(deauth_conf.peer_sta_address()->data(), kAp0Bssid.byte, ETH_ALEN);

  EXPECT_EQ(client_ifc_.stats_.deauth_indications.size(), 0U);
  EXPECT_EQ(client_ifc_.stats_.disassoc_results.size(), 0U);
  EXPECT_EQ(client_ifc_.stats_.disassoc_indications.size(), 0U);
}

// DUT is configured to roam when AP sends a BTM request, but the firmware
// fails to obtain the target BSS info (e.g. due to lack of firmware support, or
// some other firmware error).
TEST_F(WnmTest, DisconnectOnBtmReqWhenTargetBssInfoUnsupported) {
  setup_btm_firmware_support_ = true;
  PreInit();
  Init();

  // Inject firmware error to "target_bss_info" iovar.
  WithSimDevice([this](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjIovar("target_bss_info", ZX_ERR_NOT_SUPPORTED, BCME_UNSUPPORTED,
                                         client_ifc_.iface_id_);
  });

  simulation::FakeAp ap_0(env_.get(), kAp0Bssid, kDefaultSsid, kAp0Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  simulation::FakeAp ap_1(env_.get(), kAp1Bssid, kDefaultSsid, kAp1Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap_0.EnableBeacon(zx::msec(60));
  ap_1.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap_0);
  aps_.push_back(&ap_1);

  client_ifc_.AssociateWith(ap_0, zx::msec(10));

  MacAddr client_mac;
  client_ifc_.GetMacAddr(&client_mac);
  const simulation::SimBtmReqMode req_mode{.preferred_candidate_list_included = true};
  const simulation::SimNeighborReportElement neighbor{
      .bssid = kAp1Bssid,
      .operating_class = kAp1OperatingClass,
      .channel_number = kAp1Channel.number,
  };
  const std::vector<simulation::SimNeighborReportElement> candidates({neighbor});
  const simulation::SimBtmReqFrame btm_req(kAp0Bssid, client_mac, req_mode, candidates);
  ScheduleBtmReq(btm_req, zx::sec(1));
  env_->Run(kTestDuration);

  EXPECT_EQ(SimInterface::AssocContext::kNone, client_ifc_.assoc_ctx_.state);
  EXPECT_EQ(kAp0Bssid, client_ifc_.assoc_ctx_.bssid);
  EXPECT_EQ(1U, client_ifc_.stats_.connect_attempts);
  EXPECT_EQ(1U, btm_req_frame_count_);

  // Roam result should have been sent.
  ASSERT_EQ(client_ifc_.stats_.roam_result_indications.size(), 1U);
  const auto& roam_result_ind = client_ifc_.stats_.roam_result_indications.front();
  ASSERT_TRUE(roam_result_ind.status_code().has_value());
  EXPECT_FALSE(roam_result_ind.original_association_maintained().value());
  ASSERT_TRUE(roam_result_ind.target_bss_authenticated().has_value());
  // In this case, DUT does not attempt to roam to target.
  EXPECT_FALSE(roam_result_ind.target_bss_authenticated().value());
  ASSERT_TRUE(roam_result_ind.status_code().has_value());
  EXPECT_NE(wlan_ieee80211::StatusCode::kSuccess, roam_result_ind.status_code().value());
}

// DUT is configured to roam when AP sends a BTM request, but the firmware
// returns target BSS info with a malformed IE buffer.
TEST_F(WnmTest, DisconnectOnBtmReqWhenTargetBssInfoIeBufferMalformed) {
  setup_btm_firmware_support_ = true;
  PreInit();
  Init();

  // This needs to have the same lifetime as the test because the sim FW will read from this
  // buffer during the test.
  const std::vector<uint8_t> malformed_ie_buf{0, 0, 0, 0};

  // Inject firmware error to "target_bss_info" iovar.
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    // IE buffer that is all zero values, and is too short.
    sim->sim_fw->err_inj_.AddErrInjIovar("target_bss_info", ZX_OK, BCME_OK, client_ifc_.iface_id_,
                                         &malformed_ie_buf);
  });

  simulation::FakeAp ap_0(env_.get(), kAp0Bssid, kDefaultSsid, kAp0Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  simulation::FakeAp ap_1(env_.get(), kAp1Bssid, kDefaultSsid, kAp1Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap_0.EnableBeacon(zx::msec(60));
  ap_1.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap_0);
  aps_.push_back(&ap_1);

  client_ifc_.AssociateWith(ap_0, zx::msec(10));

  MacAddr client_mac;
  client_ifc_.GetMacAddr(&client_mac);
  const simulation::SimBtmReqMode req_mode{.preferred_candidate_list_included = true};
  const simulation::SimNeighborReportElement neighbor{
      .bssid = kAp1Bssid,
      .operating_class = kAp1OperatingClass,
      .channel_number = kAp1Channel.number,
  };
  const std::vector<simulation::SimNeighborReportElement> candidates({neighbor});
  const simulation::SimBtmReqFrame btm_req(kAp0Bssid, client_mac, req_mode, candidates);
  ScheduleBtmReq(btm_req, zx::sec(1));
  env_->Run(kTestDuration);

  EXPECT_EQ(SimInterface::AssocContext::kNone, client_ifc_.assoc_ctx_.state);
  EXPECT_EQ(kAp0Bssid, client_ifc_.assoc_ctx_.bssid);
  EXPECT_EQ(1U, client_ifc_.stats_.connect_attempts);
  EXPECT_EQ(1U, btm_req_frame_count_);

  // Roam result should have been sent.
  ASSERT_EQ(client_ifc_.stats_.roam_result_indications.size(), 1U);
  const auto& roam_result_ind = client_ifc_.stats_.roam_result_indications.front();
  ASSERT_TRUE(roam_result_ind.status_code().has_value());
  EXPECT_FALSE(roam_result_ind.original_association_maintained().value());
  ASSERT_TRUE(roam_result_ind.target_bss_authenticated().has_value());
  // In this case, DUT does not attempt to roam to target.
  EXPECT_FALSE(roam_result_ind.target_bss_authenticated().value());
  ASSERT_TRUE(roam_result_ind.status_code().has_value());
  EXPECT_NE(wlan_ieee80211::StatusCode::kSuccess, roam_result_ind.status_code().value());
}

// DUT is configured to roam when AP sends a BTM request, but the firmware returns BSS info designed
// to trigger a heap buffer overflow/out-of-bounds read in the driver. This test ensures the driver
// safely rejects the invalid BSS info and tears down the connection without crashing.
TEST_F(WnmTest, FwInitiatedRoamHeapBufferOverflow) {
  setup_btm_firmware_support_ = true;
  PreInit();
  Init();

  simulation::FakeAp ap_0(env_.get(), kAp0Bssid, kDefaultSsid, kAp0Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  simulation::FakeAp ap_1(env_.get(), kAp1Bssid, kDefaultSsid, kAp1Channel,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);
  ap_0.EnableBeacon(zx::msec(60));
  ap_1.EnableBeacon(zx::msec(60));
  aps_.push_back(&ap_0);
  aps_.push_back(&ap_1);

  // 1. Associate with ap_0
  client_ifc_.AssociateWith(ap_0, zx::msec(10));
  env_->Run(zx::sec(1));
  ASSERT_EQ(SimInterface::AssocContext::kAssociated, client_ifc_.assoc_ctx_.state);

  // 2. Prepare Association Response IEs that are too long.
  // We fill it with a pattern to detect overflow.
  constexpr size_t kLargeAssocRespIesSize =
      WL_ASSOC_INFO_MAX;                  // Driver's assoc resp IEs limit (512)
  constexpr uint8_t kFillPattern = 0x41;  // 'A'
  std::vector<uint8_t> large_assoc_resp_ies(kLargeAssocRespIesSize, kFillPattern);

  // 3. Prepare BSS info that triggers the boundary check.
  // We set ie_offset to point past the overwritten BSS info part.
  // We set ie_length to a value that exceeds the remaining size of extra_buf (2048 - 4 = 2044).
  constexpr uint16_t kDefaultBeaconPeriod = 100;
  constexpr uint16_t kOverflowIeOffset = sizeof(brcmf_bss_info_le);  // Point past BSS info struct

  // A truly malicious length that causes the offset + length to exceed WL_EXTRA_BUF_MAX (2048)
  // which triggers our new, secure boundary check.
  constexpr uint32_t kOverflowIeLength = 2000;

  static_assert(
      kOverflowIeOffset + kOverflowIeLength > 2044,
      "Overflow length + offset must exceed the secure boundary (2044) to verify rejection");

  brcmf_bss_info_le overflow_bss_info;
  memset(&overflow_bss_info, 0, sizeof(overflow_bss_info));
  memcpy(&overflow_bss_info.BSSID, kAp1Bssid.byte, ETH_ALEN);
  overflow_bss_info.beacon_period = kDefaultBeaconPeriod;
  overflow_bss_info.capability = 0;
  overflow_bss_info.ie_offset = kOverflowIeOffset;  // point to the long IEs in extra_buf
  overflow_bss_info.ie_length = kOverflowIeLength;  // large length to trigger overflow

  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    // Directly inject the long assoc resp IEs into firmware's cache
    sim->sim_fw->SetAssocRespIes(large_assoc_resp_ies);
    // Override BRCMF_C_GET_BSS_INFO to return our BSS info designed to overflow
    sim->sim_fw->SetBssInfoOverride(overflow_bss_info);
  });

  // 4. Trigger a BTM request from ap_0 to roam to ap_1.
  MacAddr client_mac;
  client_ifc_.GetMacAddr(&client_mac);
  const simulation::SimBtmReqMode req_mode{.preferred_candidate_list_included = true};
  const simulation::SimNeighborReportElement neighbor{
      .bssid = kAp1Bssid,
      .operating_class = kAp1OperatingClass,
      .channel_number = kAp1Channel.number,
  };
  const std::vector<simulation::SimNeighborReportElement> candidates({neighbor});
  const simulation::SimBtmReqFrame btm_req(kAp0Bssid, client_mac, req_mode, candidates);

  // Schedule the BTM request
  ScheduleBtmReq(btm_req, zx::sec(1));

  // Run the environment. This should trigger the firmware-initiated roam,
  // which will lead to heap buffer overflow if vulnerable.
  env_->Run(kTestDuration);

  // Ensure that the client is disconnected because of the rejected roam attempt.
  EXPECT_EQ(SimInterface::AssocContext::kNone, client_ifc_.assoc_ctx_.state);
  EXPECT_EQ(1U, client_ifc_.stats_.connect_attempts);
  EXPECT_EQ(1U, btm_req_frame_count_);

  // Roam result should have been sent indicating failure.
  ASSERT_EQ(client_ifc_.stats_.roam_result_indications.size(), 1U);
  const auto& roam_result_ind = client_ifc_.stats_.roam_result_indications.front();
  ASSERT_TRUE(roam_result_ind.status_code().has_value());
  EXPECT_FALSE(roam_result_ind.original_association_maintained().value());
  ASSERT_TRUE(roam_result_ind.target_bss_authenticated().has_value());
  EXPECT_NE(wlan_ieee80211::StatusCode::kSuccess, roam_result_ind.status_code().value());
}

}  // namespace wlan::brcmfmac
