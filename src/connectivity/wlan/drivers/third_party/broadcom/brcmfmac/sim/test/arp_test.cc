// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <arpa/inet.h>
#include <fuchsia/wlan/ieee80211/cpp/fidl.h>
#include <zircon/errors.h>

#include <wlan/drivers/macaddr.h>
#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-env.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-fake-ap/sim-fake-ap.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/cfg80211.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/feature.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_data_path.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_utils.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {

namespace {
#define OUR_MAC {0xff, 0xfe, 0xfd, 0xfc, 0xfb, 0xfa}
#define THEIR_MAC {0xde, 0xad, 0xbe, 0xef, 0x00, 0x02}
const MacAddr kOurMac(OUR_MAC);
const MacAddr kTheirMac(THEIR_MAC);

// A simple ARP request
const ether_arp kSampleArpReq = {.ea_hdr = {.ar_hrd = htons(ETH_P_802_3),
                                            .ar_pro = htons(ETH_P_IP),
                                            .ar_hln = 6,
                                            .ar_pln = 4,
                                            .ar_op = htons(ARPOP_REQUEST)},
                                 .arp_sha = OUR_MAC,
                                 .arp_spa = {192, 168, 42, 11},
                                 .arp_tha = THEIR_MAC,
                                 .arp_tpa = {100, 101, 102, 103}};

const std::vector<uint8_t> kDummyData = {0, 1, 2, 3, 4};

void VerifyArpFrame(const std::vector<uint8_t>& frame) {
  const ethhdr* eth_hdr = reinterpret_cast<const ethhdr*>(frame.data());
  EXPECT_BYTES_EQ(eth_hdr->h_dest, common::kBcastMac.byte, ETH_ALEN);
  EXPECT_BYTES_EQ(eth_hdr->h_source, kTheirMac.byte, ETH_ALEN);
  EXPECT_EQ(ntohs(eth_hdr->h_proto), ETH_P_ARP);

  const ether_arp* arp_hdr = reinterpret_cast<const ether_arp*>(frame.data() + sizeof(ethhdr));
  EXPECT_EQ(memcmp(arp_hdr, &kSampleArpReq, sizeof(ether_arp)), 0);
}

void VerifyNonArpFrame(const std::vector<uint8_t>& frame) {
  // For this test only, assume anything that is the right size is an ARP frame
  ASSERT_NE(frame.size(), sizeof(ethhdr) + sizeof(ether_arp));
  ASSERT_EQ(frame.size(), sizeof(ethhdr) + kDummyData.size());
  EXPECT_BYTES_EQ(kDummyData.data(), &frame[sizeof(ethhdr)], kDummyData.size());
}

}  // namespace

class ArpTest;

struct GenericIfc : public SimInterface {
 public:
  void AssocInd(AssocIndRequestView request, AssocIndCompleter::Sync& completer) override;

  void StartConf(StartConfRequestView request, StartConfCompleter::Sync& completer) override;

  void StopConf(StopConfRequestView request, StopConfCompleter::Sync& completer) override;

  ArpTest* test_;
};

class ArpTest : public SimTest {
 public:
  static constexpr zx::duration kTestDuration = zx::sec(100);

  void Init();
  void CleanupApInterface();

  void OnAssocInd(const fuchsia_wlan_fullmac::WlanFullmacImplIfcAssocIndRequest* ind);
  void OnStartConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcStartConfRequest* resp);
  void OnStopConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcStopConfRequest* resp);

  // Send a frame directly into the environment
  void Tx(const std::vector<uint8_t>& ethFrame);

  // Interface management
  void StartAndStopSoftAP();

  // Simulation of client associating to a SoftAP interface
  void TxAuthandAssocReq();
  void VerifyAssoc();

  // Frame processing
  void ScheduleArpFrameTx(zx::duration when, bool expect_rx);
  void ScheduleNonArpFrameTx(zx::duration when);

  bool assoc_ind_recv_ = false;

 protected:
  std::vector<uint8_t> ethFrame;
  GenericIfc sim_ifc_;
};

