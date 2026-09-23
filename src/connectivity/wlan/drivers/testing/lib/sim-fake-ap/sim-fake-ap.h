// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_TESTING_LIB_SIM_FAKE_AP_SIM_FAKE_AP_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_TESTING_LIB_SIM_FAKE_AP_SIM_FAKE_AP_H_

#include <fidl/fuchsia.wlan.common/cpp/wire.h>
#include <fidl/fuchsia.wlan.ieee80211/cpp/common_types.h>
#include <fidl/fuchsia.wlan.ieee80211/cpp/natural_types.h>
#include <lib/zx/time.h>
#include <netinet/if_ether.h>
#include <stdint.h>
#include <zircon/types.h>

#include <array>
#include <functional>
#include <list>

#include <wlan/common/ieee80211.h>
#include <wlan/drivers/macaddr.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-env.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-frame.h"
#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-sta-ifc.h"

namespace wlan_ieee80211 = fuchsia_wlan_ieee80211::wire;
namespace wlan_common = fuchsia_wlan_common::wire;
namespace wlan::simulation {

// To simulate an AP. Only keep minimum information for sim-fw to generate
// response for driver.
//
class FakeAp final : public StationIfc {
 public:
  // How to handle an association request
  enum AssocHandling { ASSOC_ALLOWED, ASSOC_IGNORED, ASSOC_REFUSED_TEMPORARILY, ASSOC_REFUSED };

  struct Security {
    enum SimAuthType auth_handling_mode = AUTH_TYPE_OPEN;
    enum ieee80211_cipher_suite cipher_suite;

    static constexpr size_t kMaxKeyLen = 32;
    size_t key_len;
    std::array<uint8_t, kMaxKeyLen> key;
    enum SimSecProtoType sec_type = SEC_PROTO_TYPE_OPEN;

    // TODO(https://fxbug.dev/42139397): Remove this field which is currently used to
    // determine success or failure of a simulated authentication
    // out of band.
    bool expect_challenge_failure = false;
  };

  struct Client {
    // AUTHENTICATING is the status where AP has sent out second auth frame, and waiting for the
    // third, this only apply to AUTH_SHARED mode.
    enum Status { NOT_AUTHENTICATED, AUTHENTICATING, AUTHENTICATED, ASSOCIATED };

    Client(wlan::common::MacAddr mac_addr, Status status) : mac_addr_(mac_addr), status_(status) {}
    wlan::common::MacAddr mac_addr_;
    Status status_;
  };

  FakeAp() = delete;

  explicit FakeAp(Environment* environ) : environment_(environ) { environ->AddStation(this); }

  FakeAp(Environment* environ, const wlan::common::MacAddr& bssid,
         const fuchsia_wlan_ieee80211::Ssid& ssid,
         const fuchsia_wlan_ieee80211::wire::ChannelNumber primary_channel,
         fuchsia_wlan_ieee80211::wire::ChannelBandwidth bandwidth,
         fuchsia_wlan_ieee80211::wire::ChannelNumber vht_secondary_80_channel)
      : environment_(environ),
        primary_channel_(primary_channel),
        bandwidth_(bandwidth),
        vht_secondary_80_channel_(vht_secondary_80_channel),
        bssid_(bssid),
        ssid_(ssid) {
    environ->AddStation(this);
    tx_info_.primary_channel = primary_channel;
    tx_info_.bandwidth = bandwidth;
    tx_info_.vht_secondary_80_channel = vht_secondary_80_channel;
    beacon_state_.beacon_frame_.bssid_ = bssid;
    beacon_state_.beacon_frame_.AddSsidIe(ssid);
    // By default, assume AP is part of an infrastructure network
    beacon_state_.beacon_frame_.capability_info_.set_ibss(0);
    beacon_state_.beacon_frame_.capability_info_.set_ess(1);
  }

  FakeAp(Environment* environ, const wlan::common::MacAddr& bssid,
         const fuchsia_wlan_ieee80211::Ssid& ssid,
         const fuchsia_wlan_ieee80211::wire::ChannelNumber primary,
         fuchsia_wlan_ieee80211::wire::ChannelBandwidth bandwidth,
         uint8_t vht_secondary_80_channel_num)
      : FakeAp(environ, bssid, ssid, primary, bandwidth,
               fuchsia_wlan_ieee80211::wire::ChannelNumber{
                   .band = primary.band, .number = vht_secondary_80_channel_num}) {}

