// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/wlan/ieee80211/cpp/fidl.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <zircon/errors.h>

#include <wlan/common/channel.h>
#include <wlan/drivers/macaddr.h>
#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/cfg80211.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/fwil.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/test/device_inspect_test_utils.h"
#include "src/devices/lib/broadcom/commands.h"

using wlan::common::MacAddr;

namespace wlan::brcmfmac {

namespace {

constexpr zx::duration kSimulatedClockDuration = zx::sec(10);

}  // namespace

namespace wlan_ieee80211 = wlan_ieee80211;

constexpr uint16_t kDefaultCh = 149;
constexpr fuchsia_wlan_ieee80211::wire::WlanBand kDefaultBand =
    fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz;

const MacAddr kFakeMac({0xde, 0xad, 0xbe, 0xef, 0x00, 0x02});

class CreateSoftAPTest;

class SoftApInterface : public SimInterface {
 public:
  void AuthInd(AuthIndRequestView request, AuthIndCompleter::Sync& completer) override;
  void DeauthInd(DeauthIndRequestView request, DeauthIndCompleter::Sync& completer) override;
  void DeauthConf(DeauthConfRequestView request, DeauthConfCompleter::Sync& completer) override;
  void AssocInd(AssocIndRequestView request, AssocIndCompleter::Sync& completer) override;
  void DisassocConf(DisassocConfRequestView request,
                    DisassocConfCompleter::Sync& completer) override;
  void DisassocInd(DisassocIndRequestView request, DisassocIndCompleter::Sync& completer) override;
  void StartConf(StartConfRequestView request, StartConfCompleter::Sync& completer) override;
  void StopConf(StopConfRequestView request, StopConfCompleter::Sync& completer) override;
  void OnChannelSwitch(OnChannelSwitchRequestView request,
                       OnChannelSwitchCompleter::Sync& completer) override;

  CreateSoftAPTest* test_;
};

class CreateSoftAPTest : public SimTest {
 public:
  CreateSoftAPTest() = default;
  void SetUp() override {
    Init();
    CreateInterface();
  }

  void Init();
  void CreateInterface();
  void DeleteInterface();
  zx_status_t StartSoftAP();
  zx_status_t StopSoftAP();

  // We track a specific firmware error condition seen in AP start.
  void GetApSetSsidErrInspectCount(uint64_t* out_count);

  void TxAuthReq(simulation::SimAuthType auth_type, MacAddr client_mac);
  void TxAssocReq(MacAddr client_mac);
  void TxDisassocReq(MacAddr client_mac);
  void TxDeauthReq(MacAddr client_mac);
  void DeauthClient(MacAddr client_mac);
  void VerifyAuth();
  void VerifyAssoc();
  void VerifyNotAssoc();
  void VerifyStartAPConf(wlan_fullmac_wire::StartResult status);
  void VerifyStopAPConf(wlan_fullmac_wire::StopResult status);
  void VerifyNumOfClient(uint16_t expect_client_num);
  void ClearAssocInd();
  void InjectStartAPError();
  void InjectChanspecError();
  void InjectSetSsidError();
  void InjectStopAPError();
  void SetExpectMacForInds(MacAddr set_mac);

