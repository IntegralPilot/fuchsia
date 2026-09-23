// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/vpu.h"

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <cstdint>
#include <utility>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/amlogic-display/board-resources.h"
#include "src/graphics/display/drivers/amlogic-display/clock-regs.h"
#include "src/graphics/display/drivers/amlogic-display/common.h"
#include "src/graphics/display/drivers/amlogic-display/power-regs.h"
#include "src/graphics/display/drivers/amlogic-display/video-input-regs.h"
#include "src/graphics/display/drivers/amlogic-display/vpp-regs.h"
#include "src/graphics/display/drivers/amlogic-display/vpu-regs.h"

namespace amlogic_display {

namespace {
constexpr uint32_t kFirstTimeLoadMagicNumber = 0x304e65;  // 0Ne

constexpr int16_t RGB709_to_YUV709l_coeff[24] = {
    0x0000, 0x0000, 0x0000, 0x00bb, 0x0275, 0x003f, 0x1f99, 0x1ea6, 0x01c2, 0x01c2, 0x1e67, 0x1fd7,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0040, 0x0200, 0x0200, 0x0000, 0x0000, 0x0000,
};

// Below co-efficients are used to convert 709L to RGB. The table is provided
// by Amlogic
//    ycbcr limit range, 709 to RGB
//    -16      1.164  0      1.793  0
//    -128     1.164 -0.213 -0.534  0
//    -128     1.164  2.115  0      0
constexpr uint32_t capture_yuv2rgb_coeff[3][3] = {
    {0x04a8, 0x0000, 0x072c}, {0x04a8, 0x1f26, 0x1ddd}, {0x04a8, 0x0876, 0x0000}};
constexpr uint32_t capture_yuv2rgb_preoffset[3] = {0x7c0, 0x600, 0x600};
constexpr uint32_t capture_yuv2rgb_offset[3] = {0, 0, 0};

constexpr VideoInputModuleId kVideoInputModuleId = VideoInputModuleId::kVideoInputModule1;

}  // namespace

// EE Reset registers on the CBUS (regular power-gated config registers domain).
//
// A311D datasheet section 8.8.2.1 "Register Description" > "EE Reset" describes
// the bits under the RESET{0,7}_REGISTER sections. The RESET{0,7}_MASK and
// RESET{0,7}_LEVEL sections describe the interactions between the registers,
// the watchdog timer, and the reset condition.
//
// A311D datasheet section 8.8.2.1 has full MMIO addresses. S905D3 datasheet
// section 6.8.2 with the same title covers the same registers, and also
// explicitly states that the base for all registers is 0xffd0'1000. This
// address is listed under the RESET entry in A311D section 8.1 "System" >
// "Memory Map" and S905D3 datasheet section 6.1 with the same name.
//
// The following datasheets have matching information.
// * S905D2, Section 6.7.2.1 "EE Reset", Section 6.1 "Memory Map"
// * T931, Section 6.8.2.1 "EE Reset", Section 6.1 "Memory Map"
#define RESET0_LEVEL 0x80
#define RESET1_LEVEL 0x84
#define RESET2_LEVEL 0x88
#define RESET4_LEVEL 0x90
#define RESET7_LEVEL 0x9c

// static
zx::result<std::unique_ptr<Vpu>> Vpu::Create(
    fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device) {
  ZX_DEBUG_ASSERT(platform_device.is_valid());

  // Map VPU registers
  zx::result<fdf::MmioBuffer> vpu_mmio_result = MapMmio(kMmioNameVpu, platform_device);
  if (vpu_mmio_result.is_error()) {
    return vpu_mmio_result.take_error();
  }
  fdf::MmioBuffer vpu_mmio = std::move(vpu_mmio_result).value();

  zx::result<fdf::MmioBuffer> hhi_mmio_result = MapMmio(kMmioNameHhi, platform_device);
  if (hhi_mmio_result.is_error()) {
    return hhi_mmio_result.take_error();
  }
  fdf::MmioBuffer hhi_mmio = std::move(hhi_mmio_result).value();

  zx::result<fdf::MmioBuffer> aobus_mmio_result = MapMmio(kMmioNameAlwaysOnRti, platform_device);
  if (aobus_mmio_result.is_error()) {
    return aobus_mmio_result.take_error();
  }
  fdf::MmioBuffer aobus_mmio = std::move(aobus_mmio_result).value();

  zx::result<fdf::MmioBuffer> reset_mmio_result = MapMmio(kMmioNameEeReset, platform_device);
  if (reset_mmio_result.is_error()) {
    return reset_mmio_result.take_error();
  }
  fdf::MmioBuffer reset_mmio = std::move(reset_mmio_result).value();

  fbl::AllocChecker alloc_checker;
  auto vpu = fbl::make_unique_checked<Vpu>(&alloc_checker, std::move(vpu_mmio), std::move(hhi_mmio),
                                           std::move(aobus_mmio), std::move(reset_mmio));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for Vpu");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(vpu));
}

Vpu::Vpu(fdf::MmioBuffer vpu_mmio, fdf::MmioBuffer hhi_mmio, fdf::MmioBuffer aobus_mmio,
         fdf::MmioBuffer reset_mmio)
    : vpu_mmio_(std::move(vpu_mmio)),
      hhi_mmio_(std::move(hhi_mmio)),
      aobus_mmio_(std::move(aobus_mmio)),
      reset_mmio_(std::move(reset_mmio)),
      capture_state_(CAPTURE_RESET) {}

bool Vpu::CheckAndClaimHardwareOwnership() {
  uint32_t regVal = vpu_mmio_.Read32(VPP_DUMMY_DATA);
  if (regVal == kFirstTimeLoadMagicNumber) {
    // we have already been loaded once. don't set again.
    return false;
  }
  vpu_mmio_.Write32(kFirstTimeLoadMagicNumber, VPP_DUMMY_DATA);
  first_time_load_ = true;
  return true;
}

void Vpu::SetupPostProcessorOutputInterface() {
  // init vpu fifo control register
  vpu_mmio_.Write32(SetFieldValue32(vpu_mmio_.Read32(VPP_OFIFO_SIZE), /*field_begin_bit=*/0,
                                    /*field_size_bits=*/12, /*field_value=*/0xFFF),
                    VPP_OFIFO_SIZE);
  vpu_mmio_.Write32(0x08080808, VPP_HOLD_LINES);
  // default probe_sel, for highlight en
  vpu_mmio_.Write32(SetFieldValue32(vpu_mmio_.Read32(VPP_MATRIX_CTRL), /*field_begin_bit=*/12,
                                    /*field_size_bits=*/3, /*field_value=*/0x7),
                    VPP_MATRIX_CTRL);
}

void Vpu::SetupPostProcessorColorConversion(ColorSpaceConversionMode mode) {
  // TODO(https://fxbug.dev/42082404): Revise the selection of matrices used for color
  // conversion.
  switch (mode) {
    case ColorSpaceConversionMode::kRgbInternalRgbOut:
      // This deviates from the Amlogic-provided code which does an RGB ->
      // YUV conversion for all OSDs and a YUV -> RGB conversion after
      // blending.
      vpu_mmio_.Write32(
          SetFieldValue32(vpu_mmio_.Read32(VPP_WRAP_OSD1_MATRIX_EN_CTRL), /*field_begin_bit=*/0,
                          /*field_size_bits=*/1, /*field_value=*/0),
          VPP_WRAP_OSD1_MATRIX_EN_CTRL);
      break;
    case ColorSpaceConversionMode::kRgbInternalYuvOut: {
      // setting up os1 for rgb -> yuv limit
      const int16_t* m = RGB709_to_YUV709l_coeff;

      // VPP WRAP OSD1 matrix
      // TODO(https://fxbug.dev/42059021): Also set VPP_WRAP_OSD2/3 when OSD2/3 is
      // supported.
      vpu_mmio_.Write32(((m[0] & 0xfff) << 16) | (m[1] & 0xfff),
                        VPP_WRAP_OSD1_MATRIX_PRE_OFFSET0_1);
      vpu_mmio_.Write32(m[2] & 0xfff, VPP_WRAP_OSD1_MATRIX_PRE_OFFSET2);
      vpu_mmio_.Write32(((m[3] & 0x1fff) << 16) | (m[4] & 0x1fff), VPP_WRAP_OSD1_MATRIX_COEF00_01);
      vpu_mmio_.Write32(((m[5] & 0x1fff) << 16) | (m[6] & 0x1fff), VPP_WRAP_OSD1_MATRIX_COEF02_10);
      vpu_mmio_.Write32(((m[7] & 0x1fff) << 16) | (m[8] & 0x1fff), VPP_WRAP_OSD1_MATRIX_COEF11_12);
      vpu_mmio_.Write32(((m[9] & 0x1fff) << 16) | (m[10] & 0x1fff), VPP_WRAP_OSD1_MATRIX_COEF20_21);
      vpu_mmio_.Write32(m[11] & 0x1fff, VPP_WRAP_OSD1_MATRIX_COEF22);
      vpu_mmio_.Write32(((m[18] & 0xfff) << 16) | (m[19] & 0xfff), VPP_WRAP_OSD1_MATRIX_OFFSET0_1);
      vpu_mmio_.Write32(m[20] & 0xfff, VPP_WRAP_OSD1_MATRIX_OFFSET2);
      vpu_mmio_.Write32(
          SetFieldValue32(vpu_mmio_.Read32(VPP_WRAP_OSD1_MATRIX_EN_CTRL), /*field_begin_bit=*/0,
                          /*field_size_bits=*/1, /*field_value=*/1),
          VPP_WRAP_OSD1_MATRIX_EN_CTRL);
      break;
    }
    default:
      ZX_ASSERT_MSG(false, "Invalid color conversion mode: %d", static_cast<int>(mode));
  }

  vpu_mmio_.Write32(0xf, DOLBY_PATH_CTRL);

  // Disables VPP POST2 matrix.
  vpu_mmio_.Write32(
      SetFieldValue32(vpu_mmio_.Read32(VPP_POST2_MATRIX_EN_CTRL), /*field_begin_bit=*/0,
                      /*field_size_bits=*/1, /*field_value=*/0),
      VPP_POST2_MATRIX_EN_CTRL);
}

void Vpu::ConfigureClock() {
  // vpu clock
  auto vpu_clock_control = VpuClockControl::Get().FromValue(0);
  vpu_clock_control.set_final_mux_selection(VpuClockControl::FinalMuxSource::kBranch0)
      .set_branch0_mux_source(VpuClockControl::ClockSource::kFixed666Mhz)
      .SetBranch0MuxDivider(1)
      .WriteTo(&hhi_mmio_);
  vpu_clock_control.ReadFrom(&hhi_mmio_).set_branch0_mux_enabled(true).WriteTo(&hhi_mmio_);

  // vpu clkb
  // bit 0 is set since kVpuClkFrequency > clkB max frequency (350MHz)
  VpuClockBControl::Get()
      .FromValue(0)
      .set_clock_source(VpuClockBControl::ClockSource::kFixed500Mhz)
      .SetDivider2(2)
      .WriteTo(&hhi_mmio_);

  // vapb clk
  // turn on ge2d clock since kVpuClkFrequency > 250MHz
  VideoAdvancedPeripheralBusClockControl::Get()
      .FromValue(0)
      .set_final_mux_selection(VideoAdvancedPeripheralBusClockControl::FinalMuxSource::kBranch0)
      .set_ge2d_clock_enabled(true)
      .set_branch0_mux_source(VideoAdvancedPeripheralBusClockControl::ClockSource::kFixed500Mhz)
      .SetBranch0MuxDivider(2)
      .WriteTo(&hhi_mmio_);
  VideoAdvancedPeripheralBusClockControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_branch0_mux_enabled(true)
      .WriteTo(&hhi_mmio_);

  VideoClockOutputControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_encoder_interlaced_enabled(false)
      .set_encoder_tv_enabled(false)
      .set_encoder_progressive_enabled(false)
      .set_encoder_lvds_enabled(false)
      .set_video_dac_clock_enabled(false)
      .set_hdmi_tx_pixel_clock_enabled(false)
      .set_lcd_analog_clock_phy3_enabled(false)
      .set_lcd_analog_clock_phy2_enabled(false)
      .WriteTo(&hhi_mmio_);

  // dmc_arb_config
  vpu_mmio_.Write32(0x0, VPU_RDARB_MODE_L1C1);
  vpu_mmio_.Write32(0x10000, VPU_RDARB_MODE_L1C2);
  vpu_mmio_.Write32(0x900000, VPU_RDARB_MODE_L2C1);
  vpu_mmio_.Write32(0x20000, VPU_WRARB_MODE_L2C1);
}

namespace {

template <typename RegisterType>
void SetPowerBits(fdf::MmioBuffer& mmio, bool powered_on, int begin_bit_index, int end_bit_index) {
  // TODO(fxbug.com/132123): AMLogic-supplied software flips each bit
  // individually, with a 5us delay between flips. The power sequences in the
  // datasheets have no mention of individual bits, and seem to suggest setting
  // each register to its final value in one operation. Carry out an experiment
  // to document if the datasheet sequences work, as the AMLogic software may
  // cater to older chips.  Document the result either way.
  RegisterType power_register = RegisterType::Get().ReadFrom(&mmio);
  const uint32_t bit_value = powered_on ? 0 : 1;
  for (int bit_index = begin_bit_index; bit_index < end_bit_index; ++bit_index) {
    const uint32_t bit_mask = bit_value << bit_index;
    power_register.set_reg_value(power_register.reg_value() & ~bit_mask).WriteTo(&mmio);
    zx::nanosleep(zx::deadline_after(zx::usec(5)));
  }
}

template <typename RegisterType>
void SetPowerUnits(fdf::MmioBuffer& mmio, bool powered_on, int begin_unit_index,
                   int end_unit_index) {
  // TODO(fxbug.com/132123): AMLogic-supplied software flips each unit
  // individually, with a 5us delay between flips. The power sequences in the
  // datasheets have no mention of individual bits, and seem to suggest setting
  // each register to its final value in one operation. Carry out an experiment
  // to document if the datasheet sequences work, as the AMLogic software may
  // cater to older chips.  Document the result either way.
  RegisterType power_register = RegisterType::Get().ReadFrom(&mmio);
  const MemoryPowerDomainMode mode =
      powered_on ? MemoryPowerDomainMode::kPoweredOn : MemoryPowerDomainMode::kPoweredOff;
  for (int unit_index = begin_unit_index; unit_index < end_unit_index; ++unit_index) {
    const uint32_t unit_mask = static_cast<uint32_t>(mode) << unit_index;
    power_register.set_reg_value(power_register.reg_value() & ~unit_mask).WriteTo(&mmio);
    zx::nanosleep(zx::deadline_after(zx::usec(5)));
  }
}

}  // namespace

void Vpu::PowerOn() {
  // Implements the power sequences documented below.
  //
  // A311D datasheet Section 8.2.3 "EE Top Level Power Modes", Table 8-6 "Power
  //     Sequence of VPU", page 88
  // S905D3 datasheet Section 6.2.3.2 "EE Top Level Power Modes" > "VPU",
  //     Table 6-3 "Power & Global Clock Control Summary", page 75
  // S905D2 datasheet Section 6.2.3 "EE Top Level Power Modes", Table 6-4 "Power
  //     Sequence of EE Domain", page 84

  auto general_power = AlwaysOnGeneralPowerSleep::Get().ReadFrom(&aobus_mmio_);
  general_power.set_vpu_hdmi_powered_off(false).WriteTo(&aobus_mmio_);

  // TODO(fxbug.com/132123): The A311D power sequence waits for bits 9-8 in
  // `AlwaysOnGeneralPowerAck`here. The S905D3 and S905D2 power sequences only
  // wait for bit 8 in the same register. AMLogic-supplied bringup code uses a
  // hard-coded 20us timeout instead.

  SetPowerUnits<VpuMemoryPower0>(hhi_mmio_, /*powered_on=*/true, 0, 16);
  SetPowerUnits<VpuMemoryPower1>(hhi_mmio_, /*powered_on=*/true, 0, 16);

  // The S905D2 power sequence does not include `VpuMemoryPower2`. However, the
  // datasheet has a register-level reference for it, which indicates that all
  // fields outside of `vpp_watermark_power` are unused. So, the sequence below
  // is harmless on S905D2.
  //
  // The A311D power sequence lists bits 0-31 of `VpuMemoryPower2`. We follow
  // the AMLogic-supplied bringup code, which only flips the bits below.

  // TODO(fxbug.com/132123): The S905D3 power sequence and AMLogic-supplied
  // bringup code flips bits 0-31 of `VpuMemoryPower2`.
  SetPowerUnits<VpuMemoryPower2>(hhi_mmio_, /*powered_on=*/true, 0, 1);
  SetPowerUnits<VpuMemoryPower2>(hhi_mmio_, /*powered_on=*/true, 2, 9);
  SetPowerUnits<VpuMemoryPower2>(hhi_mmio_, /*powered_on=*/true, 15, 16);

  // TODO(fxbug.com/132123): The S905D3 power sequence also flips bits 0-31 of
  // registers `VpuMemoryPower3` and `VpuMemoryPower4`. The AMLogic-supplied
  // bringup code flips bits 0-31 of `VpuMemoryPower3` and bits 0-3 of
  // `VpuMemoryPower4`.

  SetPowerBits<MemoryPower0>(hhi_mmio_, /*powered_on=*/true, 8, 16);
  zx::nanosleep(zx::deadline_after(zx::usec(20)));

  // Reset VIU + VENC
  // Reset VENCI + VENCP + VADC + VENCL
  // Reset HDMI-APB + HDMI-SYS + HDMI-TX + HDMI-CEC
  reset_mmio_.Write32(
      reset_mmio_.Read32(RESET0_LEVEL) & ~((1 << 5) | (1 << 10) | (1 << 19) | (1 << 13)),
      RESET0_LEVEL);
  reset_mmio_.Write32(reset_mmio_.Read32(RESET1_LEVEL) & ~(1 << 5), RESET1_LEVEL);
  reset_mmio_.Write32(reset_mmio_.Read32(RESET2_LEVEL) & ~(1 << 15), RESET2_LEVEL);
  reset_mmio_.Write32(
      reset_mmio_.Read32(RESET4_LEVEL) &
          ~((1 << 6) | (1 << 7) | (1 << 13) | (1 << 5) | (1 << 9) | (1 << 4) | (1 << 12)),
      RESET4_LEVEL);
  reset_mmio_.Write32(reset_mmio_.Read32(RESET7_LEVEL) & ~(1 << 7), RESET7_LEVEL);

  // TODO(fxbug.com/132123): The A311D and S905D3 power sequences disable output
  // isolation after VPU power ACK, and before changing the VPU memory power
  // registers. The AMLogic-supplied bringup code disables isolation after
  // completing the reset sequence.

  // TODO(fxbug.com/132123): The S905D3 power sequence and AMLogic-supplied
  // bringup code configure VPU/HDMI isolation in the
  // `AlwaysOnGeneralPowerIsolation` register instead.
  general_power.set_vpu_hdmi_isolation_enabled_s905d2_a311d(false).WriteTo(&aobus_mmio_);

  // release Reset
  reset_mmio_.Write32(
      reset_mmio_.Read32(RESET0_LEVEL) | ((1 << 5) | (1 << 10) | (1 << 19) | (1 << 13)),
      RESET0_LEVEL);
  reset_mmio_.Write32(reset_mmio_.Read32(RESET1_LEVEL) | (1 << 5), RESET1_LEVEL);
  reset_mmio_.Write32(reset_mmio_.Read32(RESET2_LEVEL) | (1 << 15), RESET2_LEVEL);
  reset_mmio_.Write32(
      reset_mmio_.Read32(RESET4_LEVEL) |
          ((1 << 6) | (1 << 7) | (1 << 13) | (1 << 5) | (1 << 9) | (1 << 4) | (1 << 12)),
      RESET4_LEVEL);
  reset_mmio_.Write32(reset_mmio_.Read32(RESET7_LEVEL) | (1 << 7), RESET7_LEVEL);

  ConfigureClock();
}

void Vpu::PowerOff() {
  // Implements the steps in Table 8-6 "Power Sequence of VPU" in A311D
  // datasheet Section 8.2.3 "EE Top Level Power Modes", in reverse order.

  auto general_power = AlwaysOnGeneralPowerSleep::Get().ReadFrom(&aobus_mmio_);
  general_power.set_vpu_hdmi_isolation_enabled_s905d2_a311d(true).WriteTo(&aobus_mmio_);
  zx::nanosleep(zx::deadline_after(zx::usec(20)));

  // TODO(fxbug.com/132123): The memories are powered down in exactly the same
  // order as powering up. If the order doesn't matter, we can unify the code.

  SetPowerUnits<VpuMemoryPower0>(hhi_mmio_, /*powered_on=*/false, 0, 16);
  SetPowerUnits<VpuMemoryPower1>(hhi_mmio_, /*powered_on=*/false, 0, 16);

  // The S905D2 power sequence does not include `VpuMemoryPower2`. However, the
  // datasheet has a register-level reference for it, which indicates that all
  // fields outside of `vpp_watermark_power` are unused. So, the sequence below
  // is harmless on S905D2.

  SetPowerUnits<VpuMemoryPower2>(hhi_mmio_, /*powered_on=*/false, 0, 1);
  SetPowerUnits<VpuMemoryPower2>(hhi_mmio_, /*powered_on=*/false, 2, 9);
  SetPowerUnits<VpuMemoryPower2>(hhi_mmio_, /*powered_on=*/false, 15, 16);

  SetPowerBits<MemoryPower0>(hhi_mmio_, /*powered_on=*/false, 8, 16);
  zx::nanosleep(zx::deadline_after(zx::usec(20)));

  // TODO(fxbug.com/132123): The A311D power sequence waits for bits 9-8 in
  // `AlwaysOnGeneralPowerAck`here. The S905D3 and S905D2 power sequences only
  // wait for bit 8 in the same register.

  general_power.set_vpu_hdmi_powered_off(true).WriteTo(&aobus_mmio_);

  VideoAdvancedPeripheralBusClockControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_branch0_mux_enabled(false)
      .WriteTo(&hhi_mmio_);
  VpuClockControl::Get().ReadFrom(&hhi_mmio_).set_branch0_mux_enabled(false).WriteTo(&hhi_mmio_);
}

void Vpu::AfbcPower(bool power_on) {
  auto vpu_memory_power2 = VpuMemoryPower2::Get().ReadFrom(&hhi_mmio_);
  vpu_memory_power2.set_mali_afbc_decoder_power(power_on ? MemoryPowerDomainMode::kPoweredOn
                                                         : MemoryPowerDomainMode::kPoweredOff);
  vpu_memory_power2.WriteTo(&hhi_mmio_);
  zx::nanosleep(zx::deadline_after(zx::usec(5)));
}

zx_status_t Vpu::CaptureInit(uint8_t canvas_idx, uint32_t height, uint32_t stride) {
  fbl::AutoLock lock(&capture_mutex_);
  if (capture_state_ == CAPTURE_ACTIVE) {
    fdf::error("Capture in progress");
    return ZX_ERR_UNAVAILABLE;
  }

  // Set up sources for writeback mux 0.
  WritebackMuxControl::Get()
      .ReadFrom(&vpu_mmio_)
      .SetMux0Selection(WritebackMuxSource::kDisabled)
      .WriteTo(&vpu_mmio_);
  WritebackMuxControl::Get()
      .ReadFrom(&vpu_mmio_)
      .SetMux0Selection(WritebackMuxSource::kViuWriteback0)
      .WriteTo(&vpu_mmio_);
  WrBackMiscCtrlReg::Get().ReadFrom(&vpu_mmio_).set_chan0_hsync_enable(1).WriteTo(&vpu_mmio_);
  WrBackCtrlReg::Get().ReadFrom(&vpu_mmio_).set_chan0_sel(5).WriteTo(&vpu_mmio_);

  // setup hold lines and vdin selection to internal loopback
  VideoInputCommandControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_hold_lines(0)
      .set_input_source_selection(VideoInputCommandControl::InputSource::kWritebackMux0)
      .WriteTo(&vpu_mmio_);

  VideoInputLinearFifoControl::Get(kVideoInputModuleId)
      .FromValue(0)
      .set_linear_fifo_buffer_size_pixels(1920)
      .WriteTo(&vpu_mmio_);

  // Setup input channel FIFO.
  VideoInputChannelFifoControl3::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_channel6_data_enabled(true)
      .set_channel6_go_field_signal_enabled(true)
      .set_channel6_go_line_signal_enabled(true)
      .set_channel6_input_vsync_is_negative(false)
      .set_channel6_input_hsync_is_negative(false)
      .set_channel6_async_fifo_software_reset_on_vsync(true)
      .set_channel6_clear_fifo_overflow_bit(false)
      .set_channel6_async_fifo_software_reset(false)
      .WriteTo(&vpu_mmio_);

  VdInMatrixCtrlReg::Get().ReadFrom(&vpu_mmio_).set_select(1).set_enable(1).WriteTo(&vpu_mmio_);

  VdinCoef00_01Reg::Get()
      .ReadFrom(&vpu_mmio_)
      .set_coef00(capture_yuv2rgb_coeff[0][0])
      .set_coef01(capture_yuv2rgb_coeff[0][1])
      .WriteTo(&vpu_mmio_);

  VdinCoef02_10Reg::Get()
      .ReadFrom(&vpu_mmio_)
      .set_coef02(capture_yuv2rgb_coeff[0][2])
      .set_coef10(capture_yuv2rgb_coeff[1][0])
      .WriteTo(&vpu_mmio_);

  VdinCoef11_12Reg::Get()
      .ReadFrom(&vpu_mmio_)
      .set_coef11(capture_yuv2rgb_coeff[1][1])
      .set_coef12(capture_yuv2rgb_coeff[1][2])
      .WriteTo(&vpu_mmio_);

  VdinCoef20_21Reg::Get()
      .ReadFrom(&vpu_mmio_)
      .set_coef20(capture_yuv2rgb_coeff[2][0])
      .set_coef21(capture_yuv2rgb_coeff[2][1])
      .WriteTo(&vpu_mmio_);

  VdinCoef22Reg::Get()
      .ReadFrom(&vpu_mmio_)
      .set_coef22(capture_yuv2rgb_coeff[2][2])
      .WriteTo(&vpu_mmio_);

  VdinOffset0_1Reg::Get()
      .ReadFrom(&vpu_mmio_)
      .set_offset0(capture_yuv2rgb_offset[0])
      .set_offset1(capture_yuv2rgb_offset[1])
      .WriteTo(&vpu_mmio_);

  VdinOffset2Reg::Get()
      .ReadFrom(&vpu_mmio_)
      .set_offset2(capture_yuv2rgb_offset[2])
      .WriteTo(&vpu_mmio_);

  VdinPreOffset0_1Reg::Get()
      .ReadFrom(&vpu_mmio_)
      .set_preoffset0(capture_yuv2rgb_preoffset[0])
      .set_preoffset1(capture_yuv2rgb_preoffset[1])
      .WriteTo(&vpu_mmio_);

  VdinPreOffset2Reg::Get()
      .ReadFrom(&vpu_mmio_)
      .set_preoffset2(capture_yuv2rgb_preoffset[2])
      .WriteTo(&vpu_mmio_);

  // Setup the input dimensions for video input module (VDIN1)'s memory
  // interface.
  ZX_DEBUG_ASSERT(stride > 0);
  VideoInputInterfaceWidth::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_width_px(stride)
      .WriteTo(&vpu_mmio_);

  // Configure write range on the capture buffer image.
  VideoInputWriteRangeHorizontal::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_endianness_128_bit(VideoInputWriteRangeHorizontal::Endianness::kBig)
      .SetHorizontalRange(0, stride - 1)
      .WriteTo(&vpu_mmio_);
  VideoInputWriteRangeVertical::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .SetVerticalRange(0, height - 1)
      .WriteTo(&vpu_mmio_);

  VideoInputWriteControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_line_end_indicator_selection(VideoInputWriteControl::LineEndIndicator::kWidth)
      // The memory interface by default writes pixels in 128-bit big-endian
      // format. By swapping consecutive 64-bit pairs before writing, it writes
      // pixels in 64-bit big-endian format. The canvas format must be
      // configured to match this format.
      .set_consecutive_64bit_pairs_reordered(true)
      .set_yuv_sampling_storage_format(
          VideoInputWriteControl::YuvSamplingStorageFormat::kYuv444Packed)
      .set_luma_canvas_address(canvas_idx)
      .WriteTo(&vpu_mmio_);

  // TODO(fxbug.com/132123): This seems unnecessary. Vpu::PowerOn() already
  // enables both `vdin0_memory_power` and `vdin1_memory_power`. The
  // AMLogic-supplied bringup code waits 5us after flipping a power gate.
  auto vpu_memory_power0 = VpuMemoryPower0::Get().ReadFrom(&hhi_mmio_);
  vpu_memory_power0.set_vdin1_memory_power(MemoryPowerDomainMode::kPoweredOn).WriteTo(&hhi_mmio_);

  // Capture state is now in IDLE mode
  capture_state_ = CAPTURE_IDLE;
  return ZX_OK;
}

zx_status_t Vpu::CaptureStart() {
  fbl::AutoLock lock(&capture_mutex_);
  if (capture_state_ != CAPTURE_IDLE) {
    fdf::error("Capture state is not idle! ({})", static_cast<int>(capture_state_));
    return ZX_ERR_BAD_STATE;
  }

  // Now that loopback mode is configured, start capture

  // Pause writing to the memory while the VDIN is being configured.
  VideoInputWriteControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_write_request_unpaused(false)
      .WriteTo(&vpu_mmio_);

  // Disable input path for VDIN.
  VideoInputCommandControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_video_input_enabled(false)
      .WriteTo(&vpu_mmio_);

  // Reset the memory interface.
  VideoInputMiscellaneousControl::Get()
      .ReadFrom(&vpu_mmio_)
      .set_video_input_module1_memory_interface_reset(true)
      .WriteTo(&vpu_mmio_);
  zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
  VideoInputMiscellaneousControl::Get()
      .ReadFrom(&vpu_mmio_)
      .set_video_input_module1_memory_interface_reset(false)
      .WriteTo(&vpu_mmio_);

  // Resumes writing to the memory.
  VideoInputWriteControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_write_request_unpaused(true)
      .WriteTo(&vpu_mmio_);

  // wait until resets finishes
  zx_nanosleep(zx_deadline_after(ZX_MSEC(20)));

  // Clear status bit
  VideoInputWriteControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_direct_write_done_cleared(true)
      .WriteTo(&vpu_mmio_);

  // Set as urgent
  VideoInputWriteControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_write_request_urgent(true)
      .WriteTo(&vpu_mmio_);

  // Enable loopback
  VideoInputWriteControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_write_request_enabled(true)
      .WriteTo(&vpu_mmio_);

  // enable vdin path
  VideoInputCommandControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_video_input_enabled(true)
      .WriteTo(&vpu_mmio_);

  capture_state_ = CAPTURE_ACTIVE;
  return ZX_OK;
}

zx_status_t Vpu::CaptureDone() {
  fbl::AutoLock lock(&capture_mutex_);
  capture_state_ = CAPTURE_IDLE;
  // pause write output
  VideoInputWriteControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_write_request_unpaused(false)
      .WriteTo(&vpu_mmio_);

  // disable vdin path
  VideoInputCommandControl::Get(kVideoInputModuleId)
      .ReadFrom(&vpu_mmio_)
      .set_video_input_enabled(false)
      .WriteTo(&vpu_mmio_);

  // Reset memory interface.
  VideoInputMiscellaneousControl::Get()
      .ReadFrom(&vpu_mmio_)
      .set_video_input_module1_memory_interface_reset(true)
      .WriteTo(&vpu_mmio_);
  zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
  VideoInputMiscellaneousControl::Get()
      .ReadFrom(&vpu_mmio_)
      .set_video_input_module1_memory_interface_reset(false)
      .WriteTo(&vpu_mmio_);

  return ZX_OK;
}

void Vpu::CapturePrintRegisters() {
  fdf::info("** Display Loopback Register Dump **");
  fdf::info("VdInComCtrl0Reg = 0x{:x}",
            VideoInputCommandControl::Get(kVideoInputModuleId).ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdInComStatus0Reg = 0x{:x}",
            VideoInputCommandStatus0::Get(kVideoInputModuleId).ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdInMatrixCtrlReg = 0x{:x}",
            VdInMatrixCtrlReg::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinCoef00_01Reg = 0x{:x}", VdinCoef00_01Reg::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinCoef02_10Reg = 0x{:x}", VdinCoef02_10Reg::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinCoef11_12Reg = 0x{:x}", VdinCoef11_12Reg::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinCoef20_21Reg = 0x{:x}", VdinCoef20_21Reg::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinCoef22Reg = 0x{:x}", VdinCoef22Reg::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinOffset0_1Reg = 0x{:x}", VdinOffset0_1Reg::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinOffset2Reg = 0x{:x}", VdinOffset2Reg::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinPreOffset0_1Reg = 0x{:x}",
            VdinPreOffset0_1Reg::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinPreOffset2Reg = 0x{:x}",
            VdinPreOffset2Reg::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinLFifoCtrlReg = 0x{:x}\n",
            VideoInputLinearFifoControl::Get(kVideoInputModuleId).ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdinIntfWidthM1Reg = 0x{:x}\n",
            VideoInputInterfaceWidth::Get(kVideoInputModuleId).ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdInWrCtrlReg = 0x{:x}\n",
            VideoInputWriteControl::Get(kVideoInputModuleId).ReadFrom(&vpu_mmio_).reg_value());
  fdf::info(
      "VdInWrHStartEndReg = 0x{:x}\n",
      VideoInputWriteRangeHorizontal::Get(kVideoInputModuleId).ReadFrom(&vpu_mmio_).reg_value());
  fdf::info(
      "VdInWrVStartEndReg = 0x{:x}\n",
      VideoInputWriteRangeVertical::Get(kVideoInputModuleId).ReadFrom(&vpu_mmio_).reg_value());
  fdf::info(
      "VdInAFifoCtrl3Reg = 0x{:x}\n",
      VideoInputChannelFifoControl3::Get(kVideoInputModuleId).ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdInMiscCtrlReg = 0x{:x}\n",
            VideoInputMiscellaneousControl::Get().ReadFrom(&vpu_mmio_).reg_value());
  fdf::info("VdInIfMuxCtrlReg = 0x{:x}\n",
            WritebackMuxControl::Get().ReadFrom(&vpu_mmio_).reg_value());

  fdf::info("Dumping from 0x1300 to 0x1373");
  for (int i = 0x1300; i <= 0x1373; i++) {
    fdf::info("reg[0x{:x}] = 0x{:x}", i, vpu_mmio_.Read32(i << 2));
  }
}

}  // namespace amlogic_display
