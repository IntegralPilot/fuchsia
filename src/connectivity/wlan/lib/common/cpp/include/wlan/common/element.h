// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_LIB_COMMON_CPP_INCLUDE_WLAN_COMMON_ELEMENT_H_
#define SRC_CONNECTIVITY_WLAN_LIB_COMMON_CPP_INCLUDE_WLAN_COMMON_ELEMENT_H_

#include <endian.h>
#include <fuchsia/wlan/mlme/cpp/fidl.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <cstdint>

#include <wlan/common/bitfield.h>
#include <wlan/common/element_id.h>
#include <wlan/common/logging.h>

namespace wlan {

// IEEE Std 802.11-2016, 9.4.2.1
struct ElementHeader {
  uint8_t id;
  uint8_t len;
} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.4
struct DsssParamSet {
  uint8_t current_channel;
};

// IEEE Std 802.11-2016, 9.4.2.5
struct CfParamSet {
  uint8_t count;
  uint8_t period;
  uint16_t max_duration;
  uint16_t dur_remaining;
} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.6
class BitmapControl : public common::BitField<uint8_t> {
 public:
  BitmapControl() = default;
  explicit BitmapControl(uint8_t raw) : BitField(raw) {}
  WLAN_BIT_FIELD(group_traffic_ind, 0, 1)
  WLAN_BIT_FIELD(offset, 1, 7)
};

constexpr size_t kMaxTimBitmapLen = 251;

// IEEE Std 802.11-2016, 9.4.2.9. Figure 9-131, 9-132.
struct SubbandTriplet {
  uint8_t first_channel_number;
  uint8_t number_of_channels;
  uint8_t max_tx_power;  // dBm
} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.9
struct Country {
  static constexpr size_t kCountryLen = 3;
  uint8_t data[kCountryLen];
} __PACKED;
static_assert(sizeof(Country) == Country::kCountryLen);

const uint16_t kEapolProtocolId = 0x888E;

// IEEE Std 802.11-2016, 9.4.1.17
class QosInfo : public common::BitField<uint8_t> {
 public:
  constexpr explicit QosInfo(uint8_t value) : common::BitField<uint8_t>(value) {}
  constexpr QosInfo() = default;

  // AP specific QoS Info structure: IEEE Std 802.11-2016, 9.4.1.17, Figure 9-82
  WLAN_BIT_FIELD(edca_param_set_update_count, 0, 4)
  WLAN_BIT_FIELD(qack, 4, 1)
  WLAN_BIT_FIELD(queue_request, 5, 1)
  WLAN_BIT_FIELD(txop_request, 6, 1)
  // 8th bit reserved

  // Non-AP STA specific QoS Info structure: IEEE Std 802.11-2016, 9.4.1.17,
  // Figure 9-83
  WLAN_BIT_FIELD(ac_vo_uapsd_flag, 0, 1)
  WLAN_BIT_FIELD(ac_vi_uapsd_flag, 1, 1)
  WLAN_BIT_FIELD(ac_bk_uapsd_flag, 2, 1)
  WLAN_BIT_FIELD(ac_be_uapsd_flag, 3, 1)
  // qack already defined in AP specific structure.
  WLAN_BIT_FIELD(max_sp_len, 5, 1)
  WLAN_BIT_FIELD(more_data_ack, 6, 1)
  // 8th bit reserved
} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.30, Table 9-139
enum TsDirection : uint8_t {
  kUplink = 0,
  kDownlink = 1,
  kDirectLink = 2,
  kBidirectionalLink = 3,
};

// IEEE Std 802.11-2016, 9.4.2.30, Table 9-140
enum TsAccessPolicy : uint8_t {
  // 0 reserved
  kEdca = 1,
  kHccaSpca = 2,
  kMixedMode = 3,
};

// IEEE Std 802.11-2016, 9.4.2.30, Table 9-141
namespace ts_ack_policy {
enum TsAckPolicy : uint8_t {
  kNormalAck = 0,
  kNoAck = 1,
  // 2 reserved
  kBlockAck = 3,
};
}  // namespace ts_ack_policy

// IEEE Std 802.11-2016, 9.4.2.30, Table 9-142
// Only used if TsInfo's access policy uses EDCA.
// Schedule Setting depends on TsInfo's ASPD and schedule fields.
enum TsScheduleSetting : uint8_t {
  kNoSchedule = 0,
  kUnschedledApsd = 1,
  kScheduledPsmp_GcrSp = 2,
  kScheduledApsd = 3,
};

// IEEE Std 802.11-2016, 9.4.2.30, Figure 9-267
struct NominalMsduSize : public common::BitField<uint16_t> {
  constexpr explicit NominalMsduSize(uint16_t params) : common::BitField<uint16_t>(params) {}
  constexpr NominalMsduSize() = default;