  void OnAuthInd(const wlan_fullmac_wire::WlanFullmacImplIfcAuthIndRequest* ind);
  void OnDeauthInd(const fuchsia_wlan_fullmac::WlanFullmacImplIfcDeauthIndRequest* ind);
  void OnDeauthConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcDeauthConfRequest* resp);
  void OnAssocInd(const fuchsia_wlan_fullmac::WlanFullmacImplIfcAssocIndRequest* ind);
  void OnDisassocConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcDisassocConfRequest* resp);
  void OnDisassocInd(const fuchsia_wlan_fullmac::WlanFullmacImplIfcDisassocIndRequest* ind);
  void OnStartConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcStartConfRequest* resp);
  void OnStopConf(const fuchsia_wlan_fullmac::WlanFullmacImplIfcStopConfRequest* resp);
  void OnChannelSwitch(const fuchsia_wlan_fullmac::WlanFullmacImplIfcOnChannelSwitchRequest* ind);
  // Status field in the last received authentication frame.
  wlan_ieee80211::StatusCode auth_resp_status_;

  bool auth_ind_recv_ = false;
  bool assoc_ind_recv_ = false;
  bool deauth_conf_recv_ = false;
  bool deauth_ind_recv_ = false;
  bool disassoc_ind_recv_ = false;
  bool disassoc_conf_recv_ = false;
  bool start_conf_received_ = false;
  bool stop_conf_received_ = false;
  fuchsia_wlan_fullmac::StartResult start_conf_status_;
  fuchsia_wlan_fullmac::StopResult stop_conf_status_;

  // The expect mac address for indications
  MacAddr ind_expect_mac_ = kFakeMac;

 protected:
  simulation::WlanTxInfo tx_info_ = {
      .primary_channel = {.band = kDefaultBand, .number = kDefaultCh},
      .bandwidth = fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20,
      .vht_secondary_80_channel = {.band = kDefaultBand, .number = 0}};
  bool sec_enabled_ = false;
  SoftApInterface softap_ifc_;

 private:
  void Rx(std::shared_ptr<const simulation::SimFrame> frame,
          std::shared_ptr<const simulation::WlanRxInfo> info) override;

  uint16_t CreateRsneIe(uint8_t* buffer);
};

void SoftApInterface::AuthInd(AuthIndRequestView request, AuthIndCompleter::Sync& completer) {
  test_->OnAuthInd(request);
  completer.Reply();
}
void SoftApInterface::DeauthInd(DeauthIndRequestView request, DeauthIndCompleter::Sync& completer) {
  const auto deauth_ind = fidl::ToNatural(*request);
  test_->OnDeauthInd(&deauth_ind);
  completer.Reply();
}
void SoftApInterface::DeauthConf(DeauthConfRequestView request,
                                 DeauthConfCompleter::Sync& completer) {
  const auto deauth_conf = fidl::ToNatural(*request);
  test_->OnDeauthConf(&deauth_conf);
  completer.Reply();
}
void SoftApInterface::AssocInd(AssocIndRequestView request, AssocIndCompleter::Sync& completer) {
  const auto assoc_ind = fidl::ToNatural(*request);
  test_->OnAssocInd(&assoc_ind);
  completer.Reply();
}
void SoftApInterface::DisassocConf(DisassocConfRequestView request,
                                   DisassocConfCompleter::Sync& completer) {
  const auto disassoc_conf = fidl::ToNatural(*request);
  test_->OnDisassocConf(&disassoc_conf);
  completer.Reply();
}
void SoftApInterface::DisassocInd(DisassocIndRequestView request,
                                  DisassocIndCompleter::Sync& completer) {
  const auto disassoc_ind = fidl::ToNatural(*request);
  test_->OnDisassocInd(&disassoc_ind);
  completer.Reply();
}
void SoftApInterface::StartConf(StartConfRequestView request, StartConfCompleter::Sync& completer) {
  const auto start_conf = fidl::ToNatural(*request);
  test_->OnStartConf(&start_conf);
  completer.Reply();
}
void SoftApInterface::StopConf(StopConfRequestView request, StopConfCompleter::Sync& completer) {
  const auto stop_conf = fidl::ToNatural(*request);
  test_->OnStopConf(&stop_conf);
  completer.Reply();
}
void SoftApInterface::OnChannelSwitch(OnChannelSwitchRequestView request,
                                      OnChannelSwitchCompleter::Sync& completer) {
  const auto channel_switch = fidl::ToNatural(*request);
  test_->OnChannelSwitch(&channel_switch);
  completer.Reply();
}

