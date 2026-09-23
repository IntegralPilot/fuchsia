// Copyright 2023 The Fuchsia Authors. All rights reserved.

// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/video-input-regs.h"

#include <gtest/gtest.h>

namespace amlogic_display {

namespace {

TEST(WritebackMuxControl, GetSetMux0Selection) {
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux0Selection(WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kDisabled);
  }
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux0Selection(WritebackMuxSource::kEncoderInterlaced);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kEncoderInterlaced);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kEncoderInterlaced);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kEncoderInterlaced);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kDisabled);
  }
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux0Selection(WritebackMuxSource::kEncoderProgressive);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kEncoderProgressive);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kEncoderProgressive);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kEncoderProgressive);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kDisabled);
  }
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux0Selection(WritebackMuxSource::kEncoderTvPanel);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kEncoderTvPanel);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kEncoderTvPanel);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kEncoderTvPanel);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kDisabled);
  }
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux0Selection(WritebackMuxSource::kViuWriteback0);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kViuWriteback0);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kViuWriteback0);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kViuWriteback0);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kDisabled);
  }
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux0Selection(WritebackMuxSource::kViuWriteback1);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kViuWriteback1);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kViuWriteback1);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kViuWriteback1);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kDisabled);
  }
}

TEST(WritebackMuxControl, GetSetMux1Selection) {
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux1Selection(WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kDisabled);
  }
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux1Selection(WritebackMuxSource::kEncoderInterlaced);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kEncoderInterlaced);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kEncoderInterlaced);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kEncoderInterlaced);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kDisabled);
  }
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux1Selection(WritebackMuxSource::kEncoderProgressive);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kEncoderProgressive);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kEncoderProgressive);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kEncoderProgressive);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kDisabled);
  }
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux1Selection(WritebackMuxSource::kEncoderTvPanel);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kEncoderTvPanel);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kEncoderTvPanel);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kEncoderTvPanel);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kDisabled);
  }
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux1Selection(WritebackMuxSource::kViuWriteback0);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kViuWriteback0);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kViuWriteback0);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kViuWriteback0);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kDisabled);
  }
  {
    WritebackMuxControl reg = WritebackMuxControl::Get().FromValue(0);
    reg.SetMux1Selection(WritebackMuxSource::kViuWriteback1);
    EXPECT_EQ(reg.GetMux1Selection(), WritebackMuxSource::kViuWriteback1);
    EXPECT_EQ(reg.mux1_clock_selection(), WritebackMuxSource::kViuWriteback1);
    EXPECT_EQ(reg.mux1_data_selection(), WritebackMuxSource::kViuWriteback1);
    EXPECT_EQ(reg.GetMux0Selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_clock_selection(), WritebackMuxSource::kDisabled);
    EXPECT_EQ(reg.mux0_data_selection(), WritebackMuxSource::kDisabled);
  }
}

TEST(VideoInputInterfaceWidth, Width) {
  VideoInputInterfaceWidth video_input_interface_width =
      VideoInputInterfaceWidth::Get(VideoInputModuleId::kVideoInputModule0).FromValue(0);
  video_input_interface_width.set_width_px(0xfed);
  EXPECT_EQ(video_input_interface_width.width_minus_one_px(), 0xfecu);
  EXPECT_EQ(video_input_interface_width.width_px(), 0xfed);
}

TEST(VideoInputWriteRangeHorizontal, SetHorizontalRange) {
  VideoInputWriteRangeHorizontal video_input_write_range_horizontal =
      VideoInputWriteRangeHorizontal::Get(VideoInputModuleId::kVideoInputModule0).FromValue(0);
  video_input_write_range_horizontal.SetHorizontalRange(0xabc, 0xdef);
  EXPECT_EQ(video_input_write_range_horizontal.left_px_inclusive(), 0xabcu);
  EXPECT_EQ(video_input_write_range_horizontal.right_px_inclusive(), 0xdefu);
}

TEST(VideoInputWriteRangeVertical, SetVerticalRange) {
  VideoInputWriteRangeVertical video_input_write_range_vertical =
      VideoInputWriteRangeVertical::Get(VideoInputModuleId::kVideoInputModule0).FromValue(0);
  video_input_write_range_vertical.SetVerticalRange(0xabc, 0xdef);
  EXPECT_EQ(video_input_write_range_vertical.top_line_inclusive(), 0xabcu);
  EXPECT_EQ(video_input_write_range_vertical.bottom_line_inclusive(), 0xdefu);
}

}  // namespace

}  // namespace amlogic_display
