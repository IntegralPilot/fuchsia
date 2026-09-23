// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/designware-dsi/dsi-host-controller.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/mipi-dsi/mipi-dsi.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <fbl/auto_lock.h>
#include <fbl/string_buffer.h>

#include "src/graphics/display/lib/designware-dsi/dphy-interface-config.h"
#include "src/graphics/display/lib/designware-dsi/dpi-interface-config.h"
#include "src/graphics/display/lib/designware-dsi/dpi-video-timing.h"
#include "src/graphics/display/lib/designware-dsi/dsi-host-controller-config.h"
#include "src/graphics/display/lib/designware-dsi/dsi-packet-handler-config.h"
#include "src/graphics/display/lib/designware-dsi/dw-mipi-dsi-reg.h"

// Header Creation Macros
#define GEN_HDR_WC_MSB(x) ((x & 0xFF) << 16)
#define GEN_HDR_WC_LSB(x) ((x & 0xFF) << 8)
#define GEN_HDR_VC(x) ((x & 0x03) << 6)
#define GEN_HDR_DT(x) ((x & 0x3F) << 0)

namespace designware_dsi {

namespace {
constexpr uint32_t kPowerReset = 0;
constexpr uint32_t kPowerOn = 1;
constexpr uint32_t kPhyTestCtrlSet = 0x2;
constexpr uint32_t kPhyTestCtrlClr = 0x0;

constexpr uint32_t kDPhyTimeout = 200000;
constexpr uint32_t kPhyDelay = 6;
constexpr uint32_t kPhyStopWaitTime = 0x28;  // value from vendor

// Generic retry value used for BTA and FIFO related events
constexpr uint32_t kRetryMax = 20000;

constexpr uint32_t kMaxPldFifoDepth = 200;

constexpr uint32_t kBitPldRFull = 5;
constexpr uint32_t kBitPldREmpty = 4;
constexpr uint32_t kBitPldWFull = 3;
constexpr uint32_t kBitPldWEmpty = 2;
constexpr uint32_t kBitCmdFull = 1;
constexpr uint32_t kBitCmdEmpty = 0;

void LogBytes(std::span<const uint8_t> bytes) {
  if (bytes.empty()) {
    return;
  }

  static constexpr size_t kByteCountPerPrint = 16;
  // Stores up to `kByteCountPerPrint` bytes in "0x01,0x02,..." format
  static constexpr size_t kStringBufferSize = kByteCountPerPrint * 5 + 1;
  fbl::StringBuffer<kStringBufferSize> string_buffer;
  for (size_t i = 0; i < bytes.size(); i++) {
    if (i % kByteCountPerPrint == 0 && i != 0) {
      fdf::info("{}", string_buffer.c_str());
      string_buffer.Clear();
    }
    string_buffer.AppendPrintf("0x%02x,", bytes[i]);
  }
  if (!string_buffer.empty()) {
    fdf::info("{}", string_buffer.c_str());
  }
}

}  // namespace

DsiHostController::DsiHostController(fdf::MmioBuffer dsi_mmio) : dsi_mmio_(std::move(dsi_mmio)) {}

void DsiHostController::PowerUp() {
  DsiDwPwrUpReg::Get().ReadFrom(&dsi_mmio_).set_shutdown(kPowerOn).WriteTo(&dsi_mmio_);
}

void DsiHostController::PowerDown() {
  DsiDwPwrUpReg::Get().ReadFrom(&dsi_mmio_).set_shutdown(kPowerReset).WriteTo(&dsi_mmio_);
}

void DsiHostController::PhySendCode(uint32_t code, uint32_t parameter) {
  // Write code
  DsiDwPhyTstCtrl1Reg::Get().FromValue(0).set_reg_value(code).WriteTo(&dsi_mmio_);

  // Toggle PhyTestClk
  DsiDwPhyTstCtrl0Reg::Get().FromValue(0).set_reg_value(kPhyTestCtrlSet).WriteTo(&dsi_mmio_);
  DsiDwPhyTstCtrl0Reg::Get().FromValue(0).set_reg_value(kPhyTestCtrlClr).WriteTo(&dsi_mmio_);

  // Write parameter
  DsiDwPhyTstCtrl1Reg::Get().FromValue(0).set_reg_value(parameter).WriteTo(&dsi_mmio_);

  // Toggle PhyTestClk
  DsiDwPhyTstCtrl0Reg::Get().FromValue(0).set_reg_value(kPhyTestCtrlSet).WriteTo(&dsi_mmio_);
  DsiDwPhyTstCtrl0Reg::Get().FromValue(0).set_reg_value(kPhyTestCtrlClr).WriteTo(&dsi_mmio_);
}

void DsiHostController::PhyPowerUp() {
  DsiDwPhyRstzReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_phy_forcepll(1)
      .set_phy_enableclk(1)
      .set_phy_rstz(1)
      .set_phy_shutdownz(1)
      .WriteTo(&dsi_mmio_);
}

void DsiHostController::PhyPowerDown() {
  DsiDwPhyRstzReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_phy_rstz(0)
      .set_phy_shutdownz(0)
      .WriteTo(&dsi_mmio_);
}

zx_status_t DsiHostController::PhyWaitForReady() {
  int timeout = kDPhyTimeout;
  while ((DsiDwPhyStatusReg::Get().ReadFrom(&dsi_mmio_).phy_lock() == 0) && timeout--) {
    zx_nanosleep(zx_deadline_after(ZX_USEC(kPhyDelay)));
  }
  if (timeout <= 0) {
    fdf::error("Timed out waiting for D-PHY lock");
    return ZX_ERR_TIMED_OUT;
  }

  timeout = kDPhyTimeout;
  while ((DsiDwPhyStatusReg::Get().ReadFrom(&dsi_mmio_).phy_stopstateclklane() == 0) && timeout--) {
    zx_nanosleep(zx_deadline_after(ZX_USEC(kPhyDelay)));
  }
  if (timeout <= 0) {
    fdf::error("Timed out waiting for D-PHY StopStateClk to be set");
    return ZX_ERR_TIMED_OUT;
  }
  return ZX_OK;
}

zx::result<> DsiHostController::IssueCommands(
    std::span<const mipi_dsi::DsiCommandAndResponse> commands) {
  for (const mipi_dsi::DsiCommandAndResponse& command : commands) {
    zx_status_t status = IssueCommand(command);
    if (status != ZX_OK) {
      fdf::error("Failed to issue a command: {}", zx::make_result(status));
      return zx::error(status);
    }
  }
  return zx::ok();
}

void DsiHostController::SetMode(mipi_dsi::DsiOperationMode operation_mode) {
  DsiDwModeCfgReg::Get().ReadFrom(&dsi_mmio_).SetOperationMode(operation_mode).WriteTo(&dsi_mmio_);
}

zx::result<> DsiHostController::Config(const DsiHostControllerConfig& config) {
  if (!config.IsValid()) {
    fdf::error("Invalid DsiHostControllerConfig provided");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Enable LP transmission in CMD Mode
  DsiDwCmdModeCfgReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_max_rd_pkt_size(1)
      .set_dcs_lw_tx(1)
      .set_dcs_sr_0p_tx(1)
      .set_dcs_sw_1p_tx(1)
      .set_dcs_sw_0p_tx(1)
      .set_gen_lw_tx(1)
      .set_gen_sr_2p_tx(1)
      .set_gen_sr_1p_tx(1)
      .set_gen_sr_0p_tx(1)
      .set_gen_sw_2p_tx(1)
      .set_gen_sw_1p_tx(1)
      .set_gen_sw_0p_tx(1)
      .WriteTo(&dsi_mmio_);

  // Packet header settings - Enable CRC and ECC. BTA will be enabled based on CMD
  DsiDwPckhdlCfgReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_crc_rx_en(1)
      .set_ecc_rx_en(1)
      .WriteTo(&dsi_mmio_);

  // DesignWare DSI Host Setup based on MIPI DSI Host Controller User Guide (Sec 3.1.1)

  // 1. Global configuration: Lane number and PHY stop wait time
  DsiDwPhyIfCfgReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_phy_stop_wait_time(kPhyStopWaitTime)
      .set_n_lanes(config.dphy_interface_config.data_lane_count - 1)
      .WriteTo(&dsi_mmio_);

  // 2.1 Configure virtual channel
  DsiDwDpiVcidReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_dpi_vcid(kMipiDsiVirtualChanId)
      .WriteTo(&dsi_mmio_);

  // 2.2, Configure Color format
  const bool pixel_stream_packet_format_is_18bit_loosely_packed =
      config.dsi_packet_handler_config.pixel_stream_packet_format ==
      mipi_dsi::DsiPixelStreamPacketFormat::k18BitR6G6B6LooselyPacked;
  DsiDwDpiColorCodingReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_loosely18_en(pixel_stream_packet_format_is_18bit_loosely_packed)
      .SetColorComponentMapping(config.dpi_interface_config.color_component_mapping)
      .WriteTo(&dsi_mmio_);
  // 2.3 Configure Signal polarity - Keep as default
  DsiDwDpiCfgPolReg::Get().FromValue(0).set_reg_value(0).WriteTo(&dsi_mmio_);

  // The following values are relevent for video mode
  // 3.1 Configure low power transitions and video mode type

  // Note: If the lp_cmd_en bit of the VID_MODE_CFG register is 0, the commands are sent
  // in high-speed in Video Mode. In this case, the DWC_mipi_dsi_host automatically determines
  // the area where each command can be sent and no programming or calculation is required.

  DsiDwVidModeCfgReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_vpg_en(0)
      .set_lp_cmd_en(0)
      .set_frame_bta_ack_en(1)
      .set_lp_hfp_en(1)
      .set_lp_hbp_en(1)
      .set_lp_vact_en(1)
      .set_lp_vfp_en(1)
      .set_lp_vbp_en(1)
      .set_lp_vsa_en(1)
      .SetVideoModePacketSequencing(config.dsi_packet_handler_config.packet_sequencing)
      .WriteTo(&dsi_mmio_);

  // Define the max pkt size during Low Power mode
  DsiDwDpiLpCmdTimReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_outvact_lpcmd_time(config.dpi_interface_config.low_power_command_timer_config
                                  .max_vertical_blank_escape_mode_command_size_bytes)
      .set_invact_lpcmd_time(config.dpi_interface_config.low_power_command_timer_config
                                 .max_vertical_active_escape_mode_command_size_bytes)
      .WriteTo(&dsi_mmio_);

  // 3.2   Configure video packet size settings
  DsiDwVidPktSizeReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_vid_pkt_size(config.dpi_interface_config.video_timing.horizontal_active_px)
      .WriteTo(&dsi_mmio_);

  // Disable sending vid in chunk since they are ignored by DW host IP in burst mode
  DsiDwVidNumChunksReg::Get().FromValue(0).set_reg_value(0).WriteTo(&dsi_mmio_);
  DsiDwVidNullSizeReg::Get().FromValue(0).set_reg_value(0).WriteTo(&dsi_mmio_);

  // 4 Configure the video relative parameters according to the output type
  const display::DisplayTiming& video_timing = config.dpi_interface_config.video_timing;
  const int64_t dphy_data_lane_bytes_per_second =
      config.dphy_interface_config.high_speed_mode_data_lane_bytes_per_second();

  const int32_t horizontal_sync_width_duration_lane_byte_clock_cycles =
      DpiPixelToDphyLaneByteClockCycle(video_timing.horizontal_sync_width_px,
                                       video_timing.pixel_clock_frequency_hz,
                                       dphy_data_lane_bytes_per_second);
  DsiDwVidHsaTimeReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_vid_hsa_time(horizontal_sync_width_duration_lane_byte_clock_cycles)
      .WriteTo(&dsi_mmio_);

  const int32_t horizontal_back_porch_duration_lane_byte_clock_cycles =
      DpiPixelToDphyLaneByteClockCycle(video_timing.horizontal_back_porch_px,
                                       video_timing.pixel_clock_frequency_hz,
                                       dphy_data_lane_bytes_per_second);
  DsiDwVidHbpTimeReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_vid_hbp_time(horizontal_back_porch_duration_lane_byte_clock_cycles)
      .WriteTo(&dsi_mmio_);

  const int32_t horizontal_total_duration_lane_byte_clock_cycles = DpiPixelToDphyLaneByteClockCycle(
      video_timing.horizontal_total_px(), video_timing.pixel_clock_frequency_hz,
      dphy_data_lane_bytes_per_second);
  DsiDwVidHlineTimeReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_vid_hline_time(horizontal_total_duration_lane_byte_clock_cycles)
      .WriteTo(&dsi_mmio_);

  DsiDwVidVsaLinesReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_vsa_lines(video_timing.vertical_sync_width_lines)
      .WriteTo(&dsi_mmio_);

  DsiDwVidVbpLinesReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_vbp_lines(video_timing.vertical_back_porch_lines)
      .WriteTo(&dsi_mmio_);

  DsiDwVidVactiveLinesReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_vactive_lines(video_timing.vertical_active_lines)
      .WriteTo(&dsi_mmio_);

  DsiDwVidVfpLinesReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_vfp_lines(video_timing.vertical_front_porch_lines)
      .WriteTo(&dsi_mmio_);

  // Internal dividers to divide lanebyteclk for timeout purposes

  // The quotient is guaranteed to be >= 1 and <= 255. Thus it can be cast
  // to an int32_t.
  const int32_t escape_clock_divider = static_cast<int32_t>(
      config.dphy_interface_config.high_speed_mode_data_lane_bytes_per_second() /
      config.dphy_interface_config.escape_mode_clock_lane_frequency_hz);
  DsiDwClkmgrCfgReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_to_clk_div(1)
      .set_tx_esc_clk_div(escape_clock_divider)
      .WriteTo(&dsi_mmio_);

  // Setup Phy Timers as provided by vendor
  DsiDwPhyTmrLpclkCfgReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_phy_clkhs2lp_time(
          config.dphy_interface_config
              .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles)
      .set_phy_clklp2hs_time(
          config.dphy_interface_config
              .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles)
      .WriteTo(&dsi_mmio_);
  DsiDwPhyTmrCfgReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_phy_hs2lp_time(config.dphy_interface_config
                              .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles)
      .set_phy_lp2hs_time(config.dphy_interface_config
                              .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles)
      .WriteTo(&dsi_mmio_);

  DsiDwLpclkCtrlReg::Get()
      .ReadFrom(&dsi_mmio_)
      .set_auto_clklane_ctrl(config.dphy_interface_config.clock_lane_mode_automatic_control_enabled)
      .set_phy_txrequestclkhs(1)
      .WriteTo(&dsi_mmio_);

  return zx::ok();
}

inline bool DsiHostController::IsPldREmpty() {
  return (DsiDwCmdPktStatusReg::Get().ReadFrom(&dsi_mmio_).gen_pld_r_empty() == 1);
}

inline bool DsiHostController::IsPldRFull() {
  return (DsiDwCmdPktStatusReg::Get().ReadFrom(&dsi_mmio_).gen_pld_r_full() == 1);
}

inline bool DsiHostController::IsPldWEmpty() {
  return (DsiDwCmdPktStatusReg::Get().ReadFrom(&dsi_mmio_).gen_pld_w_empty() == 1);
}

inline bool DsiHostController::IsPldWFull() {
  return (DsiDwCmdPktStatusReg::Get().ReadFrom(&dsi_mmio_).gen_pld_w_full() == 1);
}

inline bool DsiHostController::IsCmdEmpty() {
  return (DsiDwCmdPktStatusReg::Get().ReadFrom(&dsi_mmio_).gen_cmd_empty() == 1);
}

inline bool DsiHostController::IsCmdFull() {
  return (DsiDwCmdPktStatusReg::Get().ReadFrom(&dsi_mmio_).gen_cmd_full() == 1);
}

zx_status_t DsiHostController::WaitforFifo(uint32_t bit, bool val) {
  int retry = kRetryMax;
  while (((DsiDwCmdPktStatusReg::Get().ReadFrom(&dsi_mmio_).reg_value() >> bit) & 1) != val &&
         retry--) {
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
  }
  if (retry <= 0) {
    return ZX_ERR_TIMED_OUT;
  }
  return ZX_OK;
}

zx_status_t DsiHostController::WaitforPldWNotFull() { return WaitforFifo(kBitPldWFull, 0); }

zx_status_t DsiHostController::WaitforPldWEmpty() { return WaitforFifo(kBitPldWEmpty, 1); }

zx_status_t DsiHostController::WaitforPldRFull() { return WaitforFifo(kBitPldRFull, 1); }

zx_status_t DsiHostController::WaitforPldRNotEmpty() { return WaitforFifo(kBitPldREmpty, 0); }

zx_status_t DsiHostController::WaitforCmdNotFull() { return WaitforFifo(kBitCmdFull, 0); }

zx_status_t DsiHostController::WaitforCmdEmpty() { return WaitforFifo(kBitCmdEmpty, 1); }

void DsiHostController::LogCommand(const mipi_dsi::DsiCommandAndResponse& command) {
  fdf::info("MIPI DSI Outgoing Packet:");
  fdf::info("Virtual Channel ID = 0x{:x} ({})", command.virtual_channel_id,
            command.virtual_channel_id);
  fdf::info("Data Type = 0x{:x} ({})", command.data_type, command.data_type);
  fdf::info("Payload size = {}", command.payload.size());
  fdf::info("Payload Data: [");
  LogBytes(command.payload);
  fdf::info("]");
  fdf::info("Response payload size = {}", command.response_payload.size());
}

zx_status_t DsiHostController::GenericPayloadRead(uint32_t* data) {
  // make sure there is something valid to read from payload fifo
  if (WaitforPldRNotEmpty() != ZX_OK) {
    fdf::error("Timed out waiting for data in PLD R FIFO");
    return ZX_ERR_TIMED_OUT;
  }
  *data = DsiDwGenPldDataReg::Get().ReadFrom(&dsi_mmio_).reg_value();
  return ZX_OK;
}

zx_status_t DsiHostController::GenericHdrWrite(uint32_t data) {
  // make sure cmd fifo is not full before writing into it
  if (WaitforCmdNotFull() != ZX_OK) {
    fdf::error("Timed out waiting for CMD FIFO to not be full");
    return ZX_ERR_TIMED_OUT;
  }
  DsiDwGenHdrReg::Get().FromValue(0).set_reg_value(data).WriteTo(&dsi_mmio_);
  return ZX_OK;
}

zx_status_t DsiHostController::GenericPayloadWrite(uint32_t data) {
  // Make sure PLD_W is not full before writing into it
  if (WaitforPldWNotFull() != ZX_OK) {
    fdf::error("Timed out waiting for PLD W FIFO to not be full");
    return ZX_ERR_TIMED_OUT;
  }
  DsiDwGenPldDataReg::Get().FromValue(0).set_reg_value(data).WriteTo(&dsi_mmio_);
  return ZX_OK;
}

void DsiHostController::EnableBta() {
  // enable ack req after each packet transmission
  DsiDwCmdModeCfgReg::Get().ReadFrom(&dsi_mmio_).set_ack_rqst_en(1).WriteTo(&dsi_mmio_);
  // enable But Turn-Around request
  DsiDwPckhdlCfgReg::Get().ReadFrom(&dsi_mmio_).set_bta_en(1).WriteTo(&dsi_mmio_);
}

void DsiHostController::DisableBta() {
  // disable ack req after each packet transmission
  DsiDwCmdModeCfgReg::Get().ReadFrom(&dsi_mmio_).set_ack_rqst_en(0).WriteTo(&dsi_mmio_);

  // disable But Turn-Around request
  DsiDwPckhdlCfgReg::Get().ReadFrom(&dsi_mmio_).set_bta_en(0).WriteTo(&dsi_mmio_);
}

zx_status_t DsiHostController::WaitforBtaAck() {
  int retry = kRetryMax;
  while (DsiDwCmdPktStatusReg::Get().ReadFrom(&dsi_mmio_).gen_rd_cmd_busy() && retry--) {
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
  }
  if (retry <= 0) {
    fdf::error("Timed out waiting for read completion");
    return ZX_ERR_TIMED_OUT;
  }
  return ZX_OK;
}

// MIPI DSI Functions as implemented by DWC IP
zx_status_t DsiHostController::GenWriteShort(const mipi_dsi::DsiCommandAndResponse& command) {
  if (command.payload.size() > 2) {
    fdf::error("Invalid payload size ({}) for a Generic Short Write command",
               command.payload.size());
    return ZX_ERR_INVALID_ARGS;
  }
  if (command.data_type != kMipiDsiDtGenShortWrite0 &&
      command.data_type != kMipiDsiDtGenShortWrite1 &&
      command.data_type != kMipiDsiDtGenShortWrite2 &&
      command.data_type != kMipiDsiDtSetMaxRetPkt) {
    fdf::error("Invalid data type ({}) for Generic Short Write", command.data_type);
    return ZX_ERR_INVALID_ARGS;
  }

  uint32_t regVal = 0;
  regVal |= GEN_HDR_DT(command.data_type);
  regVal |= GEN_HDR_VC(command.virtual_channel_id);
  if (command.payload.size() >= 1) {
    regVal |= GEN_HDR_WC_LSB(command.payload[0]);
  }
  if (command.payload.size() == 2) {
    regVal |= GEN_HDR_WC_MSB(command.payload[1]);
  }

  return GenericHdrWrite(regVal);
}

zx_status_t DsiHostController::DcsWriteShort(const mipi_dsi::DsiCommandAndResponse& command) {
  // Check that the payload size and command match
  if (command.payload.size() != 1 && command.payload.size() != 2) {
    fdf::error("Invalid payload size ({}) for a DCS Short Write command", command.payload.size());
    return ZX_ERR_INVALID_ARGS;
  }
  if (command.data_type != kMipiDsiDtDcsShortWrite0 &&
      command.data_type != kMipiDsiDtDcsShortWrite1) {
    fdf::error("Invalid data type ({}) for Generic Short Write", command.data_type);
    return ZX_ERR_INVALID_ARGS;
  }

  uint32_t regVal = 0;
  regVal |= GEN_HDR_DT(command.data_type);
  regVal |= GEN_HDR_VC(command.virtual_channel_id);
  regVal |= GEN_HDR_WC_LSB(command.payload[0]);
  if (command.payload.size() == 2) {
    regVal |= GEN_HDR_WC_MSB(command.payload[1]);
  }

  return GenericHdrWrite(regVal);
}

// This function writes a generic long command. We can only write a maximum of FIFO_DEPTH
// to the payload fifo. This value is implementation specific.
zx_status_t DsiHostController::GenWriteLong(const mipi_dsi::DsiCommandAndResponse& command) {
  zx_status_t status = ZX_OK;
  uint32_t pld_data_idx = 0;  // payload data index
  uint32_t regVal = 0;

  ZX_DEBUG_ASSERT(command.payload.size() < kMaxPldFifoDepth);
  size_t ts = command.payload.size();  // initial transfer size

  // first write complete words
  while (ts >= 4) {
    regVal = command.payload[pld_data_idx + 0] << 0 | command.payload[pld_data_idx + 1] << 8 |
             command.payload[pld_data_idx + 2] << 16 | command.payload[pld_data_idx + 3] << 24;
    pld_data_idx += 4;

    status = GenericPayloadWrite(regVal);
    if (status != ZX_OK) {
      fdf::error("Generic Payload write failed: {}", zx::make_result(status));
      return status;
    }
    ts -= 4;
  }

  // Write remaining bytes
  if (ts > 0) {
    regVal = command.payload[pld_data_idx++] << 0;
    if (ts > 1) {
      regVal |= command.payload[pld_data_idx++] << 8;
    }
    if (ts > 2) {
      regVal |= command.payload[pld_data_idx++] << 16;
    }
    status = GenericPayloadWrite(regVal);
    if (status != ZX_OK) {
      fdf::error("Generic Payload write failed: {}", zx::make_result(status));
      return status;
    }
  }

  // At this point, we have written all of our mipi payload to FIFO. Let's transmit it
  regVal = 0;
  regVal |= GEN_HDR_DT(command.data_type);
  regVal |= GEN_HDR_VC(command.virtual_channel_id);
  regVal |= GEN_HDR_WC_LSB(static_cast<uint32_t>(command.payload.size()) & 0xFF);
  regVal |= GEN_HDR_WC_MSB((command.payload.size() & 0xFF00) >> 16);

  return GenericHdrWrite(regVal);
}

zx_status_t DsiHostController::GenRead(const mipi_dsi::DsiCommandAndResponse& command) {
  uint32_t regVal = 0;
  zx_status_t status = ZX_OK;

  // valid cmd packet
  if (command.payload.size() > 2) {
    fdf::error("Invalid payload size ({}) for a Generic Read command", command.payload.size());
    return ZX_ERR_INVALID_ARGS;
  }
  if (command.response_payload.empty()) {
    fdf::error("Response payload buffer is empty for a Generic Read command");
    return ZX_ERR_INVALID_ARGS;
  }

  regVal = 0;
  regVal |= GEN_HDR_DT(command.data_type);
  regVal |= GEN_HDR_VC(command.virtual_channel_id);
  if (command.payload.size() >= 1) {
    regVal |= GEN_HDR_WC_LSB(command.payload[0]);
  }
  if (command.payload.size() == 2) {
    regVal |= GEN_HDR_WC_MSB(command.payload[1]);
  }

  // Packet is ready. Let's enable BTA first
  EnableBta();

  if ((status = GenericHdrWrite(regVal)) != ZX_OK) {
    // no need to print extra error msg
    return status;
  }

  if ((status = WaitforBtaAck()) != ZX_OK) {
    // bta never returned. no need to print extra error msg
    return status;
  }

  // Got ACK. Let's start reading
  // We should only read rlen worth of DATA. Let's hope the device is not sending
  // more than it should.
  size_t ts = command.response_payload.size();
  uint32_t rsp_data_idx = 0;
  uint32_t data;
  while (ts >= 4) {
    status = GenericPayloadRead(&data);
    if (status != ZX_OK) {
      fdf::error("Failed to read payload data: {}", zx::make_result(status));
      return status;
    }
    command.response_payload[rsp_data_idx++] = static_cast<uint8_t>((data >> 0) & 0xFF);
    command.response_payload[rsp_data_idx++] = static_cast<uint8_t>((data >> 8) & 0xFF);
    command.response_payload[rsp_data_idx++] = static_cast<uint8_t>((data >> 16) & 0xFF);
    command.response_payload[rsp_data_idx++] = static_cast<uint8_t>((data >> 24) & 0xFF);
    ts -= 4;
  }

  // Read out remaining bytes
  if (ts > 0) {
    status = GenericPayloadRead(&data);
    if (status != ZX_OK) {
      fdf::error("Failed to read payload data: {}", zx::make_result(status));
      return status;
    }
    command.response_payload[rsp_data_idx++] = (data >> 0) & 0xFF;
    if (ts > 1) {
      command.response_payload[rsp_data_idx++] = (data >> 8) & 0xFF;
    }
    if (ts > 2) {
      command.response_payload[rsp_data_idx++] = (data >> 16) & 0xFF;
    }
  }

  // we are done. Display BTA
  DisableBta();
  return status;
}

zx_status_t DsiHostController::IssueCommand(const mipi_dsi::DsiCommandAndResponse& command) {
  fbl::AutoLock lock(&command_lock_);
  zx_status_t status = ZX_OK;

  switch (command.data_type) {
    case kMipiDsiDtGenShortWrite0:
    case kMipiDsiDtGenShortWrite1:
    case kMipiDsiDtGenShortWrite2:
    case kMipiDsiDtSetMaxRetPkt:
      status = GenWriteShort(command);
      break;
    case kMipiDsiDtGenLongWrite:
    case kMipiDsiDtDcsLongWrite:
      status = GenWriteLong(command);
      break;
    case kMipiDsiDtGenShortRead0:
    case kMipiDsiDtGenShortRead1:
    case kMipiDsiDtGenShortRead2:
    case kMipiDsiDtDcsRead0:
      status = GenRead(command);
      break;
    case kMipiDsiDtDcsShortWrite0:
    case kMipiDsiDtDcsShortWrite1:
      status = DcsWriteShort(command);
      break;
    default:
      fdf::error("Unsupported/Invalid DSI command data type: {}", command.data_type);
      status = ZX_ERR_INVALID_ARGS;
  }

  if (status != ZX_OK) {
    fdf::error("Failed to perform DSI command and/or response: {}", zx::make_result(status));
    LogCommand(command);
  }

  return status;
}

}  // namespace designware_dsi
