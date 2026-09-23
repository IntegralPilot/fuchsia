// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

#include <fidl/fuchsia.wlan.common/cpp/wire_types.h>
#include <fidl/fuchsia.wlan.fullmac/cpp/markers.h>
#include <fidl/fuchsia.wlan.phy/cpp/markers.h>
#include <fuchsia/wlan/ieee80211/cpp/fidl.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/internal/test_environment.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/channel.h>

#include <bind/fuchsia/cpp/bind.h>
#include <fbl/string_buffer.h>
#include <wlan/common/channel.h>
#include <wlan/drivers/macaddr.h>

#include "fidl/fuchsia.wlan.fullmac/cpp/wire_types.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {

fuchsia_wlan_ieee80211::Ssid Ssid(const void* ssid_data, uint8_t len) {
  fuchsia_wlan_ieee80211::Ssid ssid(len, 0);
  memcpy(ssid.data(), ssid_data, len);
  return ssid;
}
// static
const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber> SimInterface::kDefaultScanChannels =
    {
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
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 32},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 36},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 40},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 44},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 48},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 52},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 56},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 60},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 64},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 100},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 104},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 108},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 112},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 116},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 120},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 124},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 128},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 132},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 136},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 140},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 144},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 149},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 153},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 157},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 161},
        {.band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 165},
};

SimInterface::SimInterface() : test_arena_(fdf::Arena('IFAC')) {}

SimInterface::~SimInterface() {
  if (server_binding_ != nullptr) {
    Reset();
  }
}

zx_status_t SimInterface::Init(simulation::Environment* env, wlan_common::WlanMacRole role) {
  zx_status_t result = zx::channel::create(0, &ch_sme_, &ch_mlme_);
  if (result == ZX_OK) {
    env_ = env;
    role_ = role;
  }
  return result;
}

void SimInterface::Reset() {
  libsync::Completion destroy_binding_completion;
  async::PostTask(server_dispatcher_, [&]() {
    server_binding_.reset();
    destroy_binding_completion.Signal();
  });

  destroy_binding_completion.Wait();
  if (client_.is_valid()) {
    client_.TakeClientEnd();
  }
}