  ~FakeAp() { environment_->RemoveStation(this); }

  void SetChannel(const fuchsia_wlan_ieee80211::wire::ChannelNumber& primary,
                  const fuchsia_wlan_ieee80211::wire::ChannelBandwidth& bandwidth,
                  const fuchsia_wlan_ieee80211::wire::ChannelNumber& vht_secondary_80_channel);

  void SetChannel(const fuchsia_wlan_ieee80211::wire::ChannelNumber& primary,
                  const fuchsia_wlan_ieee80211::wire::ChannelBandwidth& bandwidth,
                  const uint8_t& vht_secondary_80_channel_num) {
    SetChannel(primary, bandwidth,
               fuchsia_wlan_ieee80211::wire::ChannelNumber{.band = primary.band,
                                                           .number = vht_secondary_80_channel_num});
  }

  void SetBssid(const wlan::common::MacAddr& bssid);
  void SetSsid(const fuchsia_wlan_ieee80211::Ssid& ssid);
  void SetCsaBeaconInterval(zx::duration interval);

  fuchsia_wlan_ieee80211::wire::ChannelNumber GetChannel() const { return primary_channel_; }
  fuchsia_wlan_ieee80211::wire::ChannelBandwidth GetChannelBandwidth() const { return bandwidth_; }
  fuchsia_wlan_ieee80211::wire::ChannelNumber GetSecondary80() const {
    return vht_secondary_80_channel_;
  }

  wlan::common::MacAddr GetBssid() const { return bssid_; }
  fuchsia_wlan_ieee80211::Ssid GetSsid() const { return ssid_; }
  uint32_t GetNumAssociatedClient() const;

  // Will we receive a message sent on the specified channel?
  bool CanReceiveChannel(const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel);

  // When this is not called, the default is open network.
  zx_status_t SetSecurity(struct Security sec);

  // Start beaconing. Sends first beacon immediately and schedules beacons to occur every
  // beacon_period until disabled.
  void EnableBeacon(zx::duration beacon_period);

  // Stop beaconing.
  void DisableBeacon();

  // Specify how to handle association requests
  void SetAssocHandling(enum AssocHandling mode);

  // Disassociate a Station
  zx_status_t DisassocSta(const wlan::common::MacAddr& sta_mac, wlan_ieee80211::ReasonCode reason);

  // Deauthenticate a Station
  zx_status_t DeauthSta(const wlan::common::MacAddr& sta_mac, wlan_ieee80211::ReasonCode reason);

  // Send a BSS Transition Management request.
  void SendBtmReq(const simulation::SimBtmReqFrame& btm_req);

  // Beacon-specific error injection.
  // The beacon_mutator functor will be applied to each beacon frame before transmission.
  void AddErrInjBeacon(std::function<SimBeaconFrame(const SimBeaconFrame&)> beacon_mutator);

  void DelErrInjBeacon();
  bool CheckIfErrInjBeaconEnabled() const;

  // StationIfc operations - these are the functions that allow the simulated AP to be used
  // inside of a sim-env environment.
  void Rx(std::shared_ptr<const SimFrame> frame, std::shared_ptr<const WlanRxInfo> info) override;

 private:
  void CancelNotification(uint64_t id);
  std::shared_ptr<Client> AddClient(wlan::common::MacAddr mac_addr);
  std::shared_ptr<Client> FindClient(wlan::common::MacAddr mac_addr);
  void RemoveClient(wlan::common::MacAddr mac_addr);

  void RxMgmtFrame(std::shared_ptr<const SimManagementFrame> mgmt_frame);
  void RxDataFrame(std::shared_ptr<const SimDataFrame> data_frame);

