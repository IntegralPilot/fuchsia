// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VIDEO_INPUT_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VIDEO_INPUT_REGS_H_

#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <cstdint>

#include <hwreg/bitfields.h>

namespace amlogic_display {

// There are two video input modules (VDIN) in Amlogic display engine that
// receive video from external input (e.g. BT.656) or internal input (e.g.
// internal VIU loopback), and write the data back to the DDR memory.
enum class VideoInputModuleId {
  kVideoInputModule0 = 0,
  kVideoInputModule1 = 1,
};

// VDIN0_COM_CTRL0, VDIN1_COM_CTRL0.
//
// A311D Datasheet, Section 10.2.3.42 VDIN, Pages 1086-1087, 1108-1109.
// S905D2 Datasheet, Section 7.2.3.41 VDIN, Pages 777, 801.
// S905D3 Datasheet, Section 8.2.3.42 VDIN, Pages 713-714, 736-737.
class VideoInputCommandControl : public hwreg::RegisterBase<VideoInputCommandControl, uint32_t> {
 public:
  enum class ComponentInput : uint32_t {
    kComponentInput0 = 0b00,
    kComponentInput1 = 0b01,
    kComponentInput2 = 0b10,
  };

  // Values for the `input_source_selection` field.
  enum class InputSource : uint32_t {
    kNoInput = 0,
    kMpegFromDram = 1,
    kBt656 = 2,
    kReservedForComponent = 3,
    kReservedForTvDecoder = 4,
    kReservedForHdmiRx = 5,
    kDigitalVideoInput = 6,

    // Source selected by `WritebackMuxControl`.
    //
    // This source is documented as "Internal loopback from VIU" on A311D,
    // S905D2, and T931. Experiments on VIM3 (A311D), Astro (S905D2) and
    // Sherlock (T931) confirmed that the behavior actually matches S905D3.
    kWritebackMux0 = 7,

    kReservedForMipiCsi2 = 8,

    // Source selected by `WritebackMuxControl`.
    //
    // This source is documented as "Reserved (ISP)" on A311D, S905D2, and T931.
    // Experiments on VIM3 (A311D), Astro (S905D2) and Sherlock (T931) confirmed
    // that the behavior actually matches S905D3.
    kWritebackMux1 = 9,

    kSecondBt656 = 10,
  };

