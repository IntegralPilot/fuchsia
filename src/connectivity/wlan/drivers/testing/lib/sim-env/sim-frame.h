// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_TESTING_LIB_SIM_ENV_SIM_FRAME_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_TESTING_LIB_SIM_ENV_SIM_FRAME_H_

#include <fidl/fuchsia.wlan.common/cpp/wire.h>
#include <fidl/fuchsia.wlan.ieee80211/cpp/common_types.h>
#include <fidl/fuchsia.wlan.ieee80211/cpp/natural_types.h>
#include <lib/stdcompat/span.h>
#include <zircon/types.h>

#include <list>
#include <memory>
#include <optional>

#include <wlan/drivers/macaddr.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-sta-ifc.h"
#include "src/connectivity/wlan/lib/common/cpp/include/wlan/common/mac_frame.h"

namespace wlan_ieee80211_wire = fuchsia_wlan_ieee80211::wire;

namespace wlan::simulation {

class StationIfc;

typedef struct WlanRxInfo {
  fuchsia_wlan_ieee80211::wire::ChannelNumber primary;
  fuchsia_wlan_ieee80211::wire::ChannelBandwidth bandwidth;
  fuchsia_wlan_ieee80211::wire::ChannelNumber vht_secondary_80_channel;
  double signal_strength;
  double noise_level;
} WlanRxInfo;

typedef struct WlanTxInfo {
  fuchsia_wlan_ieee80211::wire::ChannelNumber primary_channel;
  fuchsia_wlan_ieee80211::wire::ChannelBandwidth bandwidth;
  fuchsia_wlan_ieee80211::wire::ChannelNumber vht_secondary_80_channel;
} WlanTxInfo;

// Representative for security protocol, we don't have this field in real situation.
enum SimSecProtoType {
  SEC_PROTO_TYPE_OPEN,
  SEC_PROTO_TYPE_WEP,
  SEC_PROTO_TYPE_WPA1,
  SEC_PROTO_TYPE_WPA2,
  SEC_PROTO_TYPE_WPA3
};

// AUTH_TYPE used by AP and authentication frame
enum SimAuthType { AUTH_TYPE_OPEN, AUTH_TYPE_SHARED_KEY, AUTH_TYPE_SAE };

class InformationElement {
 public:
  enum SimIeType { IE_TYPE_SSID = 0, IE_TYPE_CSA = 37, IE_TYPE_WPA1 = 221, IE_TYPE_WPA2 = 48 };

  explicit InformationElement() = default;
  virtual ~InformationElement();

  virtual SimIeType IeType() const = 0;

  // Return the IE as a buffer of bytes, in the 802.11-specified format for this IE type.
  virtual std::vector<uint8_t> ToRawIe() const = 0;
};

// IEEE Std 802.11-2016, 9.4.2.2
class SsidInformationElement : public InformationElement {
 public:
  explicit SsidInformationElement(const fuchsia_wlan_ieee80211::Ssid& ssid) : ssid_(ssid) {}

  SsidInformationElement(const SsidInformationElement& ssid_ie);

  ~SsidInformationElement() override;

  SimIeType IeType() const override;

  std::vector<uint8_t> ToRawIe() const override;

  fuchsia_wlan_ieee80211::Ssid ssid_;
};

// IEEE Std 802.11-2016, 9.4.2.19
class CsaInformationElement : public InformationElement {
 public:
  explicit CsaInformationElement(bool switch_mode, uint8_t new_channel, uint8_t switch_count) {
    channel_switch_mode_ = switch_mode;
    new_channel_number_ = new_channel;
    channel_switch_count_ = switch_count;
  }

  CsaInformationElement(const CsaInformationElement& csa_ie);

  ~CsaInformationElement() override;

  SimIeType IeType() const override;

  std::vector<uint8_t> ToRawIe() const override;

  bool channel_switch_mode_;
  uint8_t new_channel_number_;
  uint8_t channel_switch_count_;
};

class SimFrame {
 public:
  enum SimFrameType { FRAME_TYPE_MGMT, FRAME_TYPE_CTRL, FRAME_TYPE_DATA };

  SimFrame() = default;
  virtual ~SimFrame();