zx_status_t SimInterface::Connect(fidl::ClientEnd<fuchsia_wlan_fullmac::WlanFullmacImpl> client_end,
                                  async_dispatcher_t* server_dispatcher) {
  fidl::WireSyncClient<fuchsia_wlan_fullmac::WlanFullmacImpl> client(std::move(client_end));

  // Establish the FIDL connection on the oppsite direction.
  auto endpoints = fidl::CreateEndpoints<fuchsia_wlan_fullmac::WlanFullmacImplIfc>();
  if (endpoints.is_error()) {
    BRCMF_ERR("Failed to create endpoints: %s", endpoints.status_string());
    return endpoints.status_value();
  }

  // Synchronously bind the server to the given dispatcher.
  server_dispatcher_ = server_dispatcher;
  libsync::Completion create_binding_completion;
  async::PostTask(server_dispatcher, [&, server_end = std::move(endpoints->server)]() mutable {
    server_binding_ =
        std::make_unique<fidl::ServerBinding<fuchsia_wlan_fullmac::WlanFullmacImplIfc>>(
            server_dispatcher_, std::move(server_end), this, fidl::kIgnoreBindingClosure);
    create_binding_completion.Signal();
  });

  create_binding_completion.Wait();

  auto req = fuchsia_wlan_fullmac::wire::WlanFullmacImplInitRequest::Builder(test_arena_)
                 .ifc(std::move(endpoints->client))
                 .Build();
  auto result = client.buffer(test_arena_)->Init(req);
  if (!result.ok()) {
    BRCMF_ERR("Failed to start wlanfullmac interface: %s", result.FormatDescription().c_str());
    return result.status();
  }

  if (result->is_error()) {
    BRCMF_ERR("Start failed: %s", zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  // Only assign the client if Start succeeded, otherwise client_ is assigned but not working.
  client_ = std::move(client);

  return ZX_OK;
}

void SimInterface::OnScanResult(OnScanResultRequestView request,
                                OnScanResultCompleter::Sync& completer) {
  auto results = scan_results_.find(request->txn_id());

  // Verify that we started a scan on this interface
  ZX_ASSERT(results != scan_results_.end());

  // Verify that the scan hasn't sent a completion notice
  ZX_ASSERT(!results->second.result_code);

  fuchsia_wlan_fullmac::WlanFullmacImplIfcOnScanResultRequest copy = fidl::ToNatural(*request);
  // Copy the IES data over since the original location may change data by the time we verify.
  std::vector<uint8_t> ies(copy.bss()->ies().data(),
                           copy.bss()->ies().data() + copy.bss()->ies().size());
  scan_results_ies_.push_back(ies);
  copy.bss()->ies() = std::vector<uint8_t>(*scan_results_ies_.rbegin());
  results->second.result_list.push_back(copy);
  completer.Reply();
}

void SimInterface::OnScanEnd(OnScanEndRequestView request, OnScanEndCompleter::Sync& completer) {
  auto& end = request;
  auto results = scan_results_.find(end->txn_id());

  // Verify that we started a scan on this interface
  ZX_ASSERT(results != scan_results_.end());

  // Verify that the scan hasn't already received a completion notice
  ZX_ASSERT(!results->second.result_code);

  results->second.result_code = end->code();
  completer.Reply();
}

void SimInterface::OnScheduledScanMatchesAvailable(
    OnScheduledScanMatchesAvailableRequestView request,
    OnScheduledScanMatchesAvailableCompleter::Sync& completer) {}

void SimInterface::OnScheduledScanStoppedByFirmware(
    OnScheduledScanStoppedByFirmwareRequestView request,
    OnScheduledScanStoppedByFirmwareCompleter::Sync& completer) {}

void SimInterface::ConnectConf(ConnectConfRequestView request,
                               ConnectConfCompleter::Sync& completer) {
  ZX_ASSERT(assoc_ctx_.state == AssocContext::kAssociating);
  auto connect_conf = fidl::ToNatural(*request);
  stats_.connect_results.push_back(connect_conf);

  if (request->result_code() == wlan_ieee80211::StatusCode::kSuccess) {
    assoc_ctx_.state = AssocContext::kAssociated;
    stats_.connect_successes++;
  } else {
    assoc_ctx_.state = AssocContext::kNone;
  }
  completer.Reply();
}

void SimInterface::RoamConf(RoamConfRequestView request, RoamConfCompleter::Sync& completer) {
  ZX_ASSERT(assoc_ctx_.state == AssocContext::kAssociated);
  ZX_ASSERT(request->has_status_code());
  const auto roam_conf = fidl::ToNatural(*request);
  stats_.roam_confirmations.push_back(roam_conf);

  if (request->status_code() == wlan_ieee80211::StatusCode::kSuccess) {
    ++stats_.roam_successes;
    ZX_ASSERT(request->has_selected_bssid());
    auto bssid = request->selected_bssid();
    ZX_ASSERT(bssid.size() == ETH_ALEN);
    memcpy(assoc_ctx_.bssid.byte, bssid.data(), ETH_ALEN);
  } else {
    assoc_ctx_.state = AssocContext::kNone;
  }
  completer.Reply();
}

void SimInterface::RoamStartInd(RoamStartIndRequestView request,
                                RoamStartIndCompleter::Sync& completer) {
  ZX_ASSERT(assoc_ctx_.state == AssocContext::kAssociated);
  completer.Reply();
}

void SimInterface::RoamResultInd(RoamResultIndRequestView request,
                                 RoamResultIndCompleter::Sync& completer) {
  ZX_ASSERT(assoc_ctx_.state == AssocContext::kAssociated);
  ZX_ASSERT(request->has_status_code());
  const auto roam_result = fidl::ToNatural(*request);
  stats_.roam_result_indications.push_back(roam_result);
  if (request->status_code() == wlan_ieee80211::StatusCode::kSuccess) {
    stats_.connect_successes++;
    ZX_ASSERT(request->has_selected_bssid());
    auto bssid = request->selected_bssid();
    ZX_ASSERT(bssid.size() == ETH_ALEN);
    memcpy(assoc_ctx_.bssid.byte, bssid.data(), ETH_ALEN);
  } else {
    assoc_ctx_.state = AssocContext::kNone;
  }
  completer.Reply();
}

void SimInterface::AuthInd(AuthIndRequestView request, AuthIndCompleter::Sync& completer) {
  ZX_ASSERT(role_ == wlan_common::WlanMacRole::kAp);
  auto auth_ind = fidl::ToNatural(*request);
  stats_.auth_indications.push_back(auth_ind);
  completer.Reply();
}

void SimInterface::DeauthConf(DeauthConfRequestView request, DeauthConfCompleter::Sync& completer) {
  const auto deauth_conf = fidl::ToNatural(*request);
  if (deauth_conf.peer_sta_address().has_value()) {
    if (memcmp(assoc_ctx_.bssid.byte, deauth_conf.peer_sta_address()->data(), ETH_ALEN) == 0) {
      assoc_ctx_.state = AssocContext::kNone;
    }
  }
  stats_.deauth_results.emplace_back(deauth_conf);
  completer.Reply();
}

void SimInterface::DeauthInd(DeauthIndRequestView request, DeauthIndCompleter::Sync& completer) {
  auto deauth_ind = fidl::ToNatural(*request);
  stats_.deauth_indications.push_back(deauth_ind);
  const auto& peer_sta_address = request->peer_sta_address().data();
  if (memcmp(assoc_ctx_.bssid.byte, peer_sta_address, ETH_ALEN) == 0) {
    assoc_ctx_.state = AssocContext::kNone;
  }

  completer.Reply();
}

void SimInterface::AssocInd(AssocIndRequestView request, AssocIndCompleter::Sync& completer) {
  ZX_ASSERT(role_ == wlan_common::WlanMacRole::kAp);
  auto assoc_ind = fidl::ToNatural(*request);
  stats_.assoc_indications.push_back(assoc_ind);
  completer.Reply();
}

void SimInterface::DisassocConf(DisassocConfRequestView request,
                                DisassocConfCompleter::Sync& completer) {
  stats_.disassoc_results.emplace_back(fidl::ToNatural(*request));
  assoc_ctx_.state = AssocContext::kNone;
  completer.Reply();
}

void SimInterface::DisassocInd(DisassocIndRequestView request,
                               DisassocIndCompleter::Sync& completer) {
  stats_.disassoc_indications.push_back(fidl::ToNatural(*request));
  assoc_ctx_.state = AssocContext::kNone;
  completer.Reply();
}

void SimInterface::StartConf(StartConfRequestView request, StartConfCompleter::Sync& completer) {
  stats_.start_confirmations.push_back(fidl::ToNatural(*request));
  completer.Reply();
}

void SimInterface::StopConf(StopConfRequestView request, StopConfCompleter::Sync& completer) {
  stats_.stop_confirmations.push_back(fidl::ToNatural(*request));
  completer.Reply();
}

void SimInterface::EapolConf(EapolConfRequestView request, EapolConfCompleter::Sync& completer) {
  completer.Reply();
}

void SimInterface::OnChannelSwitch(OnChannelSwitchRequestView request,
                                   OnChannelSwitchCompleter::Sync& completer) {
  stats_.csa_indications.push_back(request->ind);
  completer.Reply();
}

void SimInterface::SignalReport(SignalReportRequestView request,
                                SignalReportCompleter::Sync& completer) {
  completer.Reply();
}

void SimInterface::EapolInd(EapolIndRequestView request, EapolIndCompleter::Sync& completer) {
  completer.Reply();
}

void SimInterface::OnPmkAvailable(OnPmkAvailableRequestView request,
                                  OnPmkAvailableCompleter::Sync& completer) {
  completer.Reply();
}

void SimInterface::SaeHandshakeInd(SaeHandshakeIndRequestView request,
                                   SaeHandshakeIndCompleter::Sync& completer) {
  completer.Reply();
}

void SimInterface::SaeFrameRx(SaeFrameRxRequestView request, SaeFrameRxCompleter::Sync& completer) {
  completer.Reply();
}

void SimInterface::OnWmmStatusResp(OnWmmStatusRespRequestView request,
                                   OnWmmStatusRespCompleter::Sync& completer) {
  completer.Reply();
}

fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse SimInterface::Query() {
  auto result = client_.buffer(test_arena_)->Query();
  ZX_ASSERT(result.ok());
  ZX_ASSERT(!result->is_error());
  return fidl::ToNatural(*result->value());
}

void SimInterface::QuerySecuritySupport(wlan_common::SecuritySupport* out_resp) {
  auto result = client_.buffer(test_arena_)->QuerySecuritySupport();
  ZX_ASSERT(result.ok());
  ZX_ASSERT(!result->is_error());

  *out_resp = result->value()->resp();
}

void SimInterface::QuerySpectrumManagementSupport(
    wlan_common::SpectrumManagementSupport* out_resp) {
  auto result = client_.buffer(test_arena_)->QuerySpectrumManagementSupport();
  ZX_ASSERT(result.ok());
  ZX_ASSERT(!result->is_error());

  *out_resp = result->value()->resp();
}

void SimInterface::QueryTelemetrySupport(fuchsia_wlan_stats::wire::TelemetrySupport* out_resp) {
  auto result = client_.buffer(test_arena_)->QueryTelemetrySupport();
  ZX_ASSERT(result.ok());
  ZX_ASSERT(!result->is_error());

  *out_resp = result->value()->resp();
}

void SimInterface::GetMacAddr(MacAddr* out_macaddr) {
  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse info = Query();
  ZX_ASSERT(info.sta_addr().has_value());
  memcpy(out_macaddr->byte, info.sta_addr()->data(), ETH_ALEN);
}

void SimInterface::StartConnect(const MacAddr& bssid, const fuchsia_wlan_ieee80211::Ssid& ssid,
                                const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel,
                                fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw,
                                const fuchsia_wlan_ieee80211::wire::ChannelNumber& secondary80) {
  // This should only be performed on a Client interface
  ZX_ASSERT(role_ == wlan_common::WlanMacRole::kClient);

  stats_.connect_attempts++;

  // Save off context
  assoc_ctx_.state = AssocContext::kAssociating;
  assoc_ctx_.bssid = bssid;

  assoc_ctx_.ies.clear();
  assoc_ctx_.ies.push_back(0);            // SSID IE type ID
  assoc_ctx_.ies.push_back(ssid.size());  // SSID IE length
  assoc_ctx_.ies.insert(assoc_ctx_.ies.end(), ssid.data(), ssid.data() + ssid.size());
  assoc_ctx_.primary = channel;

  // Send connect request
  auto builder = wlan_fullmac_wire::WlanFullmacImplConnectRequest::Builder(test_arena_);
  fuchsia_wlan_ieee80211::wire::BssDescription bss;
  memcpy(bss.bssid.data(), bssid.byte, ETH_ALEN);
  auto ies =
      std::vector<uint8_t>(assoc_ctx_.ies.data(), assoc_ctx_.ies.data() + assoc_ctx_.ies.size());
  bss.ies = fidl::VectorView(test_arena_, ies);
  bss.primary = channel;
  bss.bandwidth = cbw;
  bss.vht_secondary_80_channel = secondary80;
  bss.bss_type = fuchsia_wlan_ieee80211::wire::BssType::kInfrastructure;
  builder.selected_bss(bss);
  builder.auth_type(wlan_fullmac_wire::WlanAuthType::kOpenSystem);
  builder.connect_failure_timeout(1000);  // ~1s (although value is ignored for now)

  auto result = client_.buffer(test_arena_)->Connect(builder.Build());
  ZX_ASSERT(result.ok());
}

void SimInterface::AssociateWith(const simulation::FakeAp& ap, std::optional<zx::duration> delay) {
  // This should only be performed on a Client interface
  ZX_ASSERT(role_ == wlan_common::WlanMacRole::kClient);

  MacAddr bssid = ap.GetBssid();
  fuchsia_wlan_ieee80211::Ssid ssid = ap.GetSsid();
  fuchsia_wlan_ieee80211::wire::ChannelNumber channel = ap.GetChannel();
  fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw = ap.GetChannelBandwidth();
  auto secondary80 = ap.GetSecondary80();

  if (delay) {
    env_->ScheduleNotification(
        std::bind(&SimInterface::StartConnect, this, bssid, ssid, channel, cbw, secondary80),
        *delay);
  } else {
    StartConnect(ap.GetBssid(), ap.GetSsid(), channel, cbw, secondary80);
  }
}

void SimInterface::StartRoam(const MacAddr& bssid,
                             const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel,
                             fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw,
                             const fuchsia_wlan_ieee80211::wire::ChannelNumber& secondary80) {
  // This should only be performed on a Client interface
  ZX_ASSERT(role_ == wlan_common::WlanMacRole::kClient);
  ++stats_.roam_attempts;

  // Send roam request
  auto builder = wlan_fullmac_wire::WlanFullmacImplRoamRequest::Builder(test_arena_);
  fuchsia_wlan_ieee80211::wire::BssDescription bss;
  memcpy(bss.bssid.data(), bssid.byte, ETH_ALEN);
  auto ies =
      std::vector<uint8_t>(assoc_ctx_.ies.data(), assoc_ctx_.ies.data() + assoc_ctx_.ies.size());
  bss.ies = fidl::VectorView(test_arena_, ies);
  bss.primary = channel;
  bss.bandwidth = cbw;
  bss.vht_secondary_80_channel = secondary80;
  bss.bss_type = fuchsia_wlan_ieee80211::wire::BssType::kInfrastructure;
  builder.selected_bss(bss);
  auto result = client_.buffer(test_arena_)->Roam(builder.Build());
  ZX_ASSERT(result.ok());
}

void SimInterface::DisassociateFrom(const MacAddr& bssid, wlan_ieee80211::ReasonCode reason) {
  // This should only be performed on a Client interface
  ZX_ASSERT(role_ == wlan_common::WlanMacRole::kClient);

  auto builder = wlan_fullmac_wire::WlanFullmacImplDisassocRequest::Builder(test_arena_);
  ::fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
  std::memcpy(peer_sta_address.data(), bssid.byte, ETH_ALEN);
  builder.peer_sta_address(peer_sta_address);
  builder.reason_code(reason);

  auto result = client_.buffer(test_arena_)->Disassoc(builder.Build());
  ZX_ASSERT(result.ok());
}

void SimInterface::DeauthenticateFrom(const MacAddr& bssid, wlan_ieee80211::ReasonCode reason) {
  // This should only be performed on a Client interface
  ZX_ASSERT(role_ == wlan_common::WlanMacRole::kClient);

  auto builder = wlan_fullmac_wire::WlanFullmacImplDeauthRequest::Builder(test_arena_);
  ::fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
  std::memcpy(peer_sta_address.data(), bssid.byte, ETH_ALEN);
  builder.peer_sta_address(peer_sta_address);
  builder.reason_code(reason);

  auto result = client_.buffer(test_arena_)->Deauth(builder.Build());
  ZX_ASSERT(result.ok());
}

void SimInterface::StartScan(
    uint64_t txn_id, bool active,
    std::optional<const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber>> channels_arg) {
  wlan_fullmac_wire::WlanScanType scan_type =
      active ? wlan_fullmac_wire::WlanScanType::kActive : wlan_fullmac_wire::WlanScanType::kPassive;
  uint32_t dwell_time = active ? kDefaultActiveScanDwellTimeMs : kDefaultPassiveScanDwellTimeMs;
  const std::vector<fuchsia_wlan_ieee80211::wire::ChannelNumber> channels =
      channels_arg.has_value() ? channels_arg.value() : kDefaultScanChannels;

  auto builder = wlan_fullmac_wire::WlanFullmacImplStartScanRequest::Builder(test_arena_);

  builder.txn_id(txn_id);
  builder.scan_type(scan_type);
  builder.channels(
      fidl::VectorView<fuchsia_wlan_ieee80211::wire::ChannelNumber>(test_arena_, channels));
  builder.min_channel_time(dwell_time);
  builder.max_channel_time(dwell_time);

  // Create an entry for tracking results
  ScanStatus scan_status;
  scan_results_.insert_or_assign(txn_id, scan_status);

  // Start the scan
  auto result = client_.buffer(test_arena_)->StartScan(builder.Build());
  ZX_ASSERT(result.ok());
}

std::optional<wlan_fullmac_wire::WlanScanResult> SimInterface::ScanResultCode(uint64_t txn_id) {
  auto results = scan_results_.find(txn_id);

  // Verify that we started a scan on this interface
  ZX_ASSERT(results != scan_results_.end());

  return results->second.result_code;
}

const std::list<fuchsia_wlan_fullmac::WlanFullmacImplIfcOnScanResultRequest>*
SimInterface::ScanResultList(uint64_t txn_id) {
  auto results = scan_results_.find(txn_id);

  // Verify that we started a scan on this interface
  ZX_ASSERT(results != scan_results_.end());

  return &results->second.result_list;
}

void SimInterface::StartSoftAp(const fuchsia_wlan_ieee80211::Ssid& ssid,
                               const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel,
                               uint32_t beacon_period, uint32_t dtim_period) {
  // This should only be performed on an AP interface
  ZX_ASSERT(role_ == wlan_common::WlanMacRole::kAp);

  auto builder = wlan_fullmac_wire::WlanFullmacImplStartBssRequest::Builder(test_arena_)
                     .bss_type(fuchsia_wlan_ieee80211::wire::BssType::kInfrastructure)
                     .beacon_period(beacon_period)
                     .dtim_period(dtim_period)
                     .primary(channel)
                     .ssid(ssid);

  // Send request to driver
  auto result = client_.buffer(test_arena_)->StartBss(builder.Build());
  ZX_ASSERT(result.ok());

  // // Remember context
  soft_ap_ctx_.ssid = ssid;

  // Return value is handled asynchronously in OnStartConf
}

void SimInterface::StopSoftAp() {
  // This should only be performed on an AP interface
  ZX_ASSERT(role_ == wlan_common::WlanMacRole::kAp);

  auto builder = wlan_fullmac_wire::WlanFullmacImplStopBssRequest::Builder(test_arena_);
  // Use the ssid from the last call to StartSoftAp
  builder.ssid(soft_ap_ctx_.ssid);

  ZX_ASSERT(soft_ap_ctx_.ssid.size());

  // Send request to driver
  auto result = client_.buffer(test_arena_)->StopBss(builder.Build());
  ZX_ASSERT(result.ok());
}

zx::channel SimInterface::TakeMlme() { return std::move(ch_mlme_); }

SimTest::SimTest() : test_arena_(fdf::Arena('T')) {
  env_ = std::make_unique<simulation::Environment>();
  env_->AddStation(this);
}

SimTest::~SimTest() {
  // Clean the ifaces created in test but not deleted.
  for (auto iface : ifaces_) {
    fuchsia_wlan_phy::WlanPhyDestroyIfaceRequest req{{.iface_id = iface.first}};
    auto result = client_->DestroyIface(req);
    if (result.is_error()) {
      BRCMF_ERR("Delete iface: %u failed", iface.first);
    }
  }
  if (driver_created_) {
    zx::result<> result = driver_test().StopDriver();
    EXPECT_EQ(ZX_OK, result.status_value());
    driver_test().ShutdownAndDestroyDriver();
  }
}

zx_status_t SimTest::PreInit() {
  zx::result<> result = driver_test().StartDriverWithCustomStartArgs(
      [&](fuchsia_driver_framework::DriverStartArgs& start_args) {
        // 1. Create a new endpoints pair for the outgoing directory of the driver.
        auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
        sim_outgoing_client_ = std::move(client);

        // 2. Intercept and replace the outgoing directory server end.
        auto orig_outgoing = std::move(start_args.outgoing_dir().value());
        start_args.outgoing_dir(std::move(server));

        // 3. Forward open requests from the test framework's original server end
        //    to the actual driver outgoing directory.
        zx_status_t status = fdio_open3_at(
            sim_outgoing_client_.handle()->get(), ".",
            uint64_t{fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolDirectory},
            orig_outgoing.TakeChannel().release());
        ZX_ASSERT_MSG(status == ZX_OK, "fdio_open3_at failed: %s", zx_status_get_string(status));
      });
  EXPECT_OK(result.status_value());

  WithSimDevice([this](brcmfmac::SimDevice* device) {
    device->InitWithEnv(env_.get(), sim_outgoing_client_);
  });

  driver_created_ = true;

  return ZX_OK;
}

zx_status_t SimTest::Init() {
  if (!driver_created_) {
    EXPECT_OK(PreInit());
  }

  libsync::Completion initialized;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    device->Initialize([&](zx_status_t status) {
      EXPECT_OK(status);
      initialized.Signal();
    });
  });
  initialized.Wait();

  // Connect to WlanPhy served on outgoing directory.
  zx::result connect_result = component::ConnectAtMember<fuchsia_wlan_phy::Service::Device>(
      CreateDriverSvcClient(), component::kDefaultInstance);

  client_ = fidl::SyncClient<fuchsia_wlan_phy::WlanPhy>(std::move(connect_result.value()));

  // Make a synchronous request to ensure that we are actually connected to the protocol.
  auto result = client_->GetSupportedMacRoles();
  EXPECT_TRUE(result.is_ok());

  return ZX_OK;
}

