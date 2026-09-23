// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/registers-arbiter.h"

#include <gtest/gtest.h>
#include <hwreg/bitfields.h>

#include "src/graphics/display/drivers/intel-display/hardware-common.h"

namespace intel_display {

namespace {

TEST(ArbitrationControlTest, HighPriorityQueueWatermark) {
  auto arb_ctl = registers::ArbitrationControl::GetForArbiter(0).FromValue(0);

  // Example from the the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 51
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 13
  arb_ctl.set_reg_value(0).set_high_priority_queue_watermark(4);
  EXPECT_EQ(3u, arb_ctl.high_priority_queue_watermark_bits());
  EXPECT_EQ(4, arb_ctl.high_priority_queue_watermark());

  // Edge cases to catch any undefined behavior.

  arb_ctl.set_reg_value(0).set_high_priority_queue_watermark(1);
  EXPECT_EQ(0u, arb_ctl.high_priority_queue_watermark_bits());
  EXPECT_EQ(1, arb_ctl.high_priority_queue_watermark());

  arb_ctl.set_reg_value(0).set_high_priority_queue_watermark(2);
  EXPECT_EQ(1u, arb_ctl.high_priority_queue_watermark_bits());
  EXPECT_EQ(2, arb_ctl.high_priority_queue_watermark());

  arb_ctl.set_reg_value(0).set_high_priority_queue_watermark(7);
  EXPECT_EQ(6u, arb_ctl.high_priority_queue_watermark_bits());
  EXPECT_EQ(7, arb_ctl.high_priority_queue_watermark());

  arb_ctl.set_reg_value(0).set_high_priority_queue_watermark(8);
  EXPECT_EQ(7u, arb_ctl.high_priority_queue_watermark_bits());
  EXPECT_EQ(8, arb_ctl.high_priority_queue_watermark());
}

TEST(ArbitrationControlTest, LowPriorityBackToBackRequestLimit) {
  auto arb_ctl = registers::ArbitrationControl::GetForArbiter(0).FromValue(0);

  // The valid values and encodings are listed in the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 51
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 13

  arb_ctl.set_reg_value(0).set_low_priority_back_to_back_request_limit(1);
  EXPECT_EQ(0u, arb_ctl.low_priority_back_to_back_request_limit_bits());
  EXPECT_EQ(1, arb_ctl.low_priority_back_to_back_request_limit());

  arb_ctl.set_reg_value(0).set_low_priority_back_to_back_request_limit(2);
  EXPECT_EQ(1u, arb_ctl.low_priority_back_to_back_request_limit_bits());
  EXPECT_EQ(2, arb_ctl.low_priority_back_to_back_request_limit());

  arb_ctl.set_reg_value(0).set_low_priority_back_to_back_request_limit(4);
  EXPECT_EQ(2u, arb_ctl.low_priority_back_to_back_request_limit_bits());
  EXPECT_EQ(4, arb_ctl.low_priority_back_to_back_request_limit());

  arb_ctl.set_reg_value(0).set_low_priority_back_to_back_request_limit(8);
  EXPECT_EQ(3u, arb_ctl.low_priority_back_to_back_request_limit_bits());
  EXPECT_EQ(8, arb_ctl.low_priority_back_to_back_request_limit());
}

TEST(ArbitrationControlTest, GetForArbiter) {
  // The register MMIO addresses come from the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 51
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 13

  auto arb_ctl = registers::ArbitrationControl::GetForArbiter(0).FromValue(0);
  EXPECT_EQ(0x45000u, arb_ctl.reg_addr());

  auto arb_ctl_abox1 = registers::ArbitrationControl::GetForArbiter(1).FromValue(0);
  EXPECT_EQ(0x45800u, arb_ctl_abox1.reg_addr());

  auto arb_ctl_abox2 = registers::ArbitrationControl::GetForArbiter(2).FromValue(0);
  EXPECT_EQ(0x45808u, arb_ctl_abox2.reg_addr());
}

TEST(ArbitrationControl2Test, DisplayCaptureWriteRequestLimit) {
  auto arb_ctl2 = registers::ArbitrationControl2::GetForArbiter(0).FromValue(0);

  // The valid values and encodings are listed in the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 54
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 16

  arb_ctl2.set_reg_value(0).set_display_capture_write_request_limit(1);
  EXPECT_EQ(0u, arb_ctl2.display_capture_write_request_limit_bits());
  EXPECT_EQ(1, arb_ctl2.display_capture_write_request_limit());

  arb_ctl2.set_reg_value(0).set_display_capture_write_request_limit(2);
  EXPECT_EQ(1u, arb_ctl2.display_capture_write_request_limit_bits());
  EXPECT_EQ(2, arb_ctl2.display_capture_write_request_limit());

  arb_ctl2.set_reg_value(0).set_display_capture_write_request_limit(4);
  EXPECT_EQ(2u, arb_ctl2.display_capture_write_request_limit_bits());
  EXPECT_EQ(4, arb_ctl2.display_capture_write_request_limit());

  arb_ctl2.set_reg_value(0).set_display_capture_write_request_limit(8);
  EXPECT_EQ(3u, arb_ctl2.display_capture_write_request_limit_bits());
  EXPECT_EQ(8, arb_ctl2.display_capture_write_request_limit());
}

TEST(ArbitrationControl2Test, DisplayStateBufferWriteRequestLimitTigerLake) {
  auto arb_ctl2 = registers::ArbitrationControl2::GetForArbiter(0).FromValue(0);

  // The valid values and encodings are listed in the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 54
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 16

  arb_ctl2.set_reg_value(0).set_display_state_buffer_write_request_limit_tiger_lake(1);
  EXPECT_EQ(0u, arb_ctl2.display_state_buffer_write_request_limit_tiger_lake_bits());
  EXPECT_EQ(1, arb_ctl2.display_state_buffer_write_request_limit_tiger_lake());

  arb_ctl2.set_reg_value(0).set_display_state_buffer_write_request_limit_tiger_lake(2);
  EXPECT_EQ(1u, arb_ctl2.display_state_buffer_write_request_limit_tiger_lake_bits());
  EXPECT_EQ(2, arb_ctl2.display_state_buffer_write_request_limit_tiger_lake());

  arb_ctl2.set_reg_value(0).set_display_state_buffer_write_request_limit_tiger_lake(4);
  EXPECT_EQ(2u, arb_ctl2.display_state_buffer_write_request_limit_tiger_lake_bits());
  EXPECT_EQ(4, arb_ctl2.display_state_buffer_write_request_limit_tiger_lake());

  arb_ctl2.set_reg_value(0).set_display_state_buffer_write_request_limit_tiger_lake(8);
  EXPECT_EQ(3u, arb_ctl2.display_state_buffer_write_request_limit_tiger_lake_bits());
  EXPECT_EQ(8, arb_ctl2.display_state_buffer_write_request_limit_tiger_lake());
}

TEST(ArbitrationControl2Test, Par5RequestLimitTigerLake) {
  auto arb_ctl2 = registers::ArbitrationControl2::GetForArbiter(0).FromValue(0);

  // The valid values and encodings are listed in the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 55
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 17

  arb_ctl2.set_reg_value(0).set_par5_request_limit_tiger_lake(1);
  EXPECT_EQ(0u, arb_ctl2.par5_request_limit_tiger_lake_bits());
  EXPECT_EQ(1, arb_ctl2.par5_request_limit_tiger_lake());

  arb_ctl2.set_reg_value(0).set_par5_request_limit_tiger_lake(2);
  EXPECT_EQ(1u, arb_ctl2.par5_request_limit_tiger_lake_bits());
  EXPECT_EQ(2, arb_ctl2.par5_request_limit_tiger_lake());

  arb_ctl2.set_reg_value(0).set_par5_request_limit_tiger_lake(4);
  EXPECT_EQ(2u, arb_ctl2.par5_request_limit_tiger_lake_bits());
  EXPECT_EQ(4, arb_ctl2.par5_request_limit_tiger_lake());

  arb_ctl2.set_reg_value(0).set_par5_request_limit_tiger_lake(16);
  EXPECT_EQ(3u, arb_ctl2.par5_request_limit_tiger_lake_bits());
  EXPECT_EQ(16, arb_ctl2.par5_request_limit_tiger_lake());
}

TEST(ArbitrationControl2Test, FrameBufferCompressionRequestLimitTigerLake) {
  auto arb_ctl2 = registers::ArbitrationControl2::GetForArbiter(0).FromValue(0);

  // The valid values and encodings are listed in the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 55
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 17

  arb_ctl2.set_reg_value(0).set_framebuffer_compression_request_limit_tiger_lake(1);
  EXPECT_EQ(0u, arb_ctl2.framebuffer_compression_request_limit_tiger_lake_bits());
  EXPECT_EQ(1, arb_ctl2.framebuffer_compression_request_limit_tiger_lake());

  arb_ctl2.set_reg_value(0).set_framebuffer_compression_request_limit_tiger_lake(2);
  EXPECT_EQ(1u, arb_ctl2.framebuffer_compression_request_limit_tiger_lake_bits());
  EXPECT_EQ(2, arb_ctl2.framebuffer_compression_request_limit_tiger_lake());

  arb_ctl2.set_reg_value(0).set_framebuffer_compression_request_limit_tiger_lake(4);
  EXPECT_EQ(2u, arb_ctl2.framebuffer_compression_request_limit_tiger_lake_bits());
  EXPECT_EQ(4, arb_ctl2.framebuffer_compression_request_limit_tiger_lake());

  arb_ctl2.set_reg_value(0).set_framebuffer_compression_request_limit_tiger_lake(8);
  EXPECT_EQ(3u, arb_ctl2.framebuffer_compression_request_limit_tiger_lake_bits());
  EXPECT_EQ(8, arb_ctl2.framebuffer_compression_request_limit_tiger_lake());
}

TEST(ArbitrationControl2Test, MaxInflightLowPriorityReadRequestsTigerLake) {
  auto arb_ctl2 = registers::ArbitrationControl2::GetForArbiter(0).FromValue(0);

  // The valid values and encodings are listed in the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 56
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 18

  arb_ctl2.set_reg_value(0).set_max_inflight_low_priority_read_requests(1);
  EXPECT_EQ(0u, arb_ctl2.max_inflight_low_priority_read_requests_bits());
  EXPECT_EQ(1, arb_ctl2.max_inflight_low_priority_read_requests());

  arb_ctl2.set_reg_value(0).set_max_inflight_low_priority_read_requests(2);
  EXPECT_EQ(1u, arb_ctl2.max_inflight_low_priority_read_requests_bits());
  EXPECT_EQ(2, arb_ctl2.max_inflight_low_priority_read_requests());

  arb_ctl2.set_reg_value(0).set_max_inflight_low_priority_read_requests(3);
  EXPECT_EQ(2u, arb_ctl2.max_inflight_low_priority_read_requests_bits());
  EXPECT_EQ(3, arb_ctl2.max_inflight_low_priority_read_requests());

  arb_ctl2.set_reg_value(0).set_max_inflight_low_priority_read_requests(4);
  EXPECT_EQ(3u, arb_ctl2.max_inflight_low_priority_read_requests_bits());
  EXPECT_EQ(4, arb_ctl2.max_inflight_low_priority_read_requests());
}

TEST(ArbitrationControl2Test, MaxInflightHighPriorityReadRequestsTigerLake) {
  auto arb_ctl2 = registers::ArbitrationControl2::GetForArbiter(0).FromValue(0);

  // The valid values and encodings are listed in the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 56
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 18

  arb_ctl2.set_reg_value(0).set_max_inflight_high_priority_read_requests(128);
  EXPECT_EQ(0u, arb_ctl2.max_inflight_high_priority_read_requests_bits());
  EXPECT_EQ(128, arb_ctl2.max_inflight_high_priority_read_requests());

  arb_ctl2.set_reg_value(0).set_max_inflight_high_priority_read_requests(64);
  EXPECT_EQ(1u, arb_ctl2.max_inflight_high_priority_read_requests_bits());
  EXPECT_EQ(64, arb_ctl2.max_inflight_high_priority_read_requests());

  arb_ctl2.set_reg_value(0).set_max_inflight_high_priority_read_requests(32);
  EXPECT_EQ(2u, arb_ctl2.max_inflight_high_priority_read_requests_bits());
  EXPECT_EQ(32, arb_ctl2.max_inflight_high_priority_read_requests());

  arb_ctl2.set_reg_value(0).set_max_inflight_high_priority_read_requests(16);
  EXPECT_EQ(3u, arb_ctl2.max_inflight_high_priority_read_requests_bits());
  EXPECT_EQ(16, arb_ctl2.max_inflight_high_priority_read_requests());
}

TEST(ArbitrationControl2Test, RequestTransactionIdQueueWatermarkTigerLake) {
  auto arb_ctl2 = registers::ArbitrationControl2::GetForArbiter(0).FromValue(0);

  // The valid values and encodings are listed in the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 56
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 18

  arb_ctl2.set_reg_value(0).set_request_transaction_id_queue_watermark(8);
  EXPECT_EQ(0u, arb_ctl2.request_transaction_id_queue_watermark_bits());
  EXPECT_EQ(8, arb_ctl2.request_transaction_id_queue_watermark());

  arb_ctl2.set_reg_value(0).set_request_transaction_id_queue_watermark(16);
  EXPECT_EQ(1u, arb_ctl2.request_transaction_id_queue_watermark_bits());
  EXPECT_EQ(16, arb_ctl2.request_transaction_id_queue_watermark());

  arb_ctl2.set_reg_value(0).set_request_transaction_id_queue_watermark(32);
  EXPECT_EQ(2u, arb_ctl2.request_transaction_id_queue_watermark_bits());
  EXPECT_EQ(32, arb_ctl2.request_transaction_id_queue_watermark());
}

TEST(ArbitrationControl2Test, GetForArbiter) {
  // The register MMIO addresses come from the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 54
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 16

  auto arb_ctl2 = registers::ArbitrationControl2::GetForArbiter(0).FromValue(0);
  EXPECT_EQ(0x45004u, arb_ctl2.reg_addr());

  auto arb_ctl2_abox1 = registers::ArbitrationControl2::GetForArbiter(1).FromValue(0);
  EXPECT_EQ(0x45804u, arb_ctl2_abox1.reg_addr());

  auto arb_ctl2_abox2 = registers::ArbitrationControl2::GetForArbiter(2).FromValue(0);
  EXPECT_EQ(0x4580cu, arb_ctl2_abox2.reg_addr());
}

TEST(BandwidthBuddyControl, GetForArbiter) {
  // The register MMIO addresses come from the reference manuals.
  //
  // Rocket Lake: IHD-OS-RKL-Vol 2-7.22 page 30
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 171
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 136

  auto bw_buddy0_ctl = registers::BandwidthBuddyControl::GetForArbiter(0).FromValue(0);
  EXPECT_EQ(0x45130u, bw_buddy0_ctl.reg_addr());

  auto bw_buddy1_ctl = registers::BandwidthBuddyControl::GetForArbiter(1).FromValue(0);
  EXPECT_EQ(0x45140u, bw_buddy1_ctl.reg_addr());

  auto bw_buddy2_ctl = registers::BandwidthBuddyControl::GetForArbiter(2).FromValue(0);
  EXPECT_EQ(0x45150u, bw_buddy2_ctl.reg_addr());
}

TEST(BandwidthBuddyPageMask, GetForArbiter) {
  // The register MMIO addresses come from the reference manuals.
  //
  // Rocket Lake: IHD-OS-RKL-Vol 2-7.22 page 32
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 173
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 138

  auto bw_buddy0_page_mask = registers::BandwidthBuddyPageMask::GetForArbiter(0).FromValue(0);
  EXPECT_EQ(0x45134u, bw_buddy0_page_mask.reg_addr());

  auto bw_buddy1_page_mask = registers::BandwidthBuddyPageMask::GetForArbiter(1).FromValue(0);
  EXPECT_EQ(0x45144u, bw_buddy1_page_mask.reg_addr());

  auto bw_buddy2_page_mask = registers::BandwidthBuddyPageMask::GetForArbiter(2).FromValue(0);
  EXPECT_EQ(0x45154u, bw_buddy2_page_mask.reg_addr());
}

TEST(MbusArbiterBoxControlTest, GetForArbiter) {
  // The register MMIO addresses come from the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 2 page 8
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 page 8

  auto mbus_abox_ctl = registers::MbusArbiterBoxControl::GetForArbiter(0).FromValue(0);
  EXPECT_EQ(0x45038u, mbus_abox_ctl.reg_addr());

  auto mbus_abox1_ctl = registers::MbusArbiterBoxControl::GetForArbiter(1).FromValue(0);
  EXPECT_EQ(0x45048u, mbus_abox1_ctl.reg_addr());

  auto mbus_abox2_ctl = registers::MbusArbiterBoxControl::GetForArbiter(2).FromValue(0);
  EXPECT_EQ(0x4504cu, mbus_abox2_ctl.reg_addr());
}

TEST(MbusDisplayBufferBoxControlTest, GetForSlice) {
  // The register MMIO addresses come from the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 2 page 10
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 page 10

  auto mbus_bbox_ctl_s1 = registers::MbusDisplayBufferBoxControl::GetForSlice(0).FromValue(0);
  EXPECT_EQ(0x45040u, mbus_bbox_ctl_s1.reg_addr());

  auto mbus_bbox_ctl_s2 = registers::MbusDisplayBufferBoxControl::GetForSlice(1).FromValue(0);
  EXPECT_EQ(0x45044u, mbus_bbox_ctl_s2.reg_addr());
}

TEST(MbusPipeDataBoxControlTest, GetForPipe) {
  // The register MMIO addresses come from the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 2 page 12
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 page 12

  auto pipe_mbus_dbox_ctl_a =
      registers::MbusPipeDataBoxControl::GetForPipe(PipeId::PIPE_A).FromValue(0);
  EXPECT_EQ(0x7003cu, pipe_mbus_dbox_ctl_a.reg_addr());

  auto pipe_mbus_dbox_ctl_b =
      registers::MbusPipeDataBoxControl::GetForPipe(PipeId::PIPE_B).FromValue(0);
  EXPECT_EQ(0x7103cu, pipe_mbus_dbox_ctl_b.reg_addr());

  auto pipe_mbus_dbox_ctl_c =
      registers::MbusPipeDataBoxControl::GetForPipe(PipeId::PIPE_C).FromValue(0);
  EXPECT_EQ(0x7203cu, pipe_mbus_dbox_ctl_c.reg_addr());

  // TODO(https://fxbug.dev/42060657): Add a test for pipe D, when we support it.
  // The MMIO address is 0x7303c.
}

TEST(PipeArbiterControlTest, DisplayStreamBufferArbitrationInterval) {
  auto pipe_arb_ctl_a = registers::PipeArbiterControl::GetForPipe(PipeId::PIPE_A).FromValue(0);

  // The valid values and encodings are listed in the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 670
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 670

  pipe_arb_ctl_a.set_reg_value(0).set_display_stream_buffer_arbitration_interval(16);
  EXPECT_EQ(0u, pipe_arb_ctl_a.display_stream_buffer_arbitration_interval_bits());
  EXPECT_EQ(16, pipe_arb_ctl_a.display_stream_buffer_arbitration_interval());

  pipe_arb_ctl_a.set_reg_value(0).set_display_stream_buffer_arbitration_interval(32);
  EXPECT_EQ(1u, pipe_arb_ctl_a.display_stream_buffer_arbitration_interval_bits());
  EXPECT_EQ(32, pipe_arb_ctl_a.display_stream_buffer_arbitration_interval());

  pipe_arb_ctl_a.set_reg_value(0).set_display_stream_buffer_arbitration_interval(64);
  EXPECT_EQ(2u, pipe_arb_ctl_a.display_stream_buffer_arbitration_interval_bits());
  EXPECT_EQ(64, pipe_arb_ctl_a.display_stream_buffer_arbitration_interval());

  pipe_arb_ctl_a.set_reg_value(0).set_display_stream_buffer_arbitration_interval(128);
  EXPECT_EQ(3u, pipe_arb_ctl_a.display_stream_buffer_arbitration_interval_bits());
  EXPECT_EQ(128, pipe_arb_ctl_a.display_stream_buffer_arbitration_interval());
}

TEST(PipeArbiterControlTest, DisplayBufferRequestsPerStreamerRequest) {
  auto pipe_arb_ctl_a = registers::PipeArbiterControl::GetForPipe(PipeId::PIPE_A).FromValue(0);

  // The valid values and encodings are listed in the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 670
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 page 670

  pipe_arb_ctl_a.set_reg_value(0).set_display_buffer_requests_per_streamer_request(2);
  EXPECT_EQ(0u, pipe_arb_ctl_a.display_buffer_requests_per_streamer_request_bits());
  EXPECT_EQ(2, pipe_arb_ctl_a.display_buffer_requests_per_streamer_request());

  pipe_arb_ctl_a.set_reg_value(0).set_display_buffer_requests_per_streamer_request(4);
  EXPECT_EQ(1u, pipe_arb_ctl_a.display_buffer_requests_per_streamer_request_bits());
  EXPECT_EQ(4, pipe_arb_ctl_a.display_buffer_requests_per_streamer_request());

  pipe_arb_ctl_a.set_reg_value(0).set_display_buffer_requests_per_streamer_request(8);
  EXPECT_EQ(2u, pipe_arb_ctl_a.display_buffer_requests_per_streamer_request_bits());
  EXPECT_EQ(8, pipe_arb_ctl_a.display_buffer_requests_per_streamer_request());

  pipe_arb_ctl_a.set_reg_value(0).set_display_buffer_requests_per_streamer_request(16);
  EXPECT_EQ(3u, pipe_arb_ctl_a.display_buffer_requests_per_streamer_request_bits());
  EXPECT_EQ(16, pipe_arb_ctl_a.display_buffer_requests_per_streamer_request());
}

TEST(PipeArbiterControlTest, GetForPipe) {
  // The register MMIO addresses come from the reference manuals.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 2 page 669
  // DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 page 669

  auto pipe_arb_ctl_a = registers::PipeArbiterControl::GetForPipe(PipeId::PIPE_A).FromValue(0);
  EXPECT_EQ(0x70028u, pipe_arb_ctl_a.reg_addr());

  auto pipe_arb_ctl_b = registers::PipeArbiterControl::GetForPipe(PipeId::PIPE_B).FromValue(0);
  EXPECT_EQ(0x71028u, pipe_arb_ctl_b.reg_addr());

  auto pipe_arb_ctl_c = registers::PipeArbiterControl::GetForPipe(PipeId::PIPE_C).FromValue(0);
  EXPECT_EQ(0x72028u, pipe_arb_ctl_c.reg_addr());

  // TODO(https://fxbug.dev/42060657): Add a test for pipe D, when we support it.
  // The MMIO address is 0x73028.
}

}  // namespace

}  // namespace intel_display