  // Frame type identifier
  virtual SimFrameType FrameType() const = 0;

  virtual SimFrame* CopyFrame() const = 0;
};

class SimManagementFrame : public SimFrame {
 public:
  enum SimMgmtFrameType {
    FRAME_TYPE_BEACON,
    FRAME_TYPE_PROBE_REQ,
    FRAME_TYPE_PROBE_RESP,
    FRAME_TYPE_ASSOC_REQ,
    FRAME_TYPE_ASSOC_RESP,
    FRAME_TYPE_DISASSOC_REQ,
    FRAME_TYPE_AUTH,
    FRAME_TYPE_DEAUTH,
    FRAME_TYPE_REASSOC_REQ,
    FRAME_TYPE_REASSOC_RESP,
    FRAME_TYPE_ACTION,
  };

  SimManagementFrame() = default;

  SimManagementFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr& dst)
      : src_addr_(src), dst_addr_(dst) {}

  SimManagementFrame(const SimManagementFrame& mgmt_frame);
  ~SimManagementFrame() override;

  // Frame type identifier
  SimFrameType FrameType() const override;
  // Frame subtype identifier for management frames
  virtual SimMgmtFrameType MgmtFrameType() const = 0;
  void AddSsidIe(const fuchsia_wlan_ieee80211::Ssid& ssid);
  void AddCsaIe(const fuchsia_wlan_ieee80211::wire::ChannelNumber& channel,
                uint8_t channel_switch_count);
  void AddRawIes(cpp20::span<const uint8_t> raw_ies);
  std::shared_ptr<InformationElement> FindIe(InformationElement::SimIeType ie_type) const;
  void RemoveIe(InformationElement::SimIeType);

  wlan::common::MacAddr src_addr_ = {};
  wlan::common::MacAddr dst_addr_ = {};
  std::list<std::shared_ptr<InformationElement>> IEs_;
  std::vector<uint8_t> raw_ies_;
  // This is a brief alternative for security related IEs since we don't include entire IE for
  // security protocol such as WPA IE or RSNE IE.
  enum SimSecProtoType sec_proto_type_ = SEC_PROTO_TYPE_OPEN;

 private:
  void AddIe(InformationElement::SimIeType ie_type, std::shared_ptr<InformationElement> ie);
};

class SimBeaconFrame : public SimManagementFrame {
 public:
  SimBeaconFrame() = default;
  explicit SimBeaconFrame(const fuchsia_wlan_ieee80211::Ssid& ssid,
                          const wlan::common::MacAddr& bssid);

  SimBeaconFrame(const SimBeaconFrame& beacon);

  ~SimBeaconFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;

  wlan::common::MacAddr bssid_;
  zx::duration interval_;
  wlan::CapabilityInfo capability_info_;
};

class SimProbeReqFrame : public SimManagementFrame {
 public:
  SimProbeReqFrame() = default;
  explicit SimProbeReqFrame(const wlan::common::MacAddr& src) : SimManagementFrame(src, {}) {}

  SimProbeReqFrame(const SimProbeReqFrame& probe_req);

  ~SimProbeReqFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;
};

class SimProbeRespFrame : public SimManagementFrame {
 public:
  SimProbeRespFrame() = default;
  explicit SimProbeRespFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr& dst,
                             const fuchsia_wlan_ieee80211::Ssid& ssid);

  SimProbeRespFrame(const SimProbeRespFrame& probe_resp);

  ~SimProbeRespFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;

  wlan::CapabilityInfo capability_info_;
};

class SimAssocReqFrame : public SimManagementFrame {
 public:
  SimAssocReqFrame() = default;
  explicit SimAssocReqFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr bssid,
                            const fuchsia_wlan_ieee80211::Ssid& ssid)
      : SimManagementFrame(src, {}), bssid_(bssid), ssid_(ssid) {}

  SimAssocReqFrame(const SimAssocReqFrame& assoc_req);

  ~SimAssocReqFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;

  wlan::common::MacAddr bssid_;
  fuchsia_wlan_ieee80211::Ssid ssid_;
};