  WLAN_BIT_FIELD(size, 0, 15) WLAN_BIT_FIELD(fixed, 15, 1)
} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.56.2
// Note this is a field of HtCapabilities element.
class HtCapabilityInfo : public common::LittleEndianBitField<2> {
 public:
  constexpr explicit HtCapabilityInfo(std::array<uint8_t, 2> ht_cap_info)
      : common::LittleEndianBitField<2>(ht_cap_info) {}
  constexpr HtCapabilityInfo() = default;

  WLAN_BIT_FIELD(as_uint16, 0, 16)  // Read/write as a uint16_t

  WLAN_BIT_FIELD(ldpc_coding_cap, 0, 1)
  WLAN_BIT_FIELD(chan_width_set, 1, 1)  // In spec: Supported Channel Width Set
  WLAN_BIT_FIELD(sm_power_save, 2, 2)   // Spatial Multiplexing Power Save
  WLAN_BIT_FIELD(greenfield, 4, 1)      // HT-Greenfield.
  WLAN_BIT_FIELD(short_gi_20, 5, 1)     // Short Guard Interval for 20 MHz
  WLAN_BIT_FIELD(short_gi_40, 6, 1)     // Short Guard Interval for 40 MHz
  WLAN_BIT_FIELD(tx_stbc, 7, 1)

  WLAN_BIT_FIELD(rx_stbc, 8, 2)             // maximum number of spatial streams. Up to 3.
  WLAN_BIT_FIELD(delayed_block_ack, 10, 1)  // HT-delayed Block Ack
  WLAN_BIT_FIELD(max_amsdu_len, 11, 1)
  WLAN_BIT_FIELD(dsss_in_40, 12, 1)  // DSSS/CCK Mode in 40 MHz
  WLAN_BIT_FIELD(reserved, 13, 1)
  WLAN_BIT_FIELD(intolerant_40, 14, 1)  // 40 MHz Intolerant
  WLAN_BIT_FIELD(lsig_txop_protect, 15, 1)

  enum ChanWidthSet {
    TWENTY_ONLY = 0,
    TWENTY_FORTY = 1,
  };

  enum SmPowerSave {
    STATIC = 0,
    DYNAMIC = 1,
    RESERVED = 2,
    DISABLED = 3,
  };

  enum MaxAmsduLen {
    OCTETS_3839 = 0,
    OCTETS_7935 = 1,
  };
} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.56.3
class AmpduParams : public common::BitField<uint8_t> {
 public:
  constexpr explicit AmpduParams(uint8_t params) : common::BitField<uint8_t>(params) {}
  constexpr AmpduParams() = default;

  WLAN_BIT_FIELD(exponent, 0, 2)           // Maximum A-MPDU Length Exponent.
  WLAN_BIT_FIELD(min_start_spacing, 2, 3)  // Minimum MPDU Start Spacing.
  WLAN_BIT_FIELD(reserved, 5, 3)

  size_t max_ampdu_len() const { return (1 << (13 + exponent())) - 1; }

