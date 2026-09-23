/*
 * Copyright 2019 The Fuchsia Authors.
 *
 * Permission to use, copy, modify, and/or distribute this software for any
 * purpose with or without fee is hereby granted, provided that the above
 * copyright notice and this permission notice appear in all copies.
 *
 * THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 * WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
 * MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
 * ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 * WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
 * ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
 * OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */

// TODO(29700): Consolidate to one ieee80211.h

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_IEEE80211_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_IEEE80211_H_

#include <netinet/if_ether.h>
#include <stddef.h>
#include <stdint.h>

#include "third_party/driver-lib/wlan/ieee80211.h"
#include "third_party/iwlwifi/platform/banjo/ieee80211.h"
#include "third_party/iwlwifi/platform/banjo/softmac.h"
#include "third_party/iwlwifi/platform/compiler.h"

#if defined(__cplusplus)
extern "C" {
#endif  // defined(__cplusplus)

// The below constants are not defined in the 802.11-2016 Std.
#define IEEE80211_MAX_CHAINS 4
#define IEEE80211_MAX_RTS_THRESHOLD 2353
#define IEEE80211_MAC_PACKET_HEADROOM_SIZE 8

// Used as the default value in the data structure to indicate the queue is not set yet.
#define IEEE80211_INVAL_HW_QUEUE 0xff

// Convert the TID sequence number into the SSN (start sequence number) in the BAR (Block Ack
// Request).
#define IEEE80211_SCTL_SEQ_MASK 0xfff
#define IEEE80211_SCTL_SEQ_OFFSET 4
#define IEEE80211_SEQ_TO_SN(seq) (((seq) >> IEEE80211_SCTL_SEQ_OFFSET) & IEEE80211_SCTL_SEQ_MASK)

/* 802.11n HT capabilities masks (for cap_info) */
#define IEEE80211_HT_CAP_LDPC_CODING 0x0001
#define IEEE80211_HT_CAP_SUP_WIDTH_20_40 0x0002
#define IEEE80211_HT_CAP_SM_PS 0x000C
#define IEEE80211_HT_CAP_SM_PS_SHIFT 2
#define IEEE80211_HT_CAP_GRN_FLD 0x0010
#define IEEE80211_HT_CAP_SGI_20 0x0020
#define IEEE80211_HT_CAP_SGI_40 0x0040
#define IEEE80211_HT_CAP_TX_STBC 0x0080
#define IEEE80211_HT_CAP_RX_STBC 0x0300
#define IEEE80211_HT_CAP_RX_STBC_SHIFT 8
#define IEEE80211_HT_CAP_DELAY_BA 0x0400
#define IEEE80211_HT_CAP_MAX_AMSDU 0x0800
#define IEEE80211_HT_CAP_DSSSCCK40 0x1000
#define IEEE80211_HT_CAP_RESERVED 0x2000
#define IEEE80211_HT_CAP_40MHZ_INTOLERANT 0x4000
#define IEEE80211_HT_CAP_LSIG_TXOP_PROT 0x8000

/* 802.11n HT capability MSC set */
#define IEEE80211_HT_MCS_RX_HIGHEST_MASK 0x3ff
#define IEEE80211_HT_MCS_TX_DEFINED 0x01
#define IEEE80211_HT_MCS_TX_RX_DIFF 0x02

#define IEEE80211_HT_MCS_TX_MAX_STREAMS_MASK 0x0C
#define IEEE80211_HT_MCS_TX_MAX_STREAMS_SHIFT 2
#define IEEE80211_HT_MCS_TX_MAX_STREAMS 4
#define IEEE80211_HT_MCS_TX_UNEQUAL_MODULATION 0x10

/* A-PMDU buffer sizes, which varies from 8KB to 1MB */
#define IEEE80211_MIN_AMPDU_BUF 0x8
#define IEEE80211_MAX_AMPDU_BUF_HT 0x40
#define IEEE80211_MAX_AMPDU_BUF_HE 0x100
#define IEEE80211_MAX_AMPDU_BUF_EHT 0x400

// Ids of information elements referred in this driver.
#define WLAN_EID_SSID 0

// The maximum length for SSID string.
//
// It is equal to fuchsia_wlan_ieee80211::wire::kMaxSsidByteLen, but
// we cannot use C++ syntax because this file is included by c files as well.
#define IEEE80211_MAX_SSID_LEN 32

// The length of PN in the CCMP header.
//
#define IEEE80211_CCMP_PN_LEN (fuchsia_wlan_ieee80211_CCMP_PN_LEN)

// The length for array size.
#define IEEE80211_MAX_PN_LEN 16

// The offset in the TX TKPI key array.
#define NL80211_TKIP_DATA_OFFSET_TX_MIC_KEY 16

// The offset in the RX TKPI key array.
#define NL80211_TKIP_DATA_OFFSET_RX_MIC_KEY 24

// The number of TID.
#define IEEE80211_NUM_TIDS (fuchsia_wlan_ieee80211_TIDS_MAX)

// The order of access categories is not clearly specified in 802.11-2016 Std.
// Therefore it cannot be moved into ieee80211 banjo file.
enum ieee80211_ac_numbers {
  IEEE80211_AC_VO = 0,
  IEEE80211_AC_VI = 1,
  IEEE80211_AC_BE = 2,
  IEEE80211_AC_BK = 3,
  IEEE80211_AC_MAX = 4,
};

enum ieee80211_frame_release_type {
  IEEE80211_FRAME_RELEASE_PSPOLL,
  IEEE80211_FRAME_RELEASE_UAPSD,
};

// IEEE Std 802.11-2016, 9.4.2.56.3, Table 9-163
enum ieee80211_max_ampdu_length_exp {
  IEEE80211_HT_MAX_AMPDU_8K = 0,
  IEEE80211_HT_MAX_AMPDU_16K = 1,
  IEEE80211_HT_MAX_AMPDU_32K = 2,
  IEEE80211_HT_MAX_AMPDU_64K = 3
};

/* Minimum MPDU start spacing */
enum ieee80211_min_mpdu_spacing {
  IEEE80211_HT_MPDU_DENSITY_NONE = 0, /* No restriction */
  IEEE80211_HT_MPDU_DENSITY_0_25 = 1, /* 1/4 usec */
  IEEE80211_HT_MPDU_DENSITY_0_5 = 2,  /* 1/2 usec */
  IEEE80211_HT_MPDU_DENSITY_1 = 3,    /* 1 usec */
  IEEE80211_HT_MPDU_DENSITY_2 = 4,    /* 2 usec */
  IEEE80211_HT_MPDU_DENSITY_4 = 5,    /* 4 usec */
  IEEE80211_HT_MPDU_DENSITY_8 = 6,    /* 8 usec */
  IEEE80211_HT_MPDU_DENSITY_16 = 7    /* 16 usec */
};

enum ieee80211_roc_type {
  IEEE80211_ROC_TYPE_NORMAL = 0,
  IEEE80211_ROC_TYPE_MGMT_TX,
};

enum ieee80211_rssi_event_data {
  RSSI_EVENT_HIGH,
  RSSI_EVENT_LOW,
};

enum ieee80211_smps_mode {
  IEEE80211_SMPS_AUTOMATIC,
  IEEE80211_SMPS_OFF,
  IEEE80211_SMPS_STATIC,
  IEEE80211_SMPS_DYNAMIC,
  IEEE80211_SMPS_NUM_MODES,
};

// NEEDS_PORTING: Below structures are only referenced in function prototype.
//                Doesn't need a dummy byte.
struct cfg80211_gtk_rekey_data;
struct cfg80211_nan_conf;
struct cfg80211_nan_func;
struct cfg80211_scan_request;
struct cfg80211_sched_scan_request;
struct cfg80211_wowlan;
struct ieee80211_key_conf;
struct ieee80211_sta_ht_cap;
struct ieee80211_scan_ies;
struct ieee80211_tdls_ch_sw_params;

// NEEDS_PORTING: Below structures are used in code but not ported yet.
// A dummy byte is required to suppress the C++ warning message for empty
// struct.
struct ieee80211_hdr {
  char dummy;
};

struct ieee80211_ops {
  char dummy;
};

struct ieee80211_p2p_noa_desc {
  char dummy;
};

// This is similar with 'wlan_band_t'.
enum nl80211_band {
  NL80211_BAND_2GHZ,
  NL80211_BAND_5GHZ,
  NL80211_BAND_6GHZ,
  NUM_NL80211_BANDS,
};

static inline enum nl80211_band convert_wlan_band_to_nl80211_band(wlan_band_t band) {
  switch (band) {
    case WLAN_BAND_TWO_GHZ:
      return NL80211_BAND_2GHZ;
    case WLAN_BAND_FIVE_GHZ:
      return NL80211_BAND_5GHZ;
    default:
      return NUM_NL80211_BANDS;
  }
}

// Channel info. Attributes of a channel.
struct ieee80211_channel {
  enum nl80211_band band;  // This value can be obtained from convert_wlan_band_to_nl80211_band(
                           // iwl_mvm_get_channel_band(ch_num)).
  uint32_t center_freq;    // unit: MHz.
  uint16_t ch_num;         // channel number (starts from 1).  TODO(fxbug.dev/119415): remove this.
  uint16_t hw_value;       // channel number (starts from 1). From wlan_channel_t.primary.
  uint32_t flags;
  int max_power;
};

// This is similar with 'channel_bandwidth_t'.
enum nl80211_chan_width {
  NL80211_CHAN_WIDTH_20_NOHT,
  NL80211_CHAN_WIDTH_20,
  NL80211_CHAN_WIDTH_40,
  NL80211_CHAN_WIDTH_80,
  NL80211_CHAN_WIDTH_80P80,
  NL80211_CHAN_WIDTH_160,
  NL80211_CHAN_WIDTH_5,
  NL80211_CHAN_WIDTH_10,

  // Below value should be used in the test case.
  NL80211_CHAN_WIDTH_40_BELOW,
};

static inline channel_bandwidth_t convert_nl80211_chan_width_to_channel_bandwidth(
    enum nl80211_chan_width width) {
  switch (width) {
    case NL80211_CHAN_WIDTH_20:
      return CHANNEL_BANDWIDTH_CBW20;
    case NL80211_CHAN_WIDTH_40:
      return CHANNEL_BANDWIDTH_CBW40;
    case NL80211_CHAN_WIDTH_40_BELOW:
      return CHANNEL_BANDWIDTH_CBW40BELOW;
    case NL80211_CHAN_WIDTH_80:
      return CHANNEL_BANDWIDTH_CBW80;
    case NL80211_CHAN_WIDTH_160:
      return CHANNEL_BANDWIDTH_CBW160;
    case NL80211_CHAN_WIDTH_80P80:
      return CHANNEL_BANDWIDTH_CBW80P80;
    default:
      break;
  }
  return CHANNEL_BANDWIDTH_CBW20;
}

static inline enum nl80211_chan_width convert_channel_bandwidth_to_nl80211_chan_width(
    channel_bandwidth_t cbw) {
  switch (cbw) {
    case CHANNEL_BANDWIDTH_CBW20:
      return NL80211_CHAN_WIDTH_20;
    case CHANNEL_BANDWIDTH_CBW40:
      return NL80211_CHAN_WIDTH_40;
    case CHANNEL_BANDWIDTH_CBW40BELOW:
      return NL80211_CHAN_WIDTH_40_BELOW;
    case CHANNEL_BANDWIDTH_CBW80:
      return NL80211_CHAN_WIDTH_80;
    case CHANNEL_BANDWIDTH_CBW160:
      return NL80211_CHAN_WIDTH_160;
    case CHANNEL_BANDWIDTH_CBW80P80:
      return NL80211_CHAN_WIDTH_80P80;
    default:
      break;
  }
  return NL80211_CHAN_WIDTH_20_NOHT;
}

// This can be converted from the `wlan_channel_t` structure.
struct cfg80211_chan_def {
  struct ieee80211_channel* chan;  // .hw_value = wlan_channel_t.primary
  enum nl80211_chan_width width;   // = wlan_channel_t.bandwidth
};

struct ieee80211_mcs_info {
  uint8_t rx_mask[IEEE80211_HT_MCS_MASK_LEN];
  __le16 rx_highest_le;
  uint8_t tx_params;
  uint8_t reserved[3];
} __packed;

struct ieee80211_sta_ht_cap {
  uint16_t cap; /* use IEEE80211_HT_CAP_ */
  bool ht_supported;
  uint8_t ampdu_factor;
  uint8_t ampdu_density;
  struct ieee80211_mcs_info mcs;
};

struct ieee80211_supported_band {
  wlan_band_t band;
  struct ieee80211_channel* channels;
  size_t n_channels;
  uint16_t* bitrates;
  size_t n_bitrates;
  struct ieee80211_sta_ht_cap ht_cap;
};

struct ieee80211_tx_queue_params {
  uint16_t txop;
  uint16_t cw_min;
  uint16_t cw_max;
  uint8_t aifs;
};

struct ieee80211_tx_rate {
  char dummy;
};

struct ieee80211_txq;
struct ieee80211_sta {
  void* drv_priv;
  struct ieee80211_txq* txq[fuchsia_wlan_ieee80211_TIDS_MAX + 1];
};

struct ieee80211_txq {
  void* drv_priv;
};

struct cfg80211_ssid {
  u8 ssid[IEEE80211_MAX_SSID_LEN];
  u8 ssid_len;
};

enum nl80211_iftype {
  NL80211_IFTYPE_UNSPECIFIED,
  NL80211_IFTYPE_STATION,
  NL80211_IFTYPE_P2P_DEVICE,
};

struct ieee80211_vif {
  enum nl80211_iftype type;
  void* drv_priv;
};

enum nl80211_scan_flags {
  NL80211_SCAN_FLAG_LOW_PRIORITY = 1 << 0,
  NL80211_SCAN_FLAG_FLUSH = 1 << 1,
  NL80211_SCAN_FLAG_AP = 1 << 2,
  NL80211_SCAN_FLAG_RANDOM_ADDR = 1 << 3,
  NL80211_SCAN_FLAG_FILS_MAX_CHANNEL_TIME = 1 << 4,
  NL80211_SCAN_FLAG_ACCEPT_BCAST_PROBE_RESP = 1 << 5,
  NL80211_SCAN_FLAG_OCE_PROBE_REQ_HIGH_TX_RATE = 1 << 6,
  NL80211_SCAN_FLAG_OCE_PROBE_REQ_DEFERRAL_SUPPRESSION = 1 << 7,
  NL80211_SCAN_FLAG_LOW_SPAN = 1 << 8,
  NL80211_SCAN_FLAG_LOW_POWER = 1 << 9,
  NL80211_SCAN_FLAG_HIGH_ACCURACY = 1 << 10,
  NL80211_SCAN_FLAG_RANDOM_SN = 1 << 11,
  NL80211_SCAN_FLAG_MIN_PREQ_CONTENT = 1 << 12,
  NL80211_SCAN_FLAG_FREQ_KHZ = 1 << 13,
  NL80211_SCAN_FLAG_COLOCATED_6GHZ = 1 << 14,
};

/**
 * struct ieee80211_key_conf - HW key configuration data
 * @tx_pn - TX packet number, in host byte order
 * @rx_seq - RX sequence number, in host byte order
 */
struct ieee80211_key_conf {
  atomic64_t tx_pn;
  uint64_t rx_seq;
  uint32_t cipher;
  uint8_t hw_key_idx;
  uint8_t keyidx;
  uint8_t key_type;
  size_t keylen;
  uint8_t key[0];
};

struct ieee80211_tx_info {
  struct {
    struct ieee80211_key_conf* hw_key;
  } control;
};

// Struct for transferring an IEEE 802.11 MAC-framed packet around the driver.
struct ieee80211_mac_packet {
  // The common portion of the MAC header.
  const struct ieee80211_frame_header* common_header;

  // The size of the entire MAC header (starting at common_header), including variable fields.
  size_t header_size;

  // Statically allocated headroom space between the MAC header and frame body, for adding
  // additional headers to the packet.
  uint8_t headroom[IEEE80211_MAC_PACKET_HEADROOM_SIZE];

  // Size of the headroom used.
  size_t headroom_used_size;

  // MAC frame body.
  const uint8_t* body;

  // MAC frame body size.
  size_t body_size;

  // Control information for this packet.
  struct ieee80211_tx_info info;
};

// Flags for the ieee80211_rx_status.flag
enum ieee80211_rx_status_flags {
  RX_FLAG_DECRYPTED = 0x1,
  RX_FLAG_PN_VALIDATED = 0x2,
  RX_FLAG_ALLOW_SAME_PN = 0x4,
};

struct ieee80211_rx_status {
  // RX flags, as in Linux.
  uint64_t flag;

  // The encryption IV, copied here since the encryption header is removed for Fuchsia.
  uint8_t extiv[8];

  // RX info struct to pass to wlanstack.
  struct wlan_rx_info rx_info;
};

size_t ieee80211_get_header_len(const struct ieee80211_frame_header* fh);

struct ieee80211_hw* ieee80211_alloc_hw(size_t priv_data_len, const struct ieee80211_ops* ops);

struct wlan_channel_number {
  wlan_band_t band;
  uint8_t number;
};

bool ieee80211_is_valid_chan(struct wlan_channel_number primary);

uint16_t ieee80211_get_center_freq(struct wlan_channel_number channel_num);

bool ieee80211_has_protected(const struct ieee80211_frame_header* fh);

bool ieee80211_is_data(const struct ieee80211_frame_header* fh);

bool ieee80211_is_data_present(const struct ieee80211_frame_header* fh);

bool ieee80211_is_data_qos(const struct ieee80211_frame_header* fh);

uint8_t ieee80211_get_tid(const struct ieee80211_frame_header* fh);

bool ieee80211_is_back_req(const struct ieee80211_frame_header* fh);

static inline uint16_t ieee80211_get_qos_ctl(const struct ieee80211_frame_header* fh) {
  uint8_t* p = (uint8_t*)fh;
  size_t offset = ieee80211_get_qos_ctrl_offset(fh);
  return *((uint16_t*)(p + offset));
}

#if defined(__cplusplus)
}  // extern "C"
#endif  // defined(__cplusplus)

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_IEEE80211_H_
