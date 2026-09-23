// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/hdmi-transmitter.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/syscalls/smc.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>

#include <fbl/auto_lock.h>
#include <fbl/vector.h>

#include "src/graphics/display/drivers/amlogic-display/hdmi-transmitter-top-regs.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/designware-hdmi/color-param.h"
#include "src/graphics/display/lib/designware-hdmi/hdmi-transmitter-controller.h"

// References
//
// The code contains references to the following documents.
//
// - ANSI/CTA-861-I: A DTV Profile for Uncompressed High Speed Digital
//   Interfaces, Consumer Technology Association (CTA), dated February 2023.
//   Referenced as "CTA-861 standard" in the CTA-861 standard.
//   Available at
//   https://shop.cta.tech/collections/standards/products/a-dtv-profile-for-uncompressed-high-speed-digital-interfaces-ansi-cta-861-i

namespace amlogic_display {

HdmiTransmitter::HdmiTransmitter(
    std::unique_ptr<designware_hdmi::HdmiTransmitterController> designware_controller,
    fdf::MmioBuffer hdmitx_top_level_mmio, zx::resource silicon_provider_service_smc)
    : designware_controller_(std::move(designware_controller)),
      hdmitx_top_level_mmio_(std::move(hdmitx_top_level_mmio)),
      silicon_provider_service_smc_(std::move(silicon_provider_service_smc)) {
  ZX_DEBUG_ASSERT(designware_controller_ != nullptr);

  // TODO(https://fxbug.dev/42074342): `silicon_provider_service_smc` may be invalid in
  // tests where fake SMC resource objects are not yet supported. Once fake SMC
  // is supported, we should add an assertion to enforce
  // `silicon_provider_service_smc` to be always valid.
}

zx::result<> HdmiTransmitter::Reset() {
  // TODO(https://fxbug.dev/42148838): Add in Resets
  // reset hdmi related blocks (HIU, HDMI SYS, HDMI_TX)
  // auto reset0_result = display->reset_register_.WriteRegister32(PRESET0_REGISTER, 1 << 19, 1 <<
  // 19); if ((reset0_result.status() != ZX_OK) || reset0_result->is_error()) {
  //   fdf::error( "Reset0 Write failed\n");
  // }

  /* FIXME: This will reset the entire HDMI subsystem including the HDCP engine.
   * At this point, we have no way of initializing HDCP block, so we need to
   * skip this for now.
   */
  // auto reset2_result = display->reset_register_.WriteRegister32(PRESET2_REGISTER, 1 << 15, 1 <<
  // 15); // Will mess up hdcp stuff if ((reset2_result.status() != ZX_OK) ||
  // reset2_result->is_error()) {
  //   fdf::error( "Reset2 Write failed\n");
  // }

  // auto reset2_result = display->reset_register_.WriteRegister32(PRESET2_REGISTER, 1 << 2, 1 <<
  // 2); if ((reset2_result.status() != ZX_OK) || reset2_result->is_error()) {
  //   fdf::error( "Reset2 Write failed\n");
  // }

  // Bring HDMI out of reset
  WriteTopLevelReg(HDMITX_TOP_SW_RESET, 0);
  usleep(200);
  WriteTopLevelReg(HDMITX_TOP_CLK_CNTL, 0x000000ff);

  fbl::AutoLock lock(&dw_lock_);
  zx_status_t status = designware_controller_->InitHw();

  return zx::make_result(status);
}

namespace {

void CalculateTxParam(const display::DisplayTiming& display_timing,
                      designware_hdmi::hdmi_param_tx* p) {
  p->is4K = display_timing.pixel_clock_frequency_hz > 500'000'000;

  // The aspect ratio field in the Auxiliary Video Information (AVI) InfoFrame.
  // Values are defined in the CTA-861 standard, Section 6.4.1 "Video Format,
  // Picture Aspect Ratio and Pixel Repetition", Table 14 "AVI InfoFrame
  // Picture Aspect Ratio Field, Data Byte 2", page 76.
  //
  // TODO(https://fxbug.dev/42085777): Revise the AVI InfoFrame naming and move the
  // values to a dedicated header.
  static constexpr uint8_t kHdmiAspectRatio4x3 = 1;
  static constexpr uint8_t kHdmiAspectRatio16x9 = 2;
  static constexpr uint8_t kHdmiAspectRatioNone = 0;
  if (display_timing.horizontal_active_px * 3 == display_timing.vertical_active_lines * 4) {
    p->aspect_ratio = kHdmiAspectRatio4x3;
  } else if (display_timing.horizontal_active_px * 9 == display_timing.vertical_active_lines * 16) {
    p->aspect_ratio = kHdmiAspectRatio16x9;
  } else {
    p->aspect_ratio = kHdmiAspectRatioNone;
  }

  // The colorimetry field in the AVI InfoFrame.
  // Values are defined in the CTA-861 standard, Section 6.4.2 "Color Component
  // Sample Format and Colorimetry", Table 19 "AVI InfoFrame Colorimetry
  // Fields", page 81.
  //
  // The colorimetry of SMPTE ST 170, commonly known as the "NTSC standard".
  static constexpr uint8_t kHdmiColorimetryNtsc = 1;

  // TODO(https://fxbug.dev/42085819): Revise AVI InfoFrame values set by the driver.
  p->colorimetry = kHdmiColorimetryNtsc;
}

}  // namespace

zx::result<> HdmiTransmitter::ModeSet(const display::DisplayTiming& timing,
                                      const designware_hdmi::ColorParam& color) {
  designware_hdmi::hdmi_param_tx p;
  CalculateTxParam(timing, &p);

  // Output normal TMDS Data
  WriteTopLevelReg(HDMITX_TOP_BIST_CNTL, 1 << 12);

  // Configure HDMI TX IP
  fbl::AutoLock lock(&dw_lock_);
  designware_controller_->ConfigHdmitx(color, timing, p);

  // Initialize HDCP 1.4.
  //
  // AMLogic-provided bringup code initializes HDCP before clearing
  // interrupts on the DesignWare HDMI IP's. Following the same sequence
  // would be difficult given our current layering, as we clear interrupts
  // in HdmiDw::ConfigHdmitx().
  //
  // Fortunately, experiments on VIM3 (using A311D) show that the HDCP
  // initialization SMC still works if invoked after the interrupts are
  // cleared.
  if (silicon_provider_service_smc_.is_valid()) {
    zx::result<> hdcp_initialization_result = InitializeHdcp14();
    if (hdcp_initialization_result.is_error()) {
      fdf::error("Failed to initialize HDCP 1.4: {}", hdcp_initialization_result);
      return hdcp_initialization_result;
    }
  } else {
    // TODO(https://fxbug.dev/42074342): This could occur in tests where fake SMC
    // resource objects are not yet supported. Once fake SMC is supported, we
    // should enforce `smc_` to be always valid and always issue a
    // `zx_smc_call()` syscall.
    fdf::warn(
        "Secure monitor call (SMC) resource is not available. "
        "Skipping initializing HDCP 1.4.");
  }

  WriteTopLevelReg(HDMITX_TOP_INTR_STAT_CLR, 0x0000001f);
  designware_controller_->SetupInterrupts();
  WriteTopLevelReg(HDMITX_TOP_INTR_MASKN, 0x9f);
  designware_controller_->Reset();

  if (p.is4K) {
    // Setup TMDS Clocks (taken from recommended test pattern in DVI spec)
    WriteTopLevelReg(HDMITX_TOP_TMDS_CLK_PTTN_01, 0);
    WriteTopLevelReg(HDMITX_TOP_TMDS_CLK_PTTN_23, 0x03ff03ff);
  } else {
    WriteTopLevelReg(HDMITX_TOP_TMDS_CLK_PTTN_01, 0x001f001f);
    WriteTopLevelReg(HDMITX_TOP_TMDS_CLK_PTTN_23, 0x001f001f);
  }
  designware_controller_->SetFcScramblerCtrl(p.is4K);

  WriteTopLevelReg(HDMITX_TOP_TMDS_CLK_PTTN_CNTL, 0x1);
  usleep(2);
  WriteTopLevelReg(HDMITX_TOP_TMDS_CLK_PTTN_CNTL, 0x2);

  designware_controller_->SetupScdc(p.is4K);
  designware_controller_->ResetFc();

  return zx::ok();
}

zx::result<fbl::Vector<uint8_t>> HdmiTransmitter::ReadExtendedEdid() {
  fbl::AutoLock lock(&dw_lock_);
  // HdmiTransmitterController already logs its errors, so we don't need to
  // print additional error logs.
  return designware_controller_->ReadExtendedEdid();
}

void HdmiTransmitter::WriteTopLevelReg(uint32_t addr, uint32_t val) {
  hdmitx_top_level_mmio_.Write32(val, addr);
}

uint32_t HdmiTransmitter::ReadTopLevelReg(uint32_t addr) {
  return hdmitx_top_level_mmio_.Read32(addr);
}

void HdmiTransmitter::PrintRegister(const char* register_name, uint32_t register_address) {
  fdf::info("{} (0x{:04x}): {}", register_name, register_address,
            ReadTopLevelReg(register_address));
}

void HdmiTransmitter::PrintTopLevelRegisters() {
  fdf::info("------------Top Registers------------");
  PrintRegister("HDMITX_TOP_SW_RESET", HDMITX_TOP_SW_RESET);
  PrintRegister("HDMITX_TOP_CLK_CNTL", HDMITX_TOP_CLK_CNTL);
  PrintRegister("HDMITX_TOP_INTR_MASKN", HDMITX_TOP_INTR_MASKN);
  PrintRegister("HDMITX_TOP_INTR_STAT_CLR", HDMITX_TOP_INTR_STAT_CLR);
  PrintRegister("HDMITX_TOP_BIST_CNTL", HDMITX_TOP_BIST_CNTL);
  PrintRegister("HDMITX_TOP_TMDS_CLK_PTTN_01", HDMITX_TOP_TMDS_CLK_PTTN_01);
  PrintRegister("HDMITX_TOP_TMDS_CLK_PTTN_23", HDMITX_TOP_TMDS_CLK_PTTN_23);
  PrintRegister("HDMITX_TOP_TMDS_CLK_PTTN_CNTL", HDMITX_TOP_TMDS_CLK_PTTN_CNTL);
}

void HdmiTransmitter::PrintRegisters() {
  PrintTopLevelRegisters();

  fbl::AutoLock lock(&dw_lock_);
  designware_controller_->PrintRegisters();
}

zx::result<> HdmiTransmitter::InitializeHdcp14() {
  static constexpr zx_smc_parameters_t params = {
      // Silicon Provider secure monitor call: "HDCP14_INIT".
      .func_id = 0x82000012,
  };
  zx_smc_result_t result = {};
  zx_status_t status = zx_smc_call(silicon_provider_service_smc_.get(), &params, &result);
  if (status != ZX_OK) {
    fdf::error("Failed to initialize HDCP 1.4: {}", zx::make_result(status));
    return zx::error(status);
  }
  return zx::ok();
}

}  // namespace amlogic_display