void CreateSoftAPTest::Rx(std::shared_ptr<const simulation::SimFrame> frame,
                          std::shared_ptr<const simulation::WlanRxInfo> info) {
  ASSERT_EQ(frame->FrameType(), simulation::SimFrame::FRAME_TYPE_MGMT);

  auto mgmt_frame = std::static_pointer_cast<const simulation::SimManagementFrame>(frame);
  if (mgmt_frame->MgmtFrameType() == simulation::SimManagementFrame::FRAME_TYPE_AUTH) {
    auto auth_frame = std::static_pointer_cast<const simulation::SimAuthFrame>(mgmt_frame);
    if (auth_frame->seq_num_ == 2)
      auth_resp_status_ = auth_frame->status_;
  }
}

void CreateSoftAPTest::Init() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);
  softap_ifc_.test_ = this;
}

void CreateSoftAPTest::CreateInterface() {
  zx_status_t status;

  status = SimTest::StartInterface(wlan_common::WlanMacRole::kAp, &softap_ifc_);
  ASSERT_EQ(status, ZX_OK);
}

void CreateSoftAPTest::DeleteInterface() {
  EXPECT_EQ(SimTest::DeleteInterface(&softap_ifc_), ZX_OK);
}

void CreateSoftAPTest::GetApSetSsidErrInspectCount(uint64_t* out_count) {
  ASSERT_NOT_NULL(out_count);
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    auto hierarchy = FetchHierarchy(device->GetInspector());
    auto* root = hierarchy.value().GetByPath({"brcmfmac-phy"});
    ASSERT_NOT_NULL(root);
    // Only verify the value of hourly counter here, the relationship between hourly counter and
    // daily counter is verified in device_inspect_test.
    auto* uint_property = root->node().get_property<inspect::UintPropertyValue>("ap_set_ssid_err");
    ASSERT_NOT_NULL(uint_property);
    *out_count = uint_property->value();
  });
}

uint16_t CreateSoftAPTest::CreateRsneIe(uint8_t* buffer) {
  // construct a fake rsne ie in the input buffer
  uint16_t offset = 0;
  uint8_t* ie = buffer;

  ie[offset++] = WLAN_IE_TYPE_RSNE;
  ie[offset++] = 20;  // The length of following content.

  // These two bytes are 16-bit version number.
  ie[offset++] = 1;  // Lower byte
  ie[offset++] = 0;  // Higher byte

  memcpy(&ie[offset], RSN_OUI,
         TLV_OUI_LEN);  // RSN OUI for multicast cipher suite.
  offset += TLV_OUI_LEN;
  ie[offset++] = WPA_CIPHER_TKIP;  // Set multicast cipher suite.

  // These two bytes indicate the length of unicast cipher list, in this case is 1.
  ie[offset++] = 1;  // Lower byte
  ie[offset++] = 0;  // Higher byte

  memcpy(&ie[offset], RSN_OUI,
         TLV_OUI_LEN);  // RSN OUI for unicast cipher suite.
  offset += TLV_OUI_LEN;
  ie[offset++] = WPA_CIPHER_CCMP_128;  // Set unicast cipher suite.

  // These two bytes indicate the length of auth management suite list, in this case is 1.
  ie[offset++] = 1;  // Lower byte
  ie[offset++] = 0;  // Higher byte

  memcpy(&ie[offset], RSN_OUI,
         TLV_OUI_LEN);  // RSN OUI for auth management suite.
  offset += TLV_OUI_LEN;
  ie[offset++] = RSN_AKM_PSK;  // Set auth management suite.

  // These two bytes indicate RSN capabilities, in this case is \x0c\x00.
  ie[offset++] = 12;  // Lower byte
  ie[offset++] = 0;   // Higher byte

  // ASSERT_EQ(offset, (const uint32_t) (ie[TLV_LEN_OFF] + TLV_HDR_LEN));
  return offset;
}