  enum MinMPDUStartSpacing {
    NO_RESTRICT = 0,
    QUARTER_USEC = 1,
    HALF_USEC = 2,
    ONE_USEC = 3,
    TWO_USEC = 4,
    FOUR_USEC = 5,
    EIGHT_USEC = 6,
    SIXTEEN_USEC = 7,
  };
} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.56.4
class SupportedMcsSet : public common::LittleEndianBitField<16> {
 public:
  constexpr explicit SupportedMcsSet(std::array<uint8_t, 16> val)
      : common::LittleEndianBitField<16>(val) {}
  constexpr SupportedMcsSet() = default;

  WLAN_BIT_FIELD(as_uint16, 0, 16)  // Read/write as a uint16_t

  // IEEE Std 802.11-2016, 9.4.2.56.4.
  WLAN_BIT_FIELD(rx_mcs_bitmask1, 0, 64)
  WLAN_BIT_FIELD(rx_mcs_bitmask2, 64, 13)
  WLAN_BIT_FIELD(reserved1, 77, 3)
  WLAN_BIT_FIELD(rx_highest_rate, 80, 10)
  WLAN_BIT_FIELD(reserved2, 90, 6)
  WLAN_BIT_FIELD(tx_set_defined, 96, 1)
  WLAN_BIT_FIELD(tx_rx_diff, 97, 1)
  WLAN_BIT_FIELD(tx_max_ss, 98, 2)
  WLAN_BIT_FIELD(tx_unequal_mod, 100, 1)
  WLAN_BIT_FIELD(reserved3, 101, 27)
} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.56.5
class HtExtCapabilities : public common::LittleEndianBitField<2> {
 public:
  constexpr explicit HtExtCapabilities(std::array<uint8_t, 2> ht_ext_cap)
      : common::LittleEndianBitField<2>(ht_ext_cap) {}
  constexpr HtExtCapabilities() = default;

  WLAN_BIT_FIELD(as_uint16, 0, 16)  // Read/write as a uint16_t

  WLAN_BIT_FIELD(pco, 0, 1)
  WLAN_BIT_FIELD(pco_transition, 1, 2)
  WLAN_BIT_FIELD(reserved1, 3, 5)
  WLAN_BIT_FIELD(mcs_feedback, 8, 2)
  WLAN_BIT_FIELD(htc_ht_support, 10, 1)
  WLAN_BIT_FIELD(rd_responder, 11, 1)
  WLAN_BIT_FIELD(reserved2, 12, 4)

  enum PcoTransitionTime {
    PCO_RESERVED = 0,  // Often translated as "No transition".
    PCO_400_USEC = 1,
    PCO_1500_USEC = 2,
    PCO_5000_USEC = 3,
  };

  enum McsFeedback {
    MCS_NOFEEDBACK = 0,
    MCS_RESERVED = 1,
    MCS_UNSOLICIED = 2,
    MCS_BOTH = 3,
  };

} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.56.6
class TxBfCapability : public common::LittleEndianBitField<4> {
 public:
  constexpr explicit TxBfCapability(std::array<uint8_t, 4> txbf_cap)
      : common::LittleEndianBitField<4>(txbf_cap) {}
  constexpr TxBfCapability() = default;

  WLAN_BIT_FIELD(as_uint32, 0, 32)  // Read/write as a uint32_t

  WLAN_BIT_FIELD(implicit_rx, 0, 1)
  WLAN_BIT_FIELD(rx_stag_sounding, 1, 1)
  WLAN_BIT_FIELD(tx_stag_sounding, 2, 1)
  WLAN_BIT_FIELD(rx_ndp, 3, 1)
  WLAN_BIT_FIELD(tx_ndp, 4, 1)
  WLAN_BIT_FIELD(implicit, 5, 1)
  WLAN_BIT_FIELD(calibration, 6, 2)
  WLAN_BIT_FIELD(csi, 8, 1)  // Explicit CSI Transmit Beamforming.