zx_status_t SimTest::CreateFactoryClient() {
  zx::result<fidl::ClientEnd<fuchsia_factory_wlan::Iovar>> client_end =
      driver_test().ConnectThroughDevfs<fuchsia_factory_wlan::Iovar>("factory-broadcom");
  EXPECT_EQ(ZX_OK, client_end.status_value());
  factory_client_.Bind(std::move(client_end.value()));
  EXPECT_EQ(true, factory_client_.is_valid());
  return ZX_OK;
}

zx_status_t SimTest::StartInterface(wlan_common::WlanMacRole role, SimInterface* sim_ifc,
                                    std::optional<MacAddr> mac_addr) {
  zx_status_t status;
  if ((status = sim_ifc->Init(env_.get(), role)) != ZX_OK) {
    return status;
  }
  fuchsia_wlan_phy::WlanPhyCreateIfaceRequest req{{
      .role = role,
      .mlme_channel = std::move(sim_ifc->ch_mlme_),
  }};

  if (mac_addr) {
    std::array<uint8_t, 6> init_sta_addr;
    memcpy(init_sta_addr.data(), mac_addr.value().byte, ETH_ALEN);
    req.init_sta_addr(init_sta_addr);
  }

  auto result = client_->CreateIface(std::move(req));

  if (result.is_error()) {
    zx_status_t status = result.error_value().is_domain_error()
                             ? result.error_value().domain_error()
                             : result.error_value().framework_error().status();
    BRCMF_ERR("%s error happened while creating interface", zx_status_get_string(status));
    return status;
  }
  sim_ifc->iface_id_ = result->iface_id().value();

  status = ZX_OK;

  if (!ifaces_.insert_or_assign(sim_ifc->iface_id_, sim_ifc).second) {
    BRCMF_ERR("Iface already exist in this test.\n");
    return ZX_ERR_ALREADY_EXISTS;
  }

  // Connect to WlanFullmacImpl
  std::string instance_name = role == wlan_common::WlanMacRole::kClient
                                  ? "brcmfmac-wlan-fullmac-client"
                                  : "brcmfmac-wlan-fullmac-ap";

  zx::result driver_connect_result =
      component::ConnectAtMember<fuchsia_wlan_fullmac::Service::WlanFullmacImpl>(
          CreateDriverSvcClient(), instance_name);
  EXPECT_EQ(ZX_OK, driver_connect_result.status_value());

  status = sim_ifc->Connect(std::move(driver_connect_result.value()), df_env_dispatcher());
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to establish FIDL connection with WlanInterface: %s",
              zx_status_get_string(status));
    return status;
  }

  // check that fullmac device count is expected.
  auto fullmac_service_prop =
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.wlan.fullmac.Service");
  EXPECT_EQ(ifaces_.size(), DeviceCountWithProperty(fullmac_service_prop));

  return ZX_OK;
}