void GenericIfc::AssocInd(AssocIndRequestView request, AssocIndCompleter::Sync& completer) {
  auto assoc_ind = fidl::ToNatural(*request);
  test_->OnAssocInd(&assoc_ind);
  completer.Reply();
}

void GenericIfc::StartConf(StartConfRequestView request, StartConfCompleter::Sync& completer) {
  auto start_conf = fidl::ToNatural(*request);
  test_->OnStartConf(&start_conf);
  completer.Reply();
}

void GenericIfc::StopConf(StopConfRequestView request, StopConfCompleter::Sync& completer) {
  auto stop_conf = fidl::ToNatural(*request);
  test_->OnStopConf(&stop_conf);
  completer.Reply();
}

void ArpTest::OnAssocInd(const fuchsia_wlan_fullmac::WlanFullmacImplIfcAssocIndRequest* ind) {
  ASSERT_EQ(std::memcmp(ind->peer_sta_address()->data(), kTheirMac.byte, ETH_ALEN), 0);
  assoc_ind_recv_ = true;
}

void ArpTest::OnStartConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcStartConfRequest* resp) {
  ASSERT_EQ(resp->result_code().value(), wlan_fullmac_wire::StartResult::kSuccess);
}

void ArpTest::OnStopConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcStopConfRequest* resp) {
  ASSERT_EQ(resp->result_code().value(), wlan_fullmac_wire::StopResult::kSuccess);
}

void ArpTest::Init() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);
  sim_ifc_.test_ = this;
}

void ArpTest::TxAuthandAssocReq() {
  // Get the mac address of the SoftAP
  const MacAddr mac(kTheirMac);
  simulation::WlanTxInfo tx_info = {
      .primary_channel = SimInterface::kDefaultSoftApChannelNum,
      .bandwidth = fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20,
      .vht_secondary_80_channel = {.band = SimInterface::kDefaultSoftApChannelNum.band,
                                   .number = 0}};
  simulation::SimAuthFrame auth_req_frame(mac, kOurMac, 1, simulation::AUTH_TYPE_OPEN,
                                          wlan_ieee80211::StatusCode::kSuccess);
  env_->Tx(auth_req_frame, tx_info, this);
  simulation::SimAssocReqFrame assoc_req_frame(mac, kOurMac, kDefaultSoftApSsid);
  env_->Tx(assoc_req_frame, tx_info, this);
}

void ArpTest::VerifyAssoc() {
  // Verify the event indications were received and
  // the number of clients
  ASSERT_EQ(assoc_ind_recv_, true);
  WithSimDevice([this](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    uint16_t num_clients = sim->sim_fw->GetNumClients(sim_ifc_.iface_id_);
    ASSERT_EQ(num_clients, 1U);
  });
}

void ArpTest::CleanupApInterface() {
  sim_ifc_.StopSoftAp();
  EXPECT_EQ(DeleteInterface(&sim_ifc_), ZX_OK);
}

void ArpTest::Tx(const std::vector<uint8_t>& ethFrame) {
  const ethhdr* eth_hdr = reinterpret_cast<const ethhdr*>(ethFrame.data());
  MacAddr dst(eth_hdr->h_dest);
  MacAddr src(eth_hdr->h_source);
  simulation::SimQosDataFrame dataFrame(true, false, dst, src, common::kBcastMac, 0, ethFrame);
  simulation::WlanTxInfo tx_info = {
      .primary_channel = SimInterface::kDefaultSoftApChannelNum,
      .bandwidth = fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20,
      .vht_secondary_80_channel = {.band = SimInterface::kDefaultSoftApChannelNum.band,
                                   .number = 0}};
  env_->Tx(dataFrame, tx_info, this);
}

void ArpTest::ScheduleArpFrameTx(zx::duration when, bool expect_rx) {
  ether_arp arp_frame = kSampleArpReq;

  cpp20::span<const uint8_t> body{reinterpret_cast<const uint8_t*>(&arp_frame), sizeof(arp_frame)};
  std::vector<uint8_t> frame_bytes =
      sim_utils::CreateEthernetFrame(common::kBcastMac, kTheirMac, ETH_P_ARP, body);
  env_->ScheduleNotification(std::bind(&ArpTest::Tx, this, frame_bytes), when);
}

