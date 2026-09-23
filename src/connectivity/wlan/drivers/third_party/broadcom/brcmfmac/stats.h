// Copyright (c) 2025 The Fuchsia Authors
//
// Permission to use, copy, modify, and/or distribute this software for any purpose with or without
// fee is hereby granted, provided that the above copyright notice and this permission notice
// appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
// SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
// AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
// NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE
// OF THIS SOFTWARE.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_STATS_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_STATS_H_

#include "fidl/fuchsia.wlan.stats/cpp/wire_types.h"

class CounterConfig {
 public:
  constexpr CounterConfig(uint16_t id, const char* name) : id_(id), name_(name) {}
  fuchsia_wlan_stats::wire::InspectCounterConfig toFidl(fidl::AnyArena& arena) const {
    return fuchsia_wlan_stats::wire::InspectCounterConfig::Builder(arena)
        .counter_id(id_)
        .counter_name(name_)
        .Build();
  }
  fuchsia_wlan_stats::wire::UnnamedCounter unnamed(uint64_t count) const {
    return fuchsia_wlan_stats::wire::UnnamedCounter{.id = id_, .count = count};
  }

 private:
  uint16_t id_;
  const char* name_;
};

struct CounterConfigs {
  static constexpr CounterConfig FW_RX_GOOD{1, "fw_rx_good"};
  static constexpr CounterConfig FW_RX_BAD{2, "fw_rx_bad"};
  static constexpr CounterConfig FW_RX_OCAST{3, "fw_rx_ocast"};
  static constexpr CounterConfig FW_TX_GOOD{4, "fw_tx_good"};
  static constexpr CounterConfig FW_TX_BAD{5, "fw_tx_bad"};

  static constexpr CounterConfig DRIVER_RX_GOOD{6, "driver_rx_good"};
  static constexpr CounterConfig DRIVER_RX_BAD{7, "driver_rx_bad"};
  static constexpr CounterConfig DRIVER_TX_TOTAL{8, "driver_tx_total"};
  static constexpr CounterConfig DRIVER_TX_CONF{9, "driver_tx_conf"};
  static constexpr CounterConfig DRIVER_TX_DROP{10, "driver_tx_drop"};
  static constexpr CounterConfig DRIVER_TX_BAD{11, "driver_tx_bad"};

  static constexpr CounterConfig WME_VO_RX_GOOD{12, "wme_vo_rx_good"};
  static constexpr CounterConfig WME_VO_RX_BAD{13, "wme_vo_rx_bad"};
  static constexpr CounterConfig WME_VO_TX_GOOD{14, "wme_vo_tx_good"};
  static constexpr CounterConfig WME_VO_TX_BAD{15, "wme_vo_tx_bad"};
  static constexpr CounterConfig WME_VI_RX_GOOD{16, "wme_vi_rx_good"};
  static constexpr CounterConfig WME_VI_RX_BAD{17, "wme_vi_rx_bad"};
  static constexpr CounterConfig WME_VI_TX_GOOD{18, "wme_vi_tx_good"};
  static constexpr CounterConfig WME_VI_TX_BAD{19, "wme_vi_tx_bad"};
  static constexpr CounterConfig WME_BE_RX_GOOD{20, "wme_be_rx_good"};
  static constexpr CounterConfig WME_BE_RX_BAD{21, "wme_be_rx_bad"};
  static constexpr CounterConfig WME_BE_TX_GOOD{22, "wme_be_tx_good"};
  static constexpr CounterConfig WME_BE_TX_BAD{23, "wme_be_tx_bad"};
  static constexpr CounterConfig WME_BK_RX_GOOD{24, "wme_bk_rx_good"};
  static constexpr CounterConfig WME_BK_RX_BAD{25, "wme_bk_rx_bad"};
  static constexpr CounterConfig WME_BK_TX_GOOD{26, "wme_bk_tx_good"};
  static constexpr CounterConfig WME_BK_TX_BAD{27, "wme_bk_tx_bad"};