zx_status_t SimTest::InterfaceDestroyed(SimInterface* ifc) {
  auto iter = ifaces_.find(ifc->iface_id_);

  if (iter == ifaces_.end()) {
    BRCMF_ERR("Iface id: %d does not exist", ifc->iface_id_);
    return ZX_ERR_NOT_FOUND;
  }

  // Destroy the server_dispatcher_ so that when this SimInterface is started again, the
  // server_dispatcher_ can be overwritten.
  ifc->Reset();
  ifaces_.erase(iter);

  auto fullmac_service_prop =
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.wlan.fullmac.Service");
  WaitForDeviceCountWithProperty(fullmac_service_prop, ifaces_.size());

  return ZX_OK;
}

uint32_t SimTest::DeviceCount() {
  return driver_test().RunInNodeContext<uint32_t>(
      [](fdf_testing::TestNode& root) { return root.children().size(); });
}

uint32_t SimTest::DeviceCountWithProperty(const fuchsia_driver_framework::NodeProperty2& property) {
  return driver_test().RunInNodeContext<uint32_t>([&](fdf_testing::TestNode& root) {
    uint32_t count = 0;
    for (const auto& [_, child] : root.children()) {
      for (const fuchsia_driver_framework::NodeProperty2& child_property : child.GetProperties()) {
        if (child_property == property) {
          count++;
          break;
        }
      }
    }

    return count;
  });
}