  WLAN_BIT_FIELD(noncomp_steering, 9, 1)  // Explicit Noncompressed Steering
  WLAN_BIT_FIELD(comp_steering, 10, 1)    // Explicit Compressed Steering
  WLAN_BIT_FIELD(csi_feedback, 11, 2)
  WLAN_BIT_FIELD(noncomp_feedback, 13, 2)
  WLAN_BIT_FIELD(comp_feedback, 15, 2)
  WLAN_BIT_FIELD(min_grouping, 17, 2)
  WLAN_BIT_FIELD(csi_antennas, 19, 2)

  WLAN_BIT_FIELD(noncomp_steering_ants, 21, 2)
  WLAN_BIT_FIELD(comp_steering_ants, 23, 2)
  WLAN_BIT_FIELD(csi_rows, 25, 2)
  WLAN_BIT_FIELD(chan_estimation, 27, 2)
  WLAN_BIT_FIELD(reserved, 29, 3)

  enum Calibration {
    CALIBRATION_NONE = 0,
    CALIBRATION_RESPOND_NOINITIATE = 1,
    CALIBRATION_RESERVED = 2,
    CALIBRATION_RESPOND_INITIATE = 3,
  };

  enum Feedback {
    // Shared for csi_feedback, noncomp_feedback, comp_feedback
    FEEDBACK_NONE = 0,
    FEEDBACK_DELAYED = 1,
    FEEDBACK_IMMEDIATE = 2,
    FEEDBACK_DELAYED_IMMEDIATE = 3,
  };

  enum MinGroup {
    MIN_GROUP_ONE = 0,  // Meaning no grouping
    MIN_GROUP_ONE_TWO = 1,
    MIN_GROUP_ONE_FOUR = 2,
    MIN_GROUP_ONE_TWO_FOUR = 3,
  };

  uint8_t csi_antennas_human() const { return csi_antennas() + 1; }
  void set_csi_antennas_human(uint8_t num) {
    constexpr uint8_t kLowerbound = 1;
    constexpr uint8_t kUpperbound = 4;
    if (num < kLowerbound)
      num = kLowerbound;
    if (num > kUpperbound)
      num = kUpperbound;
    set_csi_antennas(num - 1);
  }

  uint8_t noncomp_steering_ants_human() const { return noncomp_steering_ants() + 1; }
  void set_noncomp_steering_ants_human(uint8_t num) {
    constexpr uint8_t kLowerbound = 1;
    constexpr uint8_t kUpperbound = 4;
    if (num < kLowerbound)
      num = kLowerbound;
    if (num > kUpperbound)
      num = kUpperbound;
    set_noncomp_steering_ants(num - 1);
  }

  uint8_t comp_steering_ants_human() const { return comp_steering_ants() + 1; }
  void set_comp_steering_ants_human(uint8_t num) {
    constexpr uint8_t kLowerbound = 1;
    constexpr uint8_t kUpperbound = 4;
    if (num < kLowerbound)
      num = kLowerbound;
    if (num > kUpperbound)
      num = kUpperbound;
    set_comp_steering_ants(num - 1);
  }

  uint8_t csi_rows_human() const { return csi_rows() + 1; }
  void set_csi_rows_human(uint8_t num) {
    constexpr uint8_t kLowerbound = 1;
    constexpr uint8_t kUpperbound = 4;
    if (num < kLowerbound)
      num = kLowerbound;
    if (num > kUpperbound)
      num = kUpperbound;
    set_csi_rows(num - 1);
  }

  uint8_t chan_estimation_human() const { return chan_estimation() + 1; }
  void set_chan_estimation_human(uint8_t num) {
    constexpr uint8_t kLowerbound = 1;
    constexpr uint8_t kUpperbound = 4;
    if (num < kLowerbound)
      num = kLowerbound;
    if (num > kUpperbound)
      num = kUpperbound;
    set_chan_estimation(num - 1);
  }

} __PACKED;

class AselCapability : public common::BitField<uint8_t> {
 public:
  constexpr explicit AselCapability(uint8_t asel_cap) : common::BitField<uint8_t>(asel_cap) {}
  constexpr AselCapability() = default;