zx_status_t CreateSoftAPTest::StartSoftAP() {
  const fuchsia_wlan_ieee80211::Ssid ssid = {'S', 'i', 'm', '_', 'A', 'P'};
  auto builder = wlan_fullmac_wire::WlanFullmacImplStartBssRequest::Builder(test_arena_)
                     .bss_type(fuchsia_wlan_ieee80211::wire::BssType::kInfrastructure)
                     .beacon_period(100)
                     .dtim_period(100)
                     .primary({.band = kDefaultBand, .number = kDefaultCh})
                     .ssid(ssid);

  // If sec mode is requested, create a dummy RSNE IE (our SoftAP only
  // supports WPA2)
  if (sec_enabled_) {
    uint8_t rsne_data[fuchsia_wlan_ieee80211::wire::kWlanIeBodyMaxLen];
    uint32_t rsne_len = CreateRsneIe(rsne_data);
    auto rsne = std::vector<uint8_t>(rsne_data, rsne_data + rsne_len);
    builder.rsne(fidl::VectorView<uint8_t>(test_arena_, rsne));
  }
  auto result = softap_ifc_.client_.buffer(softap_ifc_.test_arena_)->StartBss(builder.Build());
  EXPECT_TRUE(result.ok());

  // Retrieve wsec from SIM FW to check if it is set appropriately
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    uint32_t wsec;
    struct brcmf_if* ifp = brcmf_get_ifp(sim->drvr, softap_ifc_.iface_id_);
    zx_status_t status = brcmf_fil_iovar_int_get(ifp, "wsec", &wsec, nullptr);
    EXPECT_EQ(status, ZX_OK);
    if (sec_enabled_) {
      EXPECT_NE(wsec, (uint32_t)0);
    } else {
      EXPECT_EQ(wsec, (uint32_t)0);
    }
  });
  return ZX_OK;
}

void CreateSoftAPTest::InjectStartAPError() {
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjCmd(BRCMF_C_SET_SSID, ZX_ERR_IO, BCME_OK, softap_ifc_.iface_id_);
  });
}

void CreateSoftAPTest::InjectStopAPError() {
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjIovar("bss", ZX_ERR_IO, BCME_OK, softap_ifc_.iface_id_);
  });
}

void CreateSoftAPTest::InjectChanspecError() {
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjIovar("chanspec", ZX_ERR_IO, BCME_BADARG, softap_ifc_.iface_id_);
  });
}

void CreateSoftAPTest::InjectSetSsidError() {
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjCmd(BRCMF_C_SET_SSID, ZX_OK, BCME_ERROR, softap_ifc_.iface_id_);
  });
}

void CreateSoftAPTest::SetExpectMacForInds(MacAddr set_mac) { ind_expect_mac_ = set_mac; }

zx_status_t CreateSoftAPTest::StopSoftAP() {
  auto builder = wlan_fullmac_wire::WlanFullmacImplStopBssRequest::Builder(test_arena_);
  const fuchsia_wlan_ieee80211::Ssid ssid = {'S', 'i', 'm', '_', 'A', 'P'};
  builder.ssid(ssid);
  auto result = softap_ifc_.client_.buffer(softap_ifc_.test_arena_)->StopBss(builder.Build());
  EXPECT_TRUE(result.ok());
  return ZX_OK;
}