  static constexpr CounterConfig FW_TX_RETRANSMITS{29, "fw_tx_retransmits"};
  static constexpr CounterConfig FW_TX_DATA_ERRORS{30, "fw_tx_data_errors"};
  static constexpr CounterConfig FW_TX_STATUS_ERRORS{31, "fw_tx_status_errors"};
  static constexpr CounterConfig FW_TX_NO_BUFFER{32, "fw_tx_no_buffer"};
  static constexpr CounterConfig FW_TX_RUNT_FRAMES{33, "fw_tx_runt_frames"};
  static constexpr CounterConfig FW_TX_UNDERFLOW{34, "fw_tx_underflow"};
  static constexpr CounterConfig FW_TX_PHY_ERRORS{35, "fw_tx_phy_errors"};
  static constexpr CounterConfig FW_TX_DOT11_FAILURES{36, "fw_tx_dot11_failures"};
  static constexpr CounterConfig FW_TX_NO_ASSOC{37, "fw_tx_no_assoc"};
  static constexpr CounterConfig FW_TX_NO_ACK{38, "fw_tx_no_ack"};
  static constexpr CounterConfig FW_RX_DATA_ERRORS{39, "fw_rx_data_errors"};
  static constexpr CounterConfig FW_RX_OVERFLOW{40, "fw_rx_overflow"};
  static constexpr CounterConfig FW_RX_NO_BUFFER{41, "fw_rx_no_buffer"};
  static constexpr CounterConfig FW_RX_RUNT_FRAMES{42, "fw_rx_runt_frames"};
  static constexpr CounterConfig FW_RX_FRAGMENTATION_ERRORS{43, "fw_rx_fragmentation_errors"};
  static constexpr CounterConfig FW_RX_BAD_PLCP{44, "fw_rx_bad_plcp"};
  static constexpr CounterConfig FW_RX_CRS_GLITCH{45, "fw_rx_crs_glitch"};
  static constexpr CounterConfig FW_RX_BAD_FCS{46, "fw_rx_bad_fcs"};
  static constexpr CounterConfig FW_RX_GIANT_FRAMES{47, "fw_rx_giant_frames"};
  static constexpr CounterConfig FW_RX_NO_SCB{48, "fw_rx_no_scb"};
  static constexpr CounterConfig FW_RX_BAD_SRC_MAC{49, "fw_rx_bad_src_mac"};
  static constexpr CounterConfig FW_RX_DECRYPT_FAILURES{50, "fw_rx_decrypt_failures"};
  static constexpr CounterConfig SDIO_FLOW_CONTROL_EVENTS{51, "sdio_flow_control_events"};
  static constexpr CounterConfig SDIO_TX_CTRL_FRAME_GOOD{52, "sdio_tx_ctrl_frame_good"};
  static constexpr CounterConfig SDIO_TX_CTRL_FRAME_BAD{53, "sdio_tx_ctrl_frame_bad"};
  static constexpr CounterConfig SDIO_RX_CTRL_FRAME_GOOD{54, "sdio_rx_ctrl_frame_good"};
  static constexpr CounterConfig SDIO_RX_CTRL_FRAME_BAD{55, "sdio_rx_ctrl_frame_bad"};
  static constexpr CounterConfig SDIO_RX_OUT_OF_BUFS{56, "sdio_rx_out_of_bufs"};
  static constexpr CounterConfig SDIO_INTERRUPTS{57, "sdio_interrupts"};
  static constexpr CounterConfig SDIO_RX_HEADERS_READ{58, "sdio_rx_headers_read"};
  static constexpr CounterConfig SDIO_RX_PACKETS_READ{59, "sdio_rx_packets_read"};
  static constexpr CounterConfig SDIO_TX_PACKETS_WRITE{60, "sdio_tx_packets_write"};
  static constexpr CounterConfig SDIO_TX_QUEUE_FULL_COUNT{61, "sdio_tx_queue_full_count"};
  static constexpr CounterConfig SDIO_TX_ENQUEUE_COUNT{62, "sdio_tx_enqueue_count"};
  static constexpr CounterConfig BT_COEX_WLAN_PREEMPT_COUNT{63, "bt_coex_wlan_preempt_count"};
};

template <size_t N>
class GaugeConfig {
 public:
  constexpr GaugeConfig(uint16_t id, const char* name,
                        const std::array<fuchsia_wlan_stats::wire::GaugeStatistic, N> statistics)
      : id_(id), name_(name), statistics_(statistics) {}
  fuchsia_wlan_stats::wire::InspectGaugeConfig toFidl(fidl::AnyArena& arena) const {
    return fuchsia_wlan_stats::wire::InspectGaugeConfig::Builder(arena)
        .gauge_id(id_)
        .gauge_name(name_)
        .statistics(fidl::VectorView<fuchsia_wlan_stats::wire::GaugeStatistic>(
            arena, statistics_.begin(), statistics_.end()))
        .Build();
  }
  fuchsia_wlan_stats::wire::UnnamedGauge unnamed(int64_t value) const {
    return fuchsia_wlan_stats::wire::UnnamedGauge{.id = id_, .value = value};
  }

 private:
  uint16_t id_;
  const char* name_;
  const std::array<fuchsia_wlan_stats::wire::GaugeStatistic, N> statistics_;
};

struct GaugeConfigs {
  static constexpr GaugeConfig<1> SDIO_TX_SEQ{
      1, "sdio_tx_seq", {fuchsia_wlan_stats::wire::GaugeStatistic::kLast}};
  static constexpr GaugeConfig<1> SDIO_TX_MAX{
      2, "sdio_tx_max", {fuchsia_wlan_stats::wire::GaugeStatistic::kLast}};
  static constexpr GaugeConfig<1> SDIO_TX_QUEUE_LEN{
      3, "sdio_tx_queue_len", {fuchsia_wlan_stats::wire::GaugeStatistic::kMean}};
  static constexpr GaugeConfig<1> SDIO_TX_QUEUE_0_LEN{
      4, "sdio_tx_queue_0_len", {fuchsia_wlan_stats::wire::GaugeStatistic::kMean}};
  static constexpr GaugeConfig<1> SDIO_TX_QUEUE_1_LEN{
      5, "sdio_tx_queue_1_len", {fuchsia_wlan_stats::wire::GaugeStatistic::kMean}};
  static constexpr GaugeConfig<1> SDIO_TX_QUEUE_2_LEN{
      6, "sdio_tx_queue_2_len", {fuchsia_wlan_stats::wire::GaugeStatistic::kMean}};
  static constexpr GaugeConfig<1> SDIO_TX_QUEUE_3_LEN{
      7, "sdio_tx_queue_3_len", {fuchsia_wlan_stats::wire::GaugeStatistic::kMean}};
};

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_STATS_H_