  WLAN_BIT_FIELD(asel, 0, 1)
  WLAN_BIT_FIELD(csi_feedback_tx_asel, 1,
                 1)  // Explicit CSI Feedback based Transmit ASEL
  WLAN_BIT_FIELD(ant_idx_feedback_tx_asel, 2, 1)
  WLAN_BIT_FIELD(explicit_csi_feedback, 3, 1)
  WLAN_BIT_FIELD(antenna_idx_feedback, 4, 1)
  WLAN_BIT_FIELD(rx_asel, 5, 1)
  WLAN_BIT_FIELD(tx_sounding_ppdu, 6, 1)
  WLAN_BIT_FIELD(reserved, 7, 1)

} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.56
struct HtCapabilities {
  HtCapabilityInfo ht_cap_info;
  AmpduParams ampdu_params;
  SupportedMcsSet mcs_set;
  HtExtCapabilities ht_ext_cap;
  TxBfCapability txbf_cap;
  AselCapability asel_cap;

  static HtCapabilities* ViewFromRawBytes(uint8_t bytes[fuchsia::wlan::ieee80211::HT_CAP_LEN]) {
    static_assert(sizeof(HtCapabilities) == fuchsia::wlan::ieee80211::HT_CAP_LEN);
    return reinterpret_cast<HtCapabilities*>(bytes);
  }

} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.158.2
// Note this is a field of VhtCapabilities element
struct VhtCapabilitiesInfo : public common::LittleEndianBitField<4> {
 public:
  constexpr explicit VhtCapabilitiesInfo(std::array<uint8_t, 4> vht_cap_info)
      : common::LittleEndianBitField<4>(vht_cap_info) {}
  constexpr VhtCapabilitiesInfo() = default;

  WLAN_BIT_FIELD(as_uint32, 0, 32)  // Read/write as a uint32_t

  WLAN_BIT_FIELD(max_mpdu_len, 0, 2)

  // Supported channel width set. See IEEE Std 802.11-2016, Table 9-250.
  WLAN_BIT_FIELD(supported_cbw_set, 2, 2)

  WLAN_BIT_FIELD(rx_ldpc, 4, 1)
  WLAN_BIT_FIELD(sgi_cbw80, 5, 1)   // CBW80 only
  WLAN_BIT_FIELD(sgi_cbw160, 6, 1)  // CBW160 and CBW80P80
  WLAN_BIT_FIELD(tx_stbc, 7, 1)
  WLAN_BIT_FIELD(rx_stbc, 8, 3)
  WLAN_BIT_FIELD(su_bfer, 11, 1)       // Single user beamformer capable
  WLAN_BIT_FIELD(su_bfee, 12, 1)       // Single user beamformee capable
  WLAN_BIT_FIELD(bfee_sts, 13, 3)      // Beamformee Space-time spreading
  WLAN_BIT_FIELD(num_sounding, 16, 3)  // number of sounding dimensions
  WLAN_BIT_FIELD(mu_bfer, 19, 1)       // Multi user beamformer capable
  WLAN_BIT_FIELD(mu_bfee, 20, 1)       // Multi user beamformee capable
  WLAN_BIT_FIELD(txop_ps, 21, 1)       // Txop power save mode
  WLAN_BIT_FIELD(htc_vht, 22, 1)
  WLAN_BIT_FIELD(max_ampdu_exp, 23, 3)
  WLAN_BIT_FIELD(link_adapt, 26, 2)  // VHT link adaptation capable
  WLAN_BIT_FIELD(rx_ant_pattern, 28, 1)
  WLAN_BIT_FIELD(tx_ant_pattern, 29, 1)

  // Extended number of spatial stream bandwidth supported
  // See IEEE Std 80.211-2016, Table 9-250.
  WLAN_BIT_FIELD(ext_nss_bw, 30, 2)

  enum MaxMpduLen {
    OCTETS_3895 = 0,
    OCTETS_7991 = 1,
    OCTETS_11454 = 2,
    // 3 reserved
  };