void CreateSoftAPTest::OnAuthInd(const wlan_fullmac_wire::WlanFullmacImplIfcAuthIndRequest* ind) {
  ASSERT_EQ(std::memcmp(ind->peer_sta_address().data(), ind_expect_mac_.byte, ETH_ALEN), 0);
  auth_ind_recv_ = true;
}
void CreateSoftAPTest::OnDeauthInd(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcDeauthIndRequest* ind) {
  ASSERT_TRUE(ind->peer_sta_address().has_value());
  ASSERT_BYTES_EQ(ind->peer_sta_address().value().data(), ind_expect_mac_.byte, ETH_ALEN);
  deauth_ind_recv_ = true;
}
void CreateSoftAPTest::OnDeauthConf(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcDeauthConfRequest* resp) {
  ASSERT_TRUE(resp->peer_sta_address().has_value());
  ASSERT_BYTES_EQ(resp->peer_sta_address().value().data(), ind_expect_mac_.byte, ETH_ALEN);
  deauth_conf_recv_ = true;
}
void CreateSoftAPTest::OnAssocInd(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcAssocIndRequest* ind) {
  ASSERT_TRUE(ind->peer_sta_address().has_value());
  ASSERT_EQ(std::memcmp(ind->peer_sta_address()->data(), ind_expect_mac_.byte, ETH_ALEN), 0);
  assoc_ind_recv_ = true;
}
void CreateSoftAPTest::OnDisassocConf(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcDisassocConfRequest* resp) {
  ASSERT_TRUE(resp->status().has_value());
  disassoc_conf_recv_ = true;
}
void CreateSoftAPTest::OnDisassocInd(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcDisassocIndRequest* ind) {
  ASSERT_EQ(std::memcmp(ind->peer_sta_address()->data(), ind_expect_mac_.byte, ETH_ALEN), 0);
  disassoc_ind_recv_ = true;
}

void CreateSoftAPTest::OnStartConf(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcStartConfRequest* resp) {
  start_conf_received_ = true;
  start_conf_status_ = resp->result_code().value();
}

void CreateSoftAPTest::OnStopConf(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcStopConfRequest* resp) {
  stop_conf_received_ = true;
  stop_conf_status_ = resp->result_code().value();
}

void CreateSoftAPTest::OnChannelSwitch(
    const fuchsia_wlan_fullmac::WlanFullmacImplIfcOnChannelSwitchRequest* ind) {}

void CreateSoftAPTest::TxAssocReq(MacAddr client_mac) {
  // Get the mac address of the SoftAP
  MacAddr soft_ap_mac;
  softap_ifc_.GetMacAddr(&soft_ap_mac);
  simulation::SimAssocReqFrame assoc_req_frame(client_mac, soft_ap_mac, kDefaultSsid);
  env_->Tx(assoc_req_frame, tx_info_, this);
}

void CreateSoftAPTest::TxAuthReq(simulation::SimAuthType auth_type, MacAddr client_mac) {
  // Get the mac address of the SoftAP
  MacAddr soft_ap_mac;
  softap_ifc_.GetMacAddr(&soft_ap_mac);
  simulation::SimAuthFrame auth_req_frame(client_mac, soft_ap_mac, 1, auth_type,
                                          wlan_ieee80211::StatusCode::kSuccess);
  env_->Tx(auth_req_frame, tx_info_, this);
}

void CreateSoftAPTest::TxDisassocReq(MacAddr client_mac) {
  // Get the mac address of the SoftAP
  MacAddr soft_ap_mac;
  softap_ifc_.GetMacAddr(&soft_ap_mac);
  // Disassociate with the SoftAP
  simulation::SimDisassocReqFrame disassoc_req_frame(
      client_mac, soft_ap_mac, wlan_ieee80211::ReasonCode::kLeavingNetworkDisassoc);
  env_->Tx(disassoc_req_frame, tx_info_, this);
}

void CreateSoftAPTest::TxDeauthReq(MacAddr client_mac) {
  // Get the mac address of the SoftAP
  MacAddr soft_ap_mac;
  softap_ifc_.GetMacAddr(&soft_ap_mac);
  // Disassociate with the SoftAP
  simulation::SimDeauthFrame deauth_frame(client_mac, soft_ap_mac,
                                          wlan_ieee80211::ReasonCode::kLeavingNetworkDeauth);
  env_->Tx(deauth_frame, tx_info_, this);
}

void CreateSoftAPTest::DeauthClient(MacAddr client_mac) {
  auto builder =
      fuchsia_wlan_fullmac::wire::WlanFullmacImplDeauthRequest::Builder(softap_ifc_.test_arena_);

  ::fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
  std::memcpy(peer_sta_address.data(), client_mac.byte, ETH_ALEN);
  builder.peer_sta_address(peer_sta_address);
  builder.reason_code(fuchsia_wlan_ieee80211::ReasonCode::kUnspecifiedReason);

  auto result = softap_ifc_.client_.buffer(softap_ifc_.test_arena_)->Deauth(builder.Build());
  EXPECT_TRUE(result.ok());
}