class SimAssocRespFrame : public SimManagementFrame {
 public:
  SimAssocRespFrame() = default;
  explicit SimAssocRespFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr& dst,
                             wlan_ieee80211_wire::StatusCode status)
      : SimManagementFrame(src, dst), status_(status) {
    capability_info_.set_ess(1);
  }

  SimAssocRespFrame(const SimAssocRespFrame& assoc_resp);

  ~SimAssocRespFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;

  wlan_ieee80211_wire::StatusCode status_;
  wlan::CapabilityInfo capability_info_;
};

class SimDisassocReqFrame : public SimManagementFrame {
 public:
  SimDisassocReqFrame() = default;
  explicit SimDisassocReqFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr& dst,
                               wlan_ieee80211_wire::ReasonCode reason)
      : SimManagementFrame(src, dst), reason_(reason) {}

  SimDisassocReqFrame(const SimDisassocReqFrame& disassoc_req);

  ~SimDisassocReqFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;

  wlan_ieee80211_wire::ReasonCode reason_;
};

// Only one type of authentication frame for request and response
class SimAuthFrame : public SimManagementFrame {
 public:
  SimAuthFrame() = default;
  explicit SimAuthFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr& dst,
                        uint16_t seq, SimAuthType auth_type, wlan_ieee80211_wire::StatusCode status)
      : SimManagementFrame(src, dst), seq_num_(seq), auth_type_(auth_type), status_(status) {}

  SimAuthFrame(const SimAuthFrame& auth);

  ~SimAuthFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;
  void AddChallengeText(cpp20::span<const uint8_t> text);

  uint16_t seq_num_;
  SimAuthType auth_type_;
  wlan_ieee80211_wire::StatusCode status_;

  // Payload for authentication frame, especially being used for SAE process for now.
  std::vector<uint8_t> payload_;
};

class SimDeauthFrame : public SimManagementFrame {
 public:
  SimDeauthFrame() = default;
  explicit SimDeauthFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr& dst,
                          wlan_ieee80211_wire::ReasonCode reason)
      : SimManagementFrame(src, dst), reason_(reason) {}

  SimDeauthFrame(const SimDeauthFrame& deauth);

  ~SimDeauthFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;

  wlan_ieee80211_wire::ReasonCode reason_;
};

// IEEE 802.11-2020 9.3.3.7
class SimReassocReqFrame : public SimManagementFrame {
 public:
  SimReassocReqFrame() = default;
  explicit SimReassocReqFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr bssid)
      : SimManagementFrame(src, {}), bssid_(bssid) {}

  SimReassocReqFrame(const SimReassocReqFrame& reassoc_req);

  ~SimReassocReqFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;

  wlan::common::MacAddr bssid_;
};

// IEEE 802.11-2020 9.3.3.8
class SimReassocRespFrame : public SimManagementFrame {
 public:
  SimReassocRespFrame() = default;
  explicit SimReassocRespFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr& dst,
                               wlan_ieee80211_wire::StatusCode status)
      : SimManagementFrame(src, dst), status_(status) {
    capability_info_.set_ess(1);
  }

  SimReassocRespFrame(const SimReassocRespFrame& reassoc_resp);

  ~SimReassocRespFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;

  wlan_ieee80211_wire::StatusCode status_;
  wlan::CapabilityInfo capability_info_;
};

// IEEE 802.11-2020 9.3.3.13.
class SimActionFrame : public SimManagementFrame {
 public:
  // IEEE 802.11-2020 Table 9-51.
  enum SimActionCategory {
    WNM,
  };

  explicit SimActionFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr& dst,
                          SimActionCategory category)
      : SimManagementFrame(src, dst), category_(category) {}

  SimActionFrame(const SimActionFrame& action);

  ~SimActionFrame() override;

  SimMgmtFrameType MgmtFrameType() const override;

  SimFrame* CopyFrame() const override;

  SimActionCategory ActionCategory() const;

 private:
  SimActionCategory category_;
};

// IEEE 802.11-2020 9.6.13.
class SimWnmActionFrame : public SimActionFrame {
 public:
  // IEEE 802.11-2020 9.6.13.1
  enum SimWnmAction {
    BSS_TRANSITION_MANAGEMENT_REQUEST,
  };