void ArpTest::ScheduleNonArpFrameTx(zx::duration when) {
  std::vector<uint8_t> frame_bytes =
      sim_utils::CreateEthernetFrame(kOurMac, kTheirMac, 0, kDummyData);
  env_->ScheduleNotification(std::bind(&ArpTest::Tx, this, frame_bytes), when);
}

void ArpTest::StartAndStopSoftAP() {
  MacAddr ap_mac({0xfa, 0xfb, 0xfc, 0xfd, 0xfe, 0xff});
  GenericIfc softap_ifc;
  softap_ifc.test_ = this;

  ASSERT_EQ(SimTest::StartInterface(wlan_common::WlanMacRole::kAp, &softap_ifc, ap_mac), ZX_OK);

  softap_ifc.StartSoftAp();

  softap_ifc.StopSoftAp();
}

// Verify that an ARP frame received by an AP interface is not offloaded.
TEST_F(ArpTest, SoftApArpOffload) {
  Init();
  ASSERT_EQ(SimTest::StartInterface(wlan_common::WlanMacRole::kAp, &sim_ifc_, kOurMac), ZX_OK);
  sim_ifc_.StartSoftAp();

  // Have the test associate with the AP
  env_->ScheduleNotification(std::bind(&ArpTest::TxAuthandAssocReq, this), zx::sec(1));
  env_->ScheduleNotification(std::bind(&ArpTest::VerifyAssoc, this), zx::sec(2));

  // Send an ARP frame that we expect to be received
  ScheduleArpFrameTx(zx::sec(3), true);
  ScheduleNonArpFrameTx(zx::sec(4));

  // Send an ARP frame that we expect to be received
  ScheduleArpFrameTx(zx::sec(6), true);
  ScheduleNonArpFrameTx(zx::sec(7));

  // Stop AP and remove interface
  env_->ScheduleNotification(std::bind(&ArpTest::CleanupApInterface, this), zx::sec(8));

  env_->Run(kTestDuration);

  // Verify that no ARP frames were offloaded and that no non-ARP frames were suppressed
  WithSimDevice([](brcmfmac::SimDevice* device) {
    auto& received = device->DataPath().RxData();
    ASSERT_EQ(received.size(), 4);

    VerifyArpFrame(received[0]);
    VerifyNonArpFrame(received[1]);
    VerifyArpFrame(received[2]);
    VerifyNonArpFrame(received[3]);
  });
}