void CreateSoftAPTest::VerifyAuth() {
  ASSERT_EQ(auth_ind_recv_, true);
  // When auth is done, the client is already added into client list.
  VerifyNumOfClient(1);
}

void CreateSoftAPTest::VerifyAssoc() {
  // Verify the event indications were received and
  // the number of clients
  ASSERT_EQ(auth_ind_recv_, true);
  ASSERT_EQ(assoc_ind_recv_, true);
  VerifyNumOfClient(1);
}

void CreateSoftAPTest::ClearAssocInd() { assoc_ind_recv_ = false; }

void CreateSoftAPTest::VerifyNumOfClient(uint16_t expect_client_num) {
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    uint16_t num_clients = sim->sim_fw->GetNumClients(softap_ifc_.iface_id_);
    ASSERT_EQ(num_clients, expect_client_num);
  });
}

void CreateSoftAPTest::VerifyNotAssoc() {
  ASSERT_EQ(assoc_ind_recv_, false);
  ASSERT_EQ(auth_ind_recv_, false);
  VerifyNumOfClient(0);
}

void CreateSoftAPTest::VerifyStartAPConf(wlan_fullmac_wire::StartResult status) {
  ASSERT_EQ(start_conf_received_, true);
  ASSERT_EQ(start_conf_status_, status);
}

void CreateSoftAPTest::VerifyStopAPConf(wlan_fullmac_wire::StopResult status) {
  ASSERT_EQ(stop_conf_received_, true);
  ASSERT_EQ(stop_conf_status_, status);
}

TEST_F(CreateSoftAPTest, SetDefault) { DeleteInterface(); }

TEST_F(CreateSoftAPTest, CreateSoftAP) {
  zx::duration delay = zx::msec(10);
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::StartSoftAP, this), delay);
  delay += kStartAPLinkEventDelay + kApStartedEventDelay + zx::msec(10);
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::StopSoftAP, this), delay);
  env_->Run(kSimulatedClockDuration);
  VerifyStartAPConf(wlan_fullmac_wire::StartResult::kSuccess);
}

TEST_F(CreateSoftAPTest, CreateSoftAPFail) {
  InjectStartAPError();
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::StartSoftAP, this), zx::msec(50));
  env_->Run(kSimulatedClockDuration);
  VerifyStartAPConf(wlan_fullmac_wire::StartResult::kNotSupported);
}

TEST_F(CreateSoftAPTest, CreateSoftAPMissingParams) {
  // Create the Start BSS request without the SSID.
  auto builder = wlan_fullmac_wire::WlanFullmacImplStartBssRequest::Builder(test_arena_)
                     .bss_type(fuchsia_wlan_ieee80211::wire::BssType::kInfrastructure)
                     .beacon_period(100)
                     .dtim_period(100)
                     .primary({.band = kDefaultBand, .number = kDefaultCh});

  auto result = softap_ifc_.client_.buffer(softap_ifc_.test_arena_)->StartBss(builder.Build());
  EXPECT_TRUE(result.ok());
  // Should have received a StartConf with kNotSupported result.
  VerifyStartAPConf(wlan_fullmac_wire::StartResult::kNotSupported);
}

TEST_F(CreateSoftAPTest, CreateSoftAPFail_ChanSetError) {
  InjectChanspecError();
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::StartSoftAP, this), zx::msec(50));
  env_->Run(kSimulatedClockDuration);
  VerifyStartAPConf(wlan_fullmac_wire::StartResult::kNotSupported);
}