  void ScheduleNextBeacon();
  void ScheduleAssocResp(wlan_ieee80211::StatusCode status, const wlan::common::MacAddr& dst);
  void ScheduleReassocResp(wlan_ieee80211::StatusCode status, const wlan::common::MacAddr& dst);
  void ScheduleProbeResp(const wlan::common::MacAddr& dst);
  void ScheduleAuthResp(std::shared_ptr<const SimAuthFrame> auth_frame_in,
                        wlan_ieee80211::StatusCode status);
  void ScheduleQosData(bool toDS, bool fromDS, const wlan::common::MacAddr& addr1,
                       const wlan::common::MacAddr& addr2, const wlan::common::MacAddr& addr3,
                       const std::vector<uint8_t>& payload);

  // Event handlers
  void HandleBeaconNotification();
  void HandleStopCsaBeaconNotification();
  void HandleAssocRespNotification(wlan_ieee80211::StatusCode status, wlan::common::MacAddr dst);
  void HandleReassocRespNotification(wlan_ieee80211::StatusCode status, wlan::common::MacAddr dst);
  void HandleProbeRespNotification(wlan::common::MacAddr dst);
  void HandleAuthRespNotification(SimAuthFrame auth_resp_frame);
  void HandleQosDataNotification(bool toDS, bool fromDS, const wlan::common::MacAddr& addr1,
                                 const wlan::common::MacAddr& addr2,
                                 const wlan::common::MacAddr& addr3,
                                 const std::vector<uint8_t>& payload);

  // The environment in which this fake AP is operating.
  Environment* environment_;

  // meta information needed for sending transmissions
  simulation::WlanTxInfo tx_info_;
  fuchsia_wlan_ieee80211::wire::ChannelNumber primary_channel_;
  fuchsia_wlan_ieee80211::wire::ChannelBandwidth bandwidth_;
  fuchsia_wlan_ieee80211::wire::ChannelNumber vht_secondary_80_channel_;
  wlan::common::MacAddr bssid_;
  fuchsia_wlan_ieee80211::Ssid ssid_;
  struct Security security_ = {.cipher_suite = IEEE80211_CIPHER_SUITE_NONE};

  struct BeaconState {
    // Are we currently emitting beacons?
    bool is_beaconing = false;
    // Are we waiting for the execution of scheduled channel switch announcement?
    bool is_switching_channel = false;
    // This is the channel AP about to change to
    fuchsia_wlan_ieee80211::wire::ChannelNumber channel_after_csa;
    fuchsia_wlan_ieee80211::wire::ChannelBandwidth bandwidth_after_csa;
    fuchsia_wlan_ieee80211::wire::ChannelNumber vht_secondary_80_channel_after_csa;

    // Unique value that is associated with the next beacon event
    uint64_t beacon_notification_id;
    // Unique value that is associated with the upcoming channel switch event
    uint64_t channel_switch_notification_id;
    // The time in sim_env for next beacon
    zx::time next_beacon_time;

    // There is only one static copy of beacon frame, and AP can modify it according to state.
    simulation::SimBeaconFrame beacon_frame_;

    // Functor that will mutate beacon frames if error injection is enabled.
    // If this is nullptr, beacon error injection is disabled.
    std::function<SimBeaconFrame(const SimBeaconFrame&)> beacon_mutator;
  } beacon_state_;

  enum AssocHandling assoc_handling_mode_ = ASSOC_ALLOWED;

  // Delay between start and stop sending CSA beacon to old channel
  zx::duration csa_beacon_interval_ = zx::msec(150);
  // Delay between an association request and an association response
  zx::duration assoc_resp_interval_ = zx::msec(1);
  // Delay between a reassociation request and a reassociation response.
  // Value based on actual timing seen during real reassociation.
  zx::duration reassoc_resp_interval_ = zx::msec(65);
  // Delay between an Disassociation request and an Disassociation response
  zx::duration disassoc_resp_interval_ = zx::msec(1);
  // Delay between an probe request and an probe response
  zx::duration probe_resp_interval_ = zx::msec(1);
  // Delay between an auth request and an auth response
  zx::duration auth_resp_interval_ = zx::msec(1);
  // Delay between forwarding data frames
  zx::duration data_forward_interval_ = zx::msec(1);

  std::list<std::shared_ptr<Client>> clients_;
};

}  // namespace wlan::simulation

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_TESTING_LIB_SIM_FAKE_AP_SIM_FAKE_AP_H_