  enum VhtLinkAdaptation {
    LINK_ADAPT_NO_FEEDBACK = 0,
    // 1 reserved
    LINK_ADAPT_UNSOLICITED = 2,
    LINK_ADAPT_BOTH = 3,
  };

} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.158.3
struct VhtMcsNss : public common::LittleEndianBitField<8> {
 public:
  constexpr explicit VhtMcsNss(std::array<uint8_t, 8> vht_mcs_nss)
      : common::LittleEndianBitField<8>(vht_mcs_nss) {}
  constexpr VhtMcsNss() = default;

  WLAN_BIT_FIELD(as_uint64, 0, 64)  // Read/write as a uint64_t

  // Rx VHT-MCS Map
  WLAN_BIT_FIELD(rx_max_mcs_ss1, 0, 2)
  WLAN_BIT_FIELD(rx_max_mcs_ss2, 2, 2)
  WLAN_BIT_FIELD(rx_max_mcs_ss3, 4, 2)
  WLAN_BIT_FIELD(rx_max_mcs_ss4, 6, 2)
  WLAN_BIT_FIELD(rx_max_mcs_ss5, 8, 2)
  WLAN_BIT_FIELD(rx_max_mcs_ss6, 10, 2)
  WLAN_BIT_FIELD(rx_max_mcs_ss7, 12, 2)
  WLAN_BIT_FIELD(rx_max_mcs_ss8, 14, 2)

  WLAN_BIT_FIELD(rx_max_data_rate, 16, 13)
  WLAN_BIT_FIELD(max_nsts, 29, 3)

  // Tx VHT-MCS Map
  WLAN_BIT_FIELD(tx_max_mcs_ss1, 32, 2)
  WLAN_BIT_FIELD(tx_max_mcs_ss2, 34, 2)
  WLAN_BIT_FIELD(tx_max_mcs_ss3, 36, 2)
  WLAN_BIT_FIELD(tx_max_mcs_ss4, 38, 2)
  WLAN_BIT_FIELD(tx_max_mcs_ss5, 40, 2)
  WLAN_BIT_FIELD(tx_max_mcs_ss6, 42, 2)
  WLAN_BIT_FIELD(tx_max_mcs_ss7, 44, 2)
  WLAN_BIT_FIELD(tx_max_mcs_ss8, 46, 2)
  WLAN_BIT_FIELD(tx_max_data_rate, 48, 13)

  WLAN_BIT_FIELD(ext_nss_bw, 61, 1)
  // bit 62, 63 reserved

  enum VhtMcsSet {
    VHT_MCS_0_TO_7 = 0,
    VHT_MCS_0_TO_8 = 1,
    VHT_MCS_0_TO_9 = 2,
    VHT_MCS_NONE = 3,
  };

  uint8_t get_rx_max_mcs_ss(uint8_t ss_num) const {
    ZX_DEBUG_ASSERT(1 <= ss_num && ss_num <= 8);
    constexpr uint8_t kMcsBitOffset = 0;  // rx_max_mcs_ss1
    constexpr uint8_t kBitWidth = 2;
    uint8_t offset = kMcsBitOffset + (ss_num - 1) * kBitWidth;
    uint64_t mask = ((1ull << kBitWidth) - 1) << offset;
    return static_cast<uint8_t>((as_uint64() & mask) >> offset);
  }

  uint8_t get_tx_max_mcs_ss(uint8_t ss_num) const {
    ZX_DEBUG_ASSERT(1 <= ss_num && ss_num <= 8);
    constexpr uint8_t kMcsBitOffset = 32;  // tx_max_mcs_ss1
    constexpr uint8_t kBitWidth = 2;
    uint8_t offset = kMcsBitOffset + (ss_num - 1) * kBitWidth;
    uint64_t mask = ((1ull << kBitWidth) - 1) << offset;
    return static_cast<uint8_t>((as_uint64() & mask) >> offset);
  }