// SoftAP can encounter this specific SET_SSID firmware error, which we detect and log.
TEST_F(CreateSoftAPTest, CreateSoftAPFail_SetSsidError) {
  InjectSetSsidError();
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::StartSoftAP, this), zx::msec(50));
  uint64_t count;
  GetApSetSsidErrInspectCount(&count);
  ASSERT_EQ(count, 0u);
  env_->Run(kSimulatedClockDuration);

  VerifyStartAPConf(wlan_fullmac_wire::StartResult::kNotSupported);

  // Verify inspect is updated.
  GetApSetSsidErrInspectCount(&count);
  EXPECT_EQ(count, 1u);
}

// Fail the iovar bss but Stop AP should still succeed
TEST_F(CreateSoftAPTest, BssIovarFail) {
  InjectStopAPError();
  // Start SoftAP
  StartSoftAP();
  StopSoftAP();
  VerifyStopAPConf(wlan_fullmac_wire::StopResult::kSuccess);
}

TEST_F(CreateSoftAPTest, BssStopMissingParam) {
  // Start SoftAP
  StartSoftAP();
  // Create the Stop BSS request without the SSID.
  auto builder = wlan_fullmac_wire::WlanFullmacImplStopBssRequest::Builder(test_arena_);

  auto result = softap_ifc_.client_.buffer(softap_ifc_.test_arena_)->StopBss(builder.Build());
  EXPECT_TRUE(result.ok());
  // Should have received a StartConf with kNotSupported result.
  VerifyStopAPConf(wlan_fullmac_wire::StopResult::kInternalError);
}

// Start SoftAP in secure mode and then restart in open mode.
// Appropriate secure mode is checked in StartSoftAP() after SoftAP
// is started
TEST_F(CreateSoftAPTest, CreateSecureSoftAP) {
  // Start SoftAP in secure mode
  sec_enabled_ = true;
  StartSoftAP();
  StopSoftAP();
  // Restart SoftAP in open mode
  StartSoftAP();
  StopSoftAP();
}

TEST_F(CreateSoftAPTest, AssociateWithSoftAP) {
  StartSoftAP();
  env_->ScheduleNotification(
      std::bind(&CreateSoftAPTest::TxAuthReq, this, simulation::AUTH_TYPE_OPEN, kFakeMac),
      zx::msec(5));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAuth, this), zx::msec(8));

  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxAssocReq, this, kFakeMac),
                             zx::msec(10));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAssoc, this), zx::msec(50));
  env_->Run(kSimulatedClockDuration);
}

TEST_F(CreateSoftAPTest, DisassociateThenAssociateWithSoftAP) {
  StartSoftAP();
  env_->ScheduleNotification(
      std::bind(&CreateSoftAPTest::TxAuthReq, this, simulation::AUTH_TYPE_OPEN, kFakeMac),
      zx::msec(5));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAuth, this), zx::msec(8));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxAssocReq, this, kFakeMac),
                             zx::msec(10));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAssoc, this), zx::msec(50));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::ClearAssocInd, this), zx::msec(75));
  // Assoc a second time
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxAssocReq, this, kFakeMac),
                             zx::msec(100));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAssoc, this), zx::msec(150));
  env_->Run(kSimulatedClockDuration);
}

TEST_F(CreateSoftAPTest, DisassociateFromSoftAP) {
  StartSoftAP();
  env_->ScheduleNotification(
      std::bind(&CreateSoftAPTest::TxAuthReq, this, simulation::AUTH_TYPE_OPEN, kFakeMac),
      zx::msec(5));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAuth, this), zx::msec(8));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxAssocReq, this, kFakeMac),
                             zx::msec(10));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAssoc, this), zx::msec(50));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxDisassocReq, this, kFakeMac),
                             zx::msec(60));
  env_->Run(kSimulatedClockDuration);
  // Only disassoc ind should be seen.
  EXPECT_EQ(deauth_ind_recv_, false);
  EXPECT_EQ(disassoc_ind_recv_, true);
  VerifyNumOfClient(0);
}