// On a client interface, we expect no ARP frames to be offloaded to firmware, since SoftAP feature
// disables ARP offload by default.
TEST_F(ArpTest, ClientArpOffload) {
  Init();

  ASSERT_EQ(SimTest::StartInterface(wlan_common::WlanMacRole::kClient, &sim_ifc_, kOurMac), ZX_OK);

  // Start a fake AP
  simulation::FakeAp ap(env_.get(), kTheirMac, kDefaultSoftApSsid,
                        fuchsia_wlan_ieee80211::wire::ChannelNumber{
                            .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz,
                            .number = SimInterface::kDefaultSoftApChannelNum.number},
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  // Associate with fake AP
  sim_ifc_.AssociateWith(ap, zx::sec(1));

  // Send an ARP frame that we expect to receive (not get offloaded)
  ScheduleArpFrameTx(zx::sec(2), false);
  ScheduleNonArpFrameTx(zx::sec(3));

  // Send another ARP frame that we expect to receive (not get offloaded)
  ScheduleArpFrameTx(zx::sec(5), false);
  ScheduleNonArpFrameTx(zx::sec(6));

  env_->Run(kTestDuration);

  // Verify that we completed the association process
  EXPECT_EQ(sim_ifc_.stats_.connect_successes, 1U);

  // Verify that no ARP frames were offloaded and that no non-ARP frames were suppressed
  WithSimDevice([](brcmfmac::SimDevice* device) {
    auto& received = device->DataPath().RxData();
    ASSERT_EQ(received.size(), 4);

    VerifyArpFrame(received[0]);
    VerifyNonArpFrame(received[1]);
    VerifyArpFrame(received[2]);
    VerifyNonArpFrame(received[3]);
  });
}

// Start and Stop of SoftAP should not affect ARP OL configured by client.
TEST_F(ArpTest, SoftAPStartStopDoesNotAffectArpOl) {
  Init();

  ASSERT_EQ(SimTest::StartInterface(wlan_common::WlanMacRole::kClient, &sim_ifc_, kOurMac), ZX_OK);

  // Start a fake AP
  simulation::FakeAp ap(env_.get(), kTheirMac, kDefaultSoftApSsid,
                        fuchsia_wlan_ieee80211::wire::ChannelNumber{
                            .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz,
                            .number = SimInterface::kDefaultSoftApChannelNum.number},
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  // Associate with fake AP
  sim_ifc_.AssociateWith(ap, zx::sec(1));

  // Send an ARP frame that we expect to receive (not get offloaded)
  ScheduleArpFrameTx(zx::sec(2), false);
  ScheduleNonArpFrameTx(zx::sec(3));

  // Start and Stop SoftAP. This should not affect ARP Ol
  env_->ScheduleNotification(std::bind(&ArpTest::StartAndStopSoftAP, this), zx::sec(4));

  // Send another ARP frame that we expect to receive (not get offloaded)
  ScheduleArpFrameTx(zx::sec(5), false);
  ScheduleNonArpFrameTx(zx::sec(6));

  env_->Run(kTestDuration);

  // Verify that we completed the association process
  EXPECT_EQ(sim_ifc_.stats_.connect_successes, 1U);

  // Verify that no ARP frames were offloaded and that no non-ARP frames were suppressed
  WithSimDevice([](brcmfmac::SimDevice* device) {
    auto& received = device->DataPath().RxData();
    ASSERT_EQ(received.size(), 4);

    VerifyArpFrame(received[0]);
    VerifyNonArpFrame(received[1]);
    VerifyArpFrame(received[2]);
    VerifyNonArpFrame(received[3]);
  });
}

// On a client interface, we expect all ARP frames to be offloaded to firmware when SoftAP feature
// is not available.
TEST_F(ArpTest, ClientArpOffloadNoSoftApFeat) {
  Init();

  // We disable SoftAP feature, so that our driver enabled Arp offload.
  WithSimDevice([](brcmfmac::SimDevice* device) {
    device->GetSim()->drvr->feat_flags &= (!BIT(BRCMF_FEAT_AP));
  });

  ASSERT_EQ(SimTest::StartInterface(wlan_common::WlanMacRole::kClient, &sim_ifc_, kOurMac), ZX_OK);

  // Start a fake AP
  simulation::FakeAp ap(env_.get(), kTheirMac, kDefaultSoftApSsid,
                        fuchsia_wlan_ieee80211::wire::ChannelNumber{
                            .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz,
                            .number = SimInterface::kDefaultSoftApChannelNum.number},
                        fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20, 0);

  // Associate with fake AP
  sim_ifc_.AssociateWith(ap, zx::sec(1));

  // Send an ARP frame that we expect to be offloaded
  ScheduleArpFrameTx(zx::sec(2), false);
  ScheduleNonArpFrameTx(zx::sec(3));

  // Send an ARP frame that we expect to be offloaded
  ScheduleArpFrameTx(zx::sec(5), false);
  ScheduleNonArpFrameTx(zx::sec(6));

  env_->Run(kTestDuration);

  // Verify that we completed the association process
  EXPECT_EQ(sim_ifc_.stats_.connect_successes, 1U);

  // Verify that all ARP frames were offloaded and that no non-ARP frames were suppressed
  WithSimDevice([](brcmfmac::SimDevice* device) {
    auto& received = device->DataPath().RxData();
    ASSERT_EQ(received.size(), 2);
    VerifyNonArpFrame(received[0]);
    VerifyNonArpFrame(received[1]);
  });
}

}  // namespace wlan::brcmfmac