  explicit SimWnmActionFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr& dst)
      : SimActionFrame(src, dst, SimActionCategory::WNM) {}

  SimWnmActionFrame(const SimWnmActionFrame& wnm_action);

  ~SimWnmActionFrame() override;

  SimFrame* CopyFrame() const override;

  SimWnmAction WnmAction() const { return wnm_action_; }

 private:
  // Only WNM action, later will include other actions and will be non const.
  const SimWnmAction wnm_action_ = BSS_TRANSITION_MANAGEMENT_REQUEST;
};

// IEEE 802.11-2020 Fig. 9-924.
struct SimBtmReqMode {
  bool preferred_candidate_list_included = false;
};

// Info about a neighboring AP. IEEE 802.11-2020 9.4.2.36.
struct SimNeighborReportElement {
  wlan::common::MacAddr bssid;
  uint8_t operating_class;  // IEEE 802.11-2020 Annex E
  uint8_t channel_number;
};

// IEEE 802.11-2020 9.6.13.9.
class SimBtmReqFrame : public SimWnmActionFrame {
 public:
  explicit SimBtmReqFrame(const wlan::common::MacAddr& src, const wlan::common::MacAddr& dst,
                          const SimBtmReqMode& request_mode,
                          const std::vector<SimNeighborReportElement>& candidate_list)
      : SimWnmActionFrame(src, dst), request_mode_(request_mode), candidate_list_(candidate_list) {}

  SimBtmReqFrame(const SimBtmReqFrame& btm_req);

  ~SimBtmReqFrame() override;

  SimFrame* CopyFrame() const override;

  std::vector<SimNeighborReportElement> CandidateList() const;

 private:
  SimBtmReqMode request_mode_;
  // IEEE 802.11-2020 9.4.2.36
  std::vector<SimNeighborReportElement> candidate_list_;
};

// No support for contention-free data frames, aggregation or fragmentation for now
// Assumes singular MSDU frames
// reassembly, decryption also not supported
class SimDataFrame : public SimFrame {
 public:
  enum SimDataFrameType { FRAME_TYPE_QOS_DATA };

  SimDataFrame() = default;
  explicit SimDataFrame(bool toDS, bool fromDS, wlan::common::MacAddr addr1,
                        wlan::common::MacAddr addr2, wlan::common::MacAddr addr3,
                        std::optional<uint16_t> qosControl, std::vector<uint8_t> payload)
      : toDS_(toDS),
        fromDS_(fromDS),
        addr1_(addr1),
        addr2_(addr2),
        addr3_(addr3),
        qosControl_(qosControl),
        payload_(payload) {}

  SimDataFrame(const SimDataFrame& data_frame);

  ~SimDataFrame() override;

  SimFrameType FrameType() const override;

  // Frame subtype identifier for data frames
  virtual SimDataFrameType DataFrameType() const = 0;

  // Control bits
  bool toDS_;
  bool fromDS_;

  // IEEE Std. 802.11-2016, 9.3.2.1 Table 9-26
  wlan::common::MacAddr addr1_;

  wlan::common::MacAddr addr2_;

  wlan::common::MacAddr addr3_;

  std::optional<wlan::common::MacAddr> addr4_;

  std::optional<uint16_t> qosControl_;

  // MAC Payload
  std::vector<uint8_t> payload_;
};

class SimQosDataFrame : public SimDataFrame {
 public:
  SimQosDataFrame() = default;
  explicit SimQosDataFrame(bool toDS, bool fromDS, wlan::common::MacAddr addr1,
                           wlan::common::MacAddr addr2, wlan::common::MacAddr addr3,
                           std::optional<uint16_t> qosControl, std::vector<uint8_t> payload)
      : SimDataFrame(toDS, fromDS, addr1, addr2, addr3, qosControl, payload) {}

  SimQosDataFrame(const SimQosDataFrame& qos_data);

  ~SimQosDataFrame() override;

  SimDataFrameType DataFrameType() const override;

  SimFrame* CopyFrame() const override;
};

}  // namespace wlan::simulation

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_TESTING_LIB_SIM_ENV_SIM_FRAME_H_