// After a client associates, deauth it from the SoftAP itself.
TEST_F(CreateSoftAPTest, DisassociateClientFromSoftAP) {
  StartSoftAP();
  env_->ScheduleNotification(
      std::bind(&CreateSoftAPTest::TxAuthReq, this, simulation::AUTH_TYPE_OPEN, kFakeMac),
      zx::msec(5));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAuth, this), zx::msec(8));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxAssocReq, this, kFakeMac),
                             zx::msec(10));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAssoc, this), zx::msec(50));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::DeauthClient, this, kFakeMac),
                             zx::msec(60));
  env_->Run(kSimulatedClockDuration);
  // Should have received disassoc conf.
  EXPECT_EQ(disassoc_conf_recv_, true);
  VerifyNumOfClient(0);
}

TEST_F(CreateSoftAPTest, AssocWithWrongAuth) {
  StartSoftAP();
  env_->ScheduleNotification(
      std::bind(&CreateSoftAPTest::TxAuthReq, this, simulation::AUTH_TYPE_SHARED_KEY, kFakeMac),
      zx::msec(5));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxAssocReq, this, kFakeMac),
                             zx::msec(10));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyNotAssoc, this), zx::msec(20));
  env_->Run(kSimulatedClockDuration);
  EXPECT_EQ(auth_resp_status_, wlan_ieee80211::StatusCode::kRefusedReasonUnspecified);
}

TEST_F(CreateSoftAPTest, DeauthBeforeAssoc) {
  StartSoftAP();
  env_->ScheduleNotification(
      std::bind(&CreateSoftAPTest::TxAuthReq, this, simulation::AUTH_TYPE_OPEN, kFakeMac),
      zx::msec(5));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAuth, this), zx::msec(10));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxDeauthReq, this, kFakeMac),
                             zx::msec(20));
  env_->Run(kSimulatedClockDuration);
  // Only deauth ind shoulb be seen.
  EXPECT_EQ(deauth_ind_recv_, true);
  EXPECT_EQ(disassoc_ind_recv_, false);
  VerifyNumOfClient(0);
}

TEST_F(CreateSoftAPTest, DeauthWhileAssociated) {
  StartSoftAP();
  env_->ScheduleNotification(
      std::bind(&CreateSoftAPTest::TxAuthReq, this, simulation::AUTH_TYPE_OPEN, kFakeMac),
      zx::msec(5));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAuth, this), zx::msec(8));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxAssocReq, this, kFakeMac),
                             zx::msec(10));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyAssoc, this), zx::msec(50));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxDeauthReq, this, kFakeMac),
                             zx::msec(60));
  env_->Run(kSimulatedClockDuration);
  EXPECT_EQ(deauth_ind_recv_, true);
  VerifyNumOfClient(0);
}

const MacAddr kSecondClientMac({0xde, 0xad, 0xbe, 0xef, 0x00, 0x04});

TEST_F(CreateSoftAPTest, DeauthMultiClients) {
  StartSoftAP();
  env_->ScheduleNotification(
      std::bind(&CreateSoftAPTest::TxAuthReq, this, simulation::AUTH_TYPE_OPEN, kFakeMac),
      zx::msec(5));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxAssocReq, this, kFakeMac),
                             zx::msec(10));
  env_->ScheduleNotification(
      std::bind(&CreateSoftAPTest::SetExpectMacForInds, this, kSecondClientMac), zx::msec(15));
  env_->ScheduleNotification(
      std::bind(&CreateSoftAPTest::TxAuthReq, this, simulation::AUTH_TYPE_OPEN, kSecondClientMac),
      zx::msec(20));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxAssocReq, this, kSecondClientMac),
                             zx::msec(30));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyNumOfClient, this, 2),
                             zx::msec(40));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::SetExpectMacForInds, this, kFakeMac),
                             zx::msec(45));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::TxDeauthReq, this, kFakeMac),
                             zx::msec(50));
  env_->ScheduleNotification(std::bind(&CreateSoftAPTest::VerifyNumOfClient, this, 1),
                             zx::msec(60));
  env_->Run(kSimulatedClockDuration);
}

}  // namespace wlan::brcmfmac