  void set_rx_max_mcs_ss(uint8_t ss_num, uint8_t mcs) {
    ZX_DEBUG_ASSERT(1 <= ss_num && ss_num <= 8);
    constexpr uint8_t kMcsBitOffset = 0;  // rx_max_mcs_ss1
    constexpr uint8_t kBitWidth = 2;
    uint8_t offset = kMcsBitOffset + (ss_num - 1) * kBitWidth;
    uint64_t mcs_val = static_cast<uint64_t>(mcs) << offset;
    set_as_uint64(as_uint64() | mcs_val);
  }

  void set_tx_max_mcs_ss(uint8_t ss_num, uint8_t mcs) {
    ZX_DEBUG_ASSERT(1 <= ss_num && ss_num <= 8);
    constexpr uint8_t kMcsBitOffset = 32;  // tx_max_mcs_ss1
    constexpr uint8_t kBitWidth = 2;
    uint8_t offset = kMcsBitOffset + (ss_num - 1) * kBitWidth;
    uint64_t mcs_val = static_cast<uint64_t>(mcs) << offset;
    set_as_uint64(as_uint64() | mcs_val);
  }

} __PACKED;

// IEEE Std 802.11-2016, 9.4.2.158
struct VhtCapabilities {
  VhtCapabilitiesInfo vht_cap_info;
  VhtMcsNss vht_mcs_nss;

  static VhtCapabilities* ViewFromRawBytes(uint8_t bytes[fuchsia::wlan::ieee80211::VHT_CAP_LEN]) {
    static_assert(sizeof(VhtCapabilities) == fuchsia::wlan::ieee80211::VHT_CAP_LEN);
    return reinterpret_cast<VhtCapabilities*>(bytes);
  }

} __PACKED;

// IEEE Std 802.11-2016, Figure 9-562
struct BasicVhtMcsNss : public common::BitField<uint16_t> {
 public:
  constexpr explicit BasicVhtMcsNss(uint16_t basic_mcs) : common::BitField<uint16_t>(basic_mcs) {}
  constexpr BasicVhtMcsNss() = default;

  WLAN_BIT_FIELD(ss1, 0, 2)
  WLAN_BIT_FIELD(ss2, 2, 2)
  WLAN_BIT_FIELD(ss3, 4, 2)
  WLAN_BIT_FIELD(ss4, 6, 2)
  WLAN_BIT_FIELD(ss5, 8, 2)
  WLAN_BIT_FIELD(ss6, 10, 2)
  WLAN_BIT_FIELD(ss7, 12, 2)
  WLAN_BIT_FIELD(ss8, 14, 2)

  enum VhtMcsEncoding {
    VHT_MCS_0_TO_7 = 0,
    VHT_MCS_0_TO_8 = 1,
    VHT_MCS_0_TO_9 = 2,
    VHT_MCS_NONE = 3,
  };

  uint8_t get_max_mcs_ss(uint8_t ss_num) const {
    ZX_DEBUG_ASSERT(1 <= ss_num && ss_num <= 8);
    constexpr uint8_t kMcsBitOffset = 0;  // ss1
    constexpr uint8_t kBitWidth = 2;
    uint8_t offset = kMcsBitOffset + (ss_num - 1) * kBitWidth;
    uint64_t mask = ((1ull << kBitWidth) - 1) << offset;
    return static_cast<uint8_t>((val() & mask) >> offset);
  }

  void set_max_mcs_ss(uint8_t ss_num, uint8_t mcs) {
    ZX_DEBUG_ASSERT(1 <= ss_num && ss_num <= 8);
    constexpr uint8_t kMcsBitOffset = 0;  // ss1
    constexpr uint8_t kBitWidth = 2;
    uint8_t offset = kMcsBitOffset + (ss_num - 1) * kBitWidth;
    uint64_t mcs_val = static_cast<uint64_t>(mcs) << offset;
    set_val(static_cast<uint16_t>(val() | mcs_val));
  }
};

}  // namespace wlan

#endif  // SRC_CONNECTIVITY_WLAN_LIB_COMMON_CPP_INCLUDE_WLAN_COMMON_ELEMENT_H_