  static hwreg::RegisterAddr<VideoInputCommandControl> Get(VideoInputModuleId video_input) {
    switch (video_input) {
      case VideoInputModuleId::kVideoInputModule0:
        return {0x1202 * sizeof(uint32_t)};
      case VideoInputModuleId::kVideoInputModule1:
        return {0x1302 * sizeof(uint32_t)};
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid video input module ID: %d", static_cast<int>(video_input));
  }

  DEF_BIT(31, bypass_mpeg_noise_reduction);
  DEF_BIT(30, mpeg_field_info);

  // Trigger a go_field (Vsync) pulse on the video input module when true is
  // written.
  DEF_BIT(29, trigger_go_field_pulse);

  // Trigger a go_line (Hsync) pulse on the video input module when true is
  // written.
  DEF_BIT(28, trigger_go_line_pulse);

  DEF_BIT(27, mpeg_go_field_input_signal_enabled);

  // Not documented for this register; for fields of the same name in other
  // registers (VD1_IF0_GEN_REG, DI_IF0_GEN_REG, etc.), `hold_lines` is the
  // number of lines to hold after go_field pulse and before the module is
  // enabled.
  DEF_FIELD(26, 20, hold_lines);

  // Whether the `go_field` pulse is delayed for the video input module.
  DEF_BIT(19, go_field_pulse_delayed);

  // Number of lines that `go_field` pulse is delayed, if
  // `go_field_pulse_delayed` is true.
  DEF_FIELD(18, 12, go_field_pulse_delay_lines);

  // Seems unused for internal loopback mode.
  DEF_ENUM_FIELD(ComponentInput, 11, 10, component2_output_selection);
  DEF_ENUM_FIELD(ComponentInput, 9, 8, component1_output_selection);
  DEF_ENUM_FIELD(ComponentInput, 7, 6, component0_output_selection);

  // Indicates whether the video input is cropped using a window specified by
  // `VDIN0/1_WIN_H_START_END` and `VDIN0/1_WIN_V_START_END` registers.
  DEF_BIT(5, video_input_cropped);

  DEF_BIT(4, video_input_enabled);

  // If the input source doesn't equal to any value specified in `InputSource`,
  // no input is provided to the video input module.
  DEF_ENUM_FIELD(InputSource, 3, 0, input_source_selection);

  VideoInputCommandControl& SetInputSource(InputSource input_source) {
    switch (input_source) {
      case InputSource::kNoInput:
      case InputSource::kMpegFromDram:
      case InputSource::kBt656:
      case InputSource::kReservedForComponent:
      case InputSource::kReservedForTvDecoder:
      case InputSource::kReservedForHdmiRx:
      case InputSource::kDigitalVideoInput:
      case InputSource::kWritebackMux0:
      case InputSource::kReservedForMipiCsi2:
      case InputSource::kWritebackMux1:
      case InputSource::kSecondBt656:
        return set_input_source_selection(input_source);
    }
    ZX_DEBUG_ASSERT_MSG(false, "Unsupported input source: %" PRIu32,
                        static_cast<uint32_t>(input_source));
    return set_input_source_selection(InputSource::kNoInput);
  }

  InputSource GetInputSource() const {
    InputSource input_source = input_source_selection();
    switch (input_source) {
      case InputSource::kNoInput:
      case InputSource::kMpegFromDram:
      case InputSource::kBt656:
      case InputSource::kReservedForComponent:
      case InputSource::kReservedForTvDecoder:
      case InputSource::kReservedForHdmiRx:
      case InputSource::kDigitalVideoInput:
      case InputSource::kWritebackMux0:
      case InputSource::kReservedForMipiCsi2:
      case InputSource::kWritebackMux1:
      case InputSource::kSecondBt656:
        return input_source;
    }
    return InputSource::kNoInput;
  }
};

// VDIN0_COM_STATUS0, VDIN1_COM_STATUS0
//
// This register is read-only.
//
// A311D Datasheet, Section 10.2.3.42 VDIN, Pages 1087, 1109
// S905D2 Datasheet, Section 7.2.3.41 VDIN, Pages 778, 802
// S905D3 Datasheet, Section 8.2.3.41 VDIN, Pages 714, 737
class VideoInputCommandStatus0 : public hwreg::RegisterBase<VideoInputCommandStatus0, uint32_t> {
 public:
  static hwreg::RegisterAddr<VideoInputCommandStatus0> Get(VideoInputModuleId video_input) {
    switch (video_input) {
      case VideoInputModuleId::kVideoInputModule0:
        return {0x1205 * sizeof(uint32_t)};
      case VideoInputModuleId::kVideoInputModule1:
        return {0x1305 * sizeof(uint32_t)};
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid video input module ID: %d", static_cast<int>(video_input));
  }

  // Bits 17-3` are defined differently for VDIN0_COM_STATUS0 and
  // VDIN0_COM_STATUS0, in all the datasheets mentioned above.
  //
  // VDIN0_COM_STATUS0 uses bit 17 as `vid_wr_pending_ddr_wrrsp`, bit 16
  // as `curr_pic_sec`, and bit 15 as `curr_pic_sec_sav`, bits 14-3 as
  // `lfifo_buf_cnt`.
  //
  // VDIN1_COM_STATUS0 uses bits 12-3 as `lfifo_buf_cnt` and bits 17-13
  // reserved.
  //
  // Since these fields are not currently used by the driver, we are omitting
  // these fields as reserved. Future drivers may need to fork the register
  // definitions or create helper functions to access the fields.

  // Indicates that the write of raw pixels from input source to RAM is done.
  //
  // Cleared by `clear_direct_write_done` bit in the `VideoInputWriteControl`
  // register.
  DEF_BIT(2, direct_write_done);

  // Indicates that the write of noise-reduced (NR) pixels from input source to
  // RAM is done.
  //
  // Cleared by `clear_noise_reduced_write_done` bit in the
  // `VideoInputWriteControl` register.
  DEF_BIT(1, noise_reduced_write_done);

  // Current field for interlaced input.
  //
  // For interlaced inputs, 0 means top field, 1 means bottom field.
  // This is not documented in Amlogic datasheets but appears in drivers.
  //
  // Unused for progressive inputs.
  DEF_BIT(0, current_field);
};

// There are multiple video input channels (VDIs, possibly shorthand for Video
// Data Input) available for video input modules (VDINs), numbered from VDI1 to
// VDI9. Each VDIN has one asynchronous FIFO (ASFIFO) for each VDI to receive
// pixels from.
//
// For each VDIN, there are four control registers (ASFIFO_CTRL0/1/2/3) to
// configure the way the VDIN reads pixels from a channel by setting the
// corresponding FIFO behaviors. The layout of the ASFIFO configuration fields
// is the same across all channels.
//
// Currently this driver only uses VDI 6 and VDI 8; control fields / registers
// for all the other VDI channels are not defined here.
//
// A311D Datasheet, Section 10.2.3.42 VDIN, Pages 1088-1090, 1106, 1110-1112,
// 1128.
// S905D2 Datasheet, Section 7.2.3.41 VDIN, Pages 779-781, 798-799, 802-805,
// 822.
// S905D3 Datasheet, Section 8.2.3.42 VDIN, Pages 715-717, 733, 738-740, 755.

// VDIN0_ASFIFO_CTRL2, VDIN1_ASFIFO_CTRL2
//
// A311D Datasheet, Section 10.2.3.42 VDIN, Pages 1090, 1112.
// S905D2 Datasheet, Section 7.2.3.41 VDIN, Pages 781, 805.
// S905D3 Datasheet, Section 8.2.3.42 VDIN, Pages 717, 740.
class VideoInputChannelFifoControl2
    : public hwreg::RegisterBase<VideoInputChannelFifoControl2, uint32_t> {
 public:
  // Bits 25 and 23-20 provide additional configuration on decimation.
  // These bits are not defined because this driver currently does not support
  // decimation.

  // True iff input decimation subsampling is enabled.
  DEF_BIT(24, decimation_data_enabled);

  // Only 1 / (decimation_ratio_minus_one + 1) of the pixels will be sampled.
  // Setting this field to zero effectively disables decimation.
  DEF_FIELD(19, 16, decimation_ratio_minus_one);

  // Bits 7-0 configure VDI 5. These bits are not defined because this driver
  // currently does not support decimation.

  static hwreg::RegisterAddr<VideoInputChannelFifoControl2> Get(VideoInputModuleId video_input) {
    switch (video_input) {
      case VideoInputModuleId::kVideoInputModule0:
        return {0x120f * sizeof(uint32_t)};
      case VideoInputModuleId::kVideoInputModule1:
        return {0x130f * sizeof(uint32_t)};
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid video input module ID: %d", static_cast<int>(video_input));
  }
};

// VDIN0_ASFIFO_CTRL3, VDIN1_ASFIFO_CTRL3
//
// A311D Datasheet, Section 10.2.3.42 VDIN, Pages 1106, 1128.
// S905D2 Datasheet, Section 7.2.3.41 VDIN, Pages 798-799, 822.
// S905D3 Datasheet, Section 8.2.3.42 VDIN, Pages 733, 755.
class VideoInputChannelFifoControl3
    : public hwreg::RegisterBase<VideoInputChannelFifoControl3, uint32_t> {
 public:
  static hwreg::RegisterAddr<VideoInputChannelFifoControl3> Get(VideoInputModuleId video_input) {
    switch (video_input) {
      case VideoInputModuleId::kVideoInputModule0:
        return {0x126f * sizeof(uint32_t)};
      case VideoInputModuleId::kVideoInputModule1:
        return {0x136f * sizeof(uint32_t)};
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid video input module ID: %d", static_cast<int>(video_input));
  }

  // Bits 31-24 configure VDI 9.
  // Currently this driver doesn't use VDI channel 9, so we don't define these
  // bits.

  // Bits 23-16 configure VDI 8. These bits are not documented in any
  // datasheet. Experiments on a VIM3 (using Amlogic A311D) show that these
  // bits correspond to VDI 8 / ASFIFO 8.

  // Enable data transmission on channel 8.
  DEF_BIT(23, channel8_data_enabled);

  // Enable go_field (Vsync) signals on channel 8.
  DEF_BIT(22, channel8_go_field_signal_enabled);

  // Enable go_line (Hsync) signals for channel 8.
  DEF_BIT(21, channel8_go_line_signal_enabled);

  // True iff the input video on channel 8 has negative polarity Vsync signals.
  DEF_BIT(20, channel8_input_vsync_is_negative);

  // True iff the input video on channel 6 has negative polarity Hsync signals.
  DEF_BIT(19, channel8_input_hsync_is_negative);

  // If true, the ASFIFO will be reset on each Vsync signal.
  DEF_BIT(18, channel8_async_fifo_software_reset_on_vsync);

  // Clears (and acknowledges) the `vdi8_fifo_overflow` bit in the
  // `VDIN0/1_COM_STATUS2` register.
  DEF_BIT(17, channel8_clear_fifo_overflow_bit);

  // Resets the async FIFO.
  //
  // This bit is a "level signal" bit. Drivers reset the FIFO by first setting
  // it to 1, and then to 0.
  DEF_BIT(16, channel8_async_fifo_software_reset);

  // Bits 15-9 configure VDI 7.
  // Currently this driver doesn't use VDI channel 7, so we don't define these
  // bits.

  // Enable data transmission on channel 6.
  DEF_BIT(7, channel6_data_enabled);

  // Enable go_field (Vsync) signals on channel 6.
  DEF_BIT(6, channel6_go_field_signal_enabled);

  // Enable go_line (Hsync) signals on channel 6.
  DEF_BIT(5, channel6_go_line_signal_enabled);

  // True iff the input video on channel 6 has negative polarity Vsync signals.
  DEF_BIT(4, channel6_input_vsync_is_negative);

  // True iff the input video on channel 6 has negative polarity Hsync signals.
  DEF_BIT(3, channel6_input_hsync_is_negative);

  // If true, the channel ASFIFO will be reset on each Vsync signal.
  DEF_BIT(2, channel6_async_fifo_software_reset_on_vsync);

  // Clears (and acknowledges) the `vdi6_fifo_overflow` bit in the
  // `VDIN0/1_COM_STATUS2` register.
  DEF_BIT(1, channel6_clear_fifo_overflow_bit);

  // Resets the async FIFO.
  //
  // This bit is a "level signal" bit. Drivers reset the FIFO by first setting
  // it to 1, and then to 0.
  DEF_BIT(0, channel6_async_fifo_software_reset);
};

// Selects the clock or data source for a writeback mux.
//
// Fields of this type must transition through `kDisabled` when being updated.
enum class WritebackMuxSource : uint32_t {
  // Disable the input path. A required intermediate step for changing input
  // sources.
  kDisabled = 0b00000,
  // VIU ENCI domain.
  kEncoderInterlaced = 0b00001,
  // VIU ENCP domain.
  kEncoderProgressive = 0b00010,
  // VIU ENCT domain.
  kEncoderTvPanel = 0b00100,
  // Also known as "VIU writeback domain 1" in S905D3 datasheets.
  kViuWriteback0 = 0b01000,
  // Also known as "VIU writeback domain 2" in S905D3 datasheets.
  kViuWriteback1 = 0b10000,
};

// VPU_VIU_VDIN_IF_MUX_CTRL
//
// Each VDIN (Video Input Module) can receive image data from one of the data
// sources available on the SoC, selected via the `VideoInputCommandControl`
// register. Two of the data sources, `kWritebackMux0` and `kWritebackMux1`,
// are actually multiplexers connected to multiple sources.
//
// This register configures the multiplexers. Each multiplexer has clock and
// data outputs ("paths" in the datasheets), which are configured separately.
//
// Many configurations are invalid. For example, a multiplexer's clock and data
// outputs must be connected to the same source. To reduce the likelihood of
// errors, this register's fields should be accessed exclusively via the
// higher-level helper methods defined after the fields.
//
// S905D3 Datasheet, Section 8.2.3.1 "VPU Registers", Page 314.
//
// This register is not documented in the datasheets for S905D2, A311D and T931.
// However, experiments on VIM3 (Amlogic A311D), Astro (Amlogic S905D2) and
// Sherlock (Amlogic T931) show that the register exists and has the same layout
// and functionality as that in S905D3.
class WritebackMuxControl : public hwreg::RegisterBase<WritebackMuxControl, uint32_t> {
 public:
  static hwreg::RegisterAddr<WritebackMuxControl> Get() { return {0x2783 * sizeof(uint32_t)}; }

  // Selects the data path from VIU/Encoder to writeback mux 1.
  //
  // This field is effective when a `VideoInputCommandControl` register selects
  // the `kWritebackMux1` input ("VDIN0/1 source input 9" in the datasheet).
  //
  // It's preferred to use `GetMux1Selection()` and `SetMux1Selection()` helper
  // functions to change the data path. For direct register manipulations,
  // `mux1_data_selection` and `mux1_clock_selection` must be equal, otherwise
  // the behavior is undefined.
  DEF_ENUM_FIELD(WritebackMuxSource, 28, 24, mux1_data_selection);

  // Selects the clock path from VIU/Encoder to writeback mux 1.
  //
  // This field is effective when a `VideoInputCommandControl` register selects
  // the `kWritebackMux1` input ("VDIN0/1 source input 9" in the datasheet).
  //
  // It's preferred to use `GetMux1Selection()` and `SetMux1Selection()` helper
  // functions to change the data path. For direct register manipulations,
  // `mux1_data_selection` and `mux1_clock_selection` must be equal, otherwise
  // the behavior is undefined.
  DEF_ENUM_FIELD(WritebackMuxSource, 20, 16, mux1_clock_selection);

  // Selects the data path from VIU/Encoder to writeback mux 0.
  //
  // This field is effective when a `VideoInputCommandControl` register selects
  // the `kWritebackMux0` input ("VDIN0/1 source input 7" in the datasheet).
  //
  // It's preferred to use `GetMux0Selection()` and `SetMux0Selection()` helper
  // functions to change the data path. For direct register manipulations,
  // `mux0_data_selection` and `mux0_clock_selection` must be equal, otherwise
  // the behavior is undefined.
  DEF_ENUM_FIELD(WritebackMuxSource, 12, 8, mux0_data_selection);

  // Selects the clock path from VIU/Encoder to writeback mux 0.
  //
  // This field is effective when a `VideoInputCommandControl` register selects
  // the `kWritebackMux0` input ("VDIN0/1 source input 7" in the datasheet).
  //
  // It's preferred to use `GetMux0Selection()` and `SetMux0Selection()` helper
  // functions to change the data path. For direct register manipulations,
  // `mux0_data_selection` and `mux0_clock_selection` must be equal, otherwise
  // the behavior is WritebackMuxSelection.
  DEF_ENUM_FIELD(WritebackMuxSource, 4, 0, mux0_clock_selection);

  // The clock/data source selected for writeback mux 1.
  WritebackMuxSource GetMux1Selection() const {
    WritebackMuxSource clock = mux1_clock_selection();
    WritebackMuxSource data = mux1_data_selection();
    if (clock != data) {
      fdf::warn("Writeback mux1 clock selection {} != data selection {}",
                static_cast<uint32_t>(clock), static_cast<uint32_t>(data));
    }
    return clock;
  }

  // Set the data/clock source for writeback mux 1.
  WritebackMuxControl& SetMux1Selection(WritebackMuxSource mux_selection) {
    ZX_ASSERT(GetMux1Selection() == WritebackMuxSource::kDisabled ||
              mux_selection == WritebackMuxSource::kDisabled);
    switch (mux_selection) {
      case WritebackMuxSource::kDisabled:
      case WritebackMuxSource::kEncoderInterlaced:
      case WritebackMuxSource::kEncoderProgressive:
      case WritebackMuxSource::kEncoderTvPanel:
      case WritebackMuxSource::kViuWriteback0:
      case WritebackMuxSource::kViuWriteback1:
        set_mux1_clock_selection(mux_selection);
        set_mux1_data_selection(mux_selection);
        return *this;
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid mux selection %" PRIu32,
                        static_cast<uint32_t>(mux_selection));
    return *this;
  }

  // The clock/data source selected for writeback mux 0.
  WritebackMuxSource GetMux0Selection() const {
    WritebackMuxSource clock = mux0_clock_selection();
    WritebackMuxSource data = mux0_data_selection();
    if (clock != data) {
      fdf::warn("Writeback mux0 clock selection {} != data selection {}",
                static_cast<uint32_t>(clock), static_cast<uint32_t>(data));
    }
    return clock;
  }

  // Set the data/clock source for writeback mux 0.
  WritebackMuxControl& SetMux0Selection(WritebackMuxSource mux_selection) {
    ZX_ASSERT(GetMux0Selection() == WritebackMuxSource::kDisabled ||
              mux_selection == WritebackMuxSource::kDisabled);
    switch (mux_selection) {
      case WritebackMuxSource::kDisabled:
      case WritebackMuxSource::kEncoderInterlaced:
      case WritebackMuxSource::kEncoderProgressive:
      case WritebackMuxSource::kEncoderTvPanel:
      case WritebackMuxSource::kViuWriteback0:
      case WritebackMuxSource::kViuWriteback1:
        set_mux0_clock_selection(mux_selection);
        set_mux0_data_selection(mux_selection);
        return *this;
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid mux selection %" PRIu32,
                        static_cast<uint32_t>(mux_selection));
    return *this;
  }
};

// VDIN0_LFIFO_CTRL, VDIN1_LFIFO_CTRL.
//
// A311D Datasheet, Section 10.2.3.42 VDIN, Pages 1092, 1114.
// S905D2 Datasheet, Section 7.2.3.41 VDIN, Pages 783, 807.
// S905D3 Datasheet, Section 8.2.3.42 VDIN, Pages 719, 742.
class VideoInputLinearFifoControl
    : public hwreg::RegisterBase<VideoInputLinearFifoControl, uint32_t> {
 public:
  static hwreg::RegisterAddr<VideoInputLinearFifoControl> Get(VideoInputModuleId video_input) {
    switch (video_input) {
      case VideoInputModuleId::kVideoInputModule0:
        return {0x121a * sizeof(uint32_t)};
      case VideoInputModuleId::kVideoInputModule1:
        return {0x131a * sizeof(uint32_t)};
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid video input module ID: %d", static_cast<int>(video_input));
  }

  DEF_FIELD(11, 0, linear_fifo_buffer_size_pixels);
};

// VDIN0_INTF_WIDTHM1, VDIN1_INTF_WIDTHM1.
//
// A311D Datasheet, Section 10.2.3.42 VDIN, Pages 1092, 1114.
// S905D2 Datasheet, Section 7.2.3.41 VDIN, Pages 783, 807.
// S905D3 Datasheet, Section 8.2.3.42 VDIN, Pages 720, 742.
class VideoInputInterfaceWidth : public hwreg::RegisterBase<VideoInputInterfaceWidth, uint32_t> {
 public:
  static hwreg::RegisterAddr<VideoInputInterfaceWidth> Get(VideoInputModuleId video_input) {
    switch (video_input) {
      case VideoInputModuleId::kVideoInputModule0:
        return {0x121c * sizeof(uint32_t)};
      case VideoInputModuleId::kVideoInputModule1:
        return {0x131c * sizeof(uint32_t)};
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid video input module ID: %d", static_cast<int>(video_input));
  }

  // Bit 26 and bit 25 provide additional control over the memory interface
  // write behavior. They are not used by the driver for capture, so we don't
  // define them.

  // Width minus one of the input video after up/downscaling but before
  // cropping (window function), in pixels.
  //
  // It's preferred to use width() and set_width() helpers instead.
  DEF_FIELD(12, 0, width_minus_one_px);

  int width_px() const {
    // The value `width_minus_one_px() + 1` falls within the `int` range
    // because `width_minus_one_px()` has only 13 bits.
    return static_cast<int>(width_minus_one_px()) + 1;
  }

  // Sets the input image width to `width_px` pixels.
  //
  // `width_px` must be positive and not greater than 2^13 (8192).
  VideoInputInterfaceWidth& set_width_px(int width_px) {
    ZX_DEBUG_ASSERT(width_px > 0);
    ZX_DEBUG_ASSERT(width_px <= (1 << 13));
    // `width - 1` always fits in the register field because it is guaranteed
    // to be non-negative and is less than 2^13.
    return set_width_minus_one_px(static_cast<uint32_t>(width_px - 1));
  }
};

// VDIN0_WR_H_START_END, VDIN1_WR_H_START_END.
//
// A311D Datasheet, Section 10.2.3.42 VDIN, Pages 1094, 1116.
// S905D2 Datasheet, Section 7.2.3.41 VDIN, Pages 785, 809.
// S905D3 Datasheet, Section 8.2.3.42 VDIN, Pages 721, 743.
// Display Write Back App-Note, Amlogic (Google internal), Page 4.
class VideoInputWriteRangeHorizontal
    : public hwreg::RegisterBase<VideoInputWriteRangeHorizontal, uint32_t> {
 public:
  enum class Endianness : uint8_t {
    // In big-endian mode, the memory interface writes 8-bit YUV444 capture
    // images in the following byte order (without 64-bit pair swaps):
    //    Y5  V4  U4  Y4  V3  U3  Y3  V2    U2  Y2  V1  U1  Y1  V0  U0  Y0
    //    U10 Y10 V9  U9  Y9  V8  U8  Y8    V7  U7  Y7  V6  U6  Y6  V5  U5
    //    V15 U15 Y15 V14 U14 Y14 V13 U13   Y13 V12 U12 Y12 V11 U11 Y11 V10 ...
    kBig = 0,
    // In little-endian mode, the memory interface writes 8-bit YUV444 capture
    // images in the following byte order (without 64-bit pair swaps):
    //    V0  U0  Y0  V1  U1  Y1  V2  U2    Y2  V3  U3  Y3  V4  U4  Y4  V5
    //    U5  Y5  V6  U6  Y6  V7  U7  Y7    V8  U8  Y8  V9  U9  Y9  V10 U10
    //    Y10 V11 U11 Y11 V12 U12 Y12 V13   U13 Y13 V14 U14 Y14 V15 U15 Y15
    kLittle = 1,
  };

  static hwreg::RegisterAddr<VideoInputWriteRangeHorizontal> Get(VideoInputModuleId video_input) {
    switch (video_input) {
      case VideoInputModuleId::kVideoInputModule0:
        return {0x1221 * sizeof(uint32_t)};
      case VideoInputModuleId::kVideoInputModule1:
        return {0x1321 * sizeof(uint32_t)};
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid video input module ID: %d", static_cast<int>(video_input));
  }

  // Byte ordering ("endianness") for every 128-bit (16-byte) chunk.
  //
  // This bit is not documented in any datasheet. Experiments on Khadas
  // VIM3 (A311D), Astro (S905D2) and Nelson (S905D3) all confirm that
  // this bit is available and functions as described above for both VDINs.
  DEF_ENUM_FIELD(Endianness, 30, 30, endianness_128_bit);

  // If true, the captured contents are flipped horizontally.
  //
  // This bit is documented for VDIN0 but not for VDIN1 in Amlogic datasheets.
  // Both "Display Write Back App-Note" and experiments on Khadas VIM3
  // (A311D) confirm that this bit is available and functions as described
  // above for both VDINs.
  DEF_BIT(29, horizontally_flipped);

  // The leftmost column (inclusive) to write on the write image buffer, in
  // pixels.
  //
  // Must be less than or equal to `right_px_inclusive`.
  //
  // The Amlogic datasheets specify that this field uses bits 28-16 for VDIN0
  // bits 27-16 for VDIN1. However, both the "Display Write Back App-Note"
  // and experiments on Khadas VIM3 (A311D) confirm that it actually uses
  // bits 28-16 for both VDINs.
  //
  // It's preferred to use the `SetHorizontalRange()` helper method.
  DEF_FIELD(28, 16, left_px_inclusive);

  // The rightmost column (inclusive) to write on the write image buffer, in
  // pixels.
  //
  // Must be greater than or equal to `left_px_inclusive`.
  //
  // The Amlogic datasheets specify that this field uses bits 12-0 for VDIN0
  // bits 11-0 for VDIN1. However, both the "Display Write Back App-Note"
  // and experiments on Khadas VIM3 (A311D) confirm that it actually uses
  // bits 12-0 for both VDINs.
  //
  // It's preferred to use the `SetHorizontalRange()` helper method.
  DEF_FIELD(12, 0, right_px_inclusive);

  // Sets the horizontal range of the write image buffer to
  // `[left_px_inclusive, right_px_inclusive]`.
  //
  // `left_px_inclusive` must be less than or equal to `right_px_inclusive`.
  //
  // Both `left_px_inclusive` and `right_px_inclusive` must be non-negative and
  // less than 2^13 (8192).
  VideoInputWriteRangeHorizontal& SetHorizontalRange(int left_px_inclusive,
                                                     int right_px_inclusive) {
    ZX_DEBUG_ASSERT(left_px_inclusive >= 0);
    ZX_DEBUG_ASSERT(left_px_inclusive < (1 << 13));
    ZX_DEBUG_ASSERT(right_px_inclusive >= 0);
    ZX_DEBUG_ASSERT(right_px_inclusive < (1 << 13));
    ZX_DEBUG_ASSERT(left_px_inclusive <= right_px_inclusive);

    return set_left_px_inclusive(left_px_inclusive).set_right_px_inclusive(right_px_inclusive);
  }
};

// VDIN0_WR_V_START_END, VDIN1_WR_V_START_END.
//
// A311D Datasheet, Section 10.2.3.42 VDIN, Pages 1094, 1116.
// S905D2 Datasheet, Section 7.2.3.41 VDIN, Pages 785, 809.
// S905D3 Datasheet, Section 8.2.3.42 VDIN, Pages 721, 743.
class VideoInputWriteRangeVertical
    : public hwreg::RegisterBase<VideoInputWriteRangeVertical, uint32_t> {
 public:
  static hwreg::RegisterAddr<VideoInputWriteRangeVertical> Get(VideoInputModuleId video_input) {
    switch (video_input) {
      case VideoInputModuleId::kVideoInputModule0:
        return {0x1222 * sizeof(uint32_t)};
      case VideoInputModuleId::kVideoInputModule1:
        return {0x1322 * sizeof(uint32_t)};
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid video input module ID: %d", static_cast<int>(video_input));
  }

  // If true, the captured contents are flipped vertically.
  //
  // This bit is documented for VDIN0 but not documented for VDIN1 in any
  // Amlogic datasheets. Both "Display Write Back App-Note" and experiments on
  // Khadas VIM3 (A311D) show that both video input modules have this bit
  // available.
  DEF_BIT(29, vertically_flipped);

  // The top row (inclusive) to write on the write image buffer, in lines.
  //
  // Must be less than or equal to `bottom_inclusive`.
  //
  // The Amlogic datasheets specify that this field uses bits 28-16 for VDIN0
  // bits 27-16 for VDIN1. However, both the "Display Write Back App-Note"
  // and experiments on Khadas VIM3 (A311D) confirm that it actually uses
  // bits 28-16 for both VDINs.
  //
  // It's preferred to use the `SetVerticalRange()` helper method.
  DEF_FIELD(28, 16, top_line_inclusive);

  // The bottom row (inclusive) to write on the write image buffer, in lines.
  //
  // Must be greater than or equal to `top_inclusive`.
  //
  // The Amlogic datasheets specify that this field uses bits 12-0 for VDIN0
  // bits 11-0 for VDIN1. However, both the "Display Write Back App-Note"
  // and experiments on Khadas VIM3 (A311D) confirm that it actually uses
  // bits 12-0 for both VDINs.
  //
  // It's preferred to use the `SetVerticalRange()` helper method.
  DEF_FIELD(12, 0, bottom_line_inclusive);

  // Set the vertical range of the write image buffer to `[top_line_inclusive,
  // bottom_line_inclusive]`.
  //
  // `top_line_inclusive` must be less than or equal to `bottom_line_inclusive`.
  //
  // Both `top_line_inclusive` and `bottom_line_inclusive` must be non-negative
  // and less than 2^13 (8192).
  VideoInputWriteRangeVertical& SetVerticalRange(int top_line_inclusive,
                                                 int bottom_line_inclusive) {
    ZX_DEBUG_ASSERT(top_line_inclusive >= 0);
    ZX_DEBUG_ASSERT(top_line_inclusive < (1 << 13));
    ZX_DEBUG_ASSERT(bottom_line_inclusive >= 0);
    ZX_DEBUG_ASSERT(bottom_line_inclusive < (1 << 13));
    ZX_DEBUG_ASSERT(top_line_inclusive <= bottom_line_inclusive);

    return set_top_line_inclusive(top_line_inclusive)
        .set_bottom_line_inclusive(bottom_line_inclusive);
  }
};

// VDIN0_WR_CTRL, VDIN1_WR_CTRL.
//
// A311D Datasheet, Section 10.2.3.42 VDIN, Pages 1093, 1115-1116.
// S905D2 Datasheet, Section 7.2.3.41 VDIN, Pages 784-785, 808-809.
// S905D3 Datasheet, Section 8.2.3.42 VDIN, Pages 720-721, 743.
// Private correspondence with Amlogic (Re: display loopback mode), June 27,
// 2019.
class VideoInputWriteControl : public hwreg::RegisterBase<VideoInputWriteControl, uint32_t> {
 public:
  enum class ChromaHorizontalSubsamplingMode : uint8_t {
    // Only the chroma channels of even pixels in a row are sampled.
    // The image is downsampled to YUV 4:2:x format.
    kEvenPixelsOnly = 0,
    // Only the chroma channels of odd pixels in a row are sampled.
    // The image is downsampled to YUV 4:2:x format.
    kOddPixelsOnly = 1,
    // The average value of chroma channel of each pixel pair in a row are
    // sampled.
    // The image is downsampled to YUV 4:2:x format.
    kAveragedPixels = 2,
    // The Chroma channels of all pixels are used; no subsampling is performed.
    // The image is downsampled to YUV 4:4:x format.
    //
    // To use this subsampling mode, `write_format` must be `kSemiPlanar`.
    kAllPixels = 3,
  };

  enum class LineEndIndicator : uint8_t {
    // Uses EOL (End of Line) as the line-end indication. The precise
    // definition of "EOL" is unclear from the datasheets.
    kEol = 0,
    // Uses the line width, as specified in the `VideoInputInterfaceWidth`
    // register, to determine line endings.
    kWidth = 1,
  };

  enum class ChromaChannelLayout : uint8_t {
    // First Cb (U), then Cr (V). Used in the NV12 chroma subplane.
    kCbCr = 0,
    // First Cr (V), then Cb (U). Used in the NV21 chroma subplane.
    kCrCb = 1,
  };

  enum class ChromaVerticalSubsamplingMode : uint8_t {
    // Only the chroma channels of pixels in even rows will be sampled.
    // The image will be downsampled to YUV 4:x:0 format.
    kEvenRowsOnly = 0,
    // Only the chroma channels of pixels in odd rows will be sampled.
    // The image will be downsampled to YUV 4:x:0 format.
    kOddRowsOnly = 1,
    kReserved = 2,
    // Chroma channels of all rows will be used; no subsampling is performed.
    // The image will be downsampled to YUV 4:x:x format.
    kAllRows = 3,
  };

  enum class YuvSamplingStorageFormat : uint8_t {
    // Image is in a YUV 4:2:2 packed (single-planar) format, stored in the Luma
    // canvas.
    // The detailed macropixel encoding is unclear.
    kYuv422Packed = 0,
    // Image is in a YUV 4:4:4 packed (single-planar) format, stored in the Luma
    // canvas.
    //
    // Note: RGB888 format can be considered as a special YUV 4:4:4 packed
    // format where the R/G/B channels correspond to Y/Cb/Cr channels
    // respectively.
    kYuv444Packed = 1,
    // Image is in a YUV semi-planar format. The Luma plane is stored in the
    // Luma canvas and the Chroma plane is stored in the Chroma canvas.
    kSemiPlanar = 2,
    // Image is in a YUV 4:2:2 10-bit "fully packed" (single-planar) format,
    // stored in the Luma canvas.
    // The detailed macropixel encoding is unclear.
    kYuv422FullyPackedMode = 3,
  };

  static hwreg::RegisterAddr<VideoInputWriteControl> Get(VideoInputModuleId video_input) {
    switch (video_input) {
      case VideoInputModuleId::kVideoInputModule0:
        return {0x1220 * sizeof(uint32_t)};
      case VideoInputModuleId::kVideoInputModule1:
        return {0x1320 * sizeof(uint32_t)};
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid video input module ID: %d", static_cast<int>(video_input));
  }

  // Effective iff `yuv_sampling_storage_format` is `kYuv422Packed` or
  // `kSemiPlanar`.
  DEF_ENUM_FIELD(ChromaHorizontalSubsamplingMode, 31, 30, chroma_horizontal_subsampling_mode);

  // True iff the clock gate for the memory interface is disabled, which means
  // that the memory interface uses a free running clock (i.e. a clock that is
  // not interrupted nor gated).
  DEF_BIT(29, memory_interface_clock_gate_disabled);

  // If true, the counter field for memory interface responses is cleared.
  // The register it clears is unknown from the datasheets.
  DEF_BIT(28, memory_interface_response_counter_cleared);

  // Selects the line end indicator in the memory write interface.
  DEF_ENUM_FIELD(LineEndIndicator, 27, 27, line_end_indicator_selection);

  // True iff the frame is reset on each Vsync.
  DEF_BIT(23, frame_reset_on_vsync);

  // True iff a software reset is performed on the line FIFO on each Vsync.
  DEF_BIT(22, line_fifo_reset_on_vsync);

  // If true, the `direct_write_done` bit of the `VideoInputCommandStatus0`
  // register is cleared.
  DEF_BIT(21, direct_write_done_cleared);

  // If true, the `noise_reduced_write_done` bit of the
  // `VideoInputCommandStatus0` register is cleared.
  DEF_BIT(20, noise_reduced_write_done_cleared);

  // True iff the data is reordered by swapping consecutive pairs of 64-bit
  // chunks.
  DEF_BIT(19, consecutive_64bit_pairs_reordered);

  // The layout of channels in each macropixel of the chroma subplane.
  //
  // Effective iff `yuv_sampling_storage_format` is `kSemiPlanar`.
  DEF_ENUM_FIELD(ChromaChannelLayout, 18, 18, chroma_channel_layout);

  // Effective iff `yuv_sampling_storage_format` is `kSemiPlanar`.
  DEF_ENUM_FIELD(ChromaVerticalSubsamplingMode, 17, 16, chroma_vertical_subsampling_mode);

  // YUV sampling and storage format of the image to write.
  DEF_ENUM_FIELD(YuvSamplingStorageFormat, 13, 12, yuv_sampling_storage_format);

  // True iff the value of `luma_canvas_address` is double-buffered. In that
  // case, the new value written to the register field is applied on each Vsync.
  DEF_BIT(11, luma_canvas_address_double_buffered);

  // If true, unpauses the write request.
  //
  // This bit is not in any datasheet but was mentioned in private
  // correspondence with Amlogic. Experiments on Khadas VIM3 (A311D),
  // Astro (S905D2) and Nelson (S905D3) all show that this bit is available
  // and functions as described above.
  DEF_BIT(10, write_request_unpaused);

  // True iff the next memory write request is of higher priority.
  DEF_BIT(9, write_request_urgent);

  // True iff the memory write interface allows memory write requests.
  DEF_BIT(8, write_request_enabled);

  // The address to the Luma canvas for the memory interface to write to.
  DEF_FIELD(7, 0, luma_canvas_address);
};

// VDIN_MISC_CTRL
//
// This register is not documented in A311D or S905D2 datasheets, but it
// appears in S905D3 datasheets and private correspondence with Amlogic.
//
// Experiments on Khadas VIM3 board (A311D) and Astro (S905D2) show that
// the bits defined in this class are available and have the same function
// as S905D3.
//
// S905D3 Datasheet, Section 8.2.3.1 "VPU Registers", Page 314.
// Private correspondence with Amlogic (Re: display loopback mode), June 27,
// 2019.
class VideoInputMiscellaneousControl
    : public hwreg::RegisterBase<VideoInputMiscellaneousControl, uint32_t> {
 public:
  static hwreg::RegisterAddr<VideoInputMiscellaneousControl> Get() {
    return {0x2782 * sizeof(uint32_t)};
  }

  DEF_RSVDZ_FIELD(31, 26);

  // Bits 25-6, as defined in the datasheets, provide additional control over
  // the VDIN output behavior. They are not defined because the the driver
  // doesn't use them.

  // If true, the memory interface of video input module 1 (VDIN1) is reset.
  //
  // Experiments on Khadas VIM3 (A311D) show that the memory interface is reset
  // when the bit is held true for 1us.
  DEF_BIT(4, video_input_module1_memory_interface_reset);

  // If true, the memory interface of video input module 0 (VDIN0) is reset.
  //
  // Experiments on Khadas VIM3 (A311D) show that the memory interface is
  // reset when the bit is held true for 1us.
  DEF_BIT(3, video_input_module0_memory_interface_reset);

  // If true, the video input module 1 (VDIN1) is reset.
  //
  // Experiments on Khadas VIM3 (A311D) shows that the memory interface is
  // reset when the bit is held true for 1us.
  DEF_BIT(1, video_input_module1_reset);

  // If true, the video input module 0 (VDIN0) is reset.
  //
  // Experiments on Khadas VIM3 (A311D) show that the video input module is
  // reset when the bit is held true for 1us.
  DEF_BIT(0, video_input_module0_reset);
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VIDEO_INPUT_REGS_H_