void SimTest::WaitForDeviceCount(uint32_t expected) {
  while (expected != DeviceCount()) {
    std::this_thread::sleep_for(std::chrono::seconds(1));
  }
}

void SimTest::WaitForDeviceCountWithProperty(
    const fuchsia_driver_framework::NodeProperty2& property, uint32_t expected) {
  while (expected != DeviceCountWithProperty(property)) {
    std::this_thread::sleep_for(std::chrono::seconds(1));
  }
}

void SimTest::WaitForRecoveryComplete() {
  // Wait until reset is complete. This has to happen on this thread, not the driver dispatcher.
  // Otherwise the wait will block part of the recovery work that has to happen on the driver
  // dispatcher.
  brcmfmac::SimDevice* device_ptr = nullptr;
  WithSimDevice([&](brcmfmac::SimDevice* device) { device_ptr = device; });
  device_ptr->WaitForRecoveryComplete();
}

void SimTest::WithSimDevice(fit::function<void(brcmfmac::SimDevice*)> callback) {
  driver_test().RunInDriverContext(
      [callback = std::move(callback)](brcmfmac::SimDevice& device) mutable { callback(&device); });
}

zx_status_t SimTest::DeleteInterface(SimInterface* ifc) {
  auto iter = ifaces_.find(ifc->iface_id_);

  if (iter == ifaces_.end()) {
    BRCMF_ERR("Iface id: %d does not exist", ifc->iface_id_);
    return ZX_ERR_NOT_FOUND;
  }

  fuchsia_wlan_phy::WlanPhyDestroyIfaceRequest req{{.iface_id = iter->first}};
  auto result = client_->DestroyIface(req);
  if (result.is_error()) {
    BRCMF_ERR("Failed to destroy interface.\n");
    return result.error_value().is_domain_error() ? result.error_value().domain_error()
                                                  : result.error_value().framework_error().status();
  }

  ifc->Reset();

  // Once the interface data structures have been deleted, our pointers are no longer valid.
  ifaces_.erase(iter);

  auto fullmac_service_prop =
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.wlan.fullmac.Service");
  WaitForDeviceCountWithProperty(fullmac_service_prop, ifaces_.size());

  return ZX_OK;
}

fidl::ClientEnd<fuchsia_io::Directory> SimTest::CreateDriverSvcClient() {
  return driver_test().ConnectToDriverSvcDir();
}

}  // namespace wlan::brcmfmac
