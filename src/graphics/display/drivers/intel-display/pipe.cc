// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/pipe.h"

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>

#include <algorithm>
#include <cfloat>
#include <cinttypes>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <optional>
#include <span>
#include <utility>
#include <vector>

#include "src/graphics/display/drivers/intel-display/gtt.h"
#include "src/graphics/display/drivers/intel-display/hardware-common.h"
#include "src/graphics/display/drivers/intel-display/power.h"
#include "src/graphics/display/drivers/intel-display/registers-pipe-scaler.h"
#include "src/graphics/display/drivers/intel-display/registers-pipe.h"
#include "src/graphics/display/drivers/intel-display/registers-transcoder.h"
#include "src/graphics/display/drivers/intel-display/tiling.h"
#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/display/lib/driver-utils/poll-until.h"

namespace {

uint32_t float_to_intel_display_csc_offset(float f) {
  ZX_DEBUG_ASSERT(0 <= f && f < 1.0f);  // Controller::CheckConfiguration validates this

  // f is in [0, 1). Multiply by 2^12 to convert to a 12-bit fixed-point fraction.
  return static_cast<uint32_t>(f * pow(FLT_RADIX, 12));
}

uint32_t float_to_intel_display_csc_coefficient(float f) {
  registers::CscCoeffFormat res;
  if (f < 0) {
    f *= -1;
    res.set_sign(1);
  }

  if (f < .125) {
    res.set_exponent(res.kExponent0125);
    f /= .125f;
  } else if (f < .25) {
    res.set_exponent(res.kExponent025);
    f /= .25f;
  } else if (f < .5) {
    res.set_exponent(res.kExponent05);
    f /= .5f;
  } else if (f < 1) {
    res.set_exponent(res.kExponent1);
  } else if (f < 2) {
    res.set_exponent(res.kExponent2);
    f /= 2.0f;
  } else {
    res.set_exponent(res.kExponent4);
    f /= 4.0f;
  }
  f = (f * 512) + .5f;

  if (f >= 512) {
    res.set_mantissa(0x1ff);
  } else {
    res.set_mantissa(static_cast<uint16_t>(f));
  }

  return res.reg_value();
}

uint32_t encode_pipe_color_component(uint8_t component) {
  // Convert to unsigned .10 fixed point format
  return component << 2;
}

}  // namespace

namespace intel_display {

Pipe::Pipe(fdf::MmioBuffer* mmio_space, registers::Platform platform, PipeId pipe_id,
           PowerWellRef pipe_power)
    : mmio_space_(mmio_space),
      platform_(platform),
      pipe_id_(pipe_id),
      pipe_power_(std::move(pipe_power)) {}

// static
void Pipe::ResetTranscoder(TranscoderId transcoder_id, registers::Platform platform,
                           fdf::MmioBuffer* mmio_space) {
  registers::TranscoderRegs transcoder_regs(transcoder_id);

  // Disable transcoder and wait for it to stop. These are the "Disable
  // Transcoder" steps from:
  //
  // Tiger Lake - IHD-OS-TGL-Vol 12-12.21
  // * "DSI Transcoder Disable Sequence" pages 128-129 (Incomplete)
  // * "Sequences for DisplayPort" > "Disable Sequence" pages 147-148 (Incomplete)
  // * "Sequences for HDMI and DVI" > "Disable Sequence" pages 150-151
  // * "Sequences for WD" > "Disable Sequence" pages 151-152 (Incomplete)
  // Kaby Lake - IHD-OS-KBL-Vol 12-1.17
  // * "Sequences for DisplayPort" > "Disable Sequence" pages 115-116 (Incomplete)
  // * "Sequences for HDMI" > "Disable Sequence" page 118
  // Skylake - IHD-OS-SKL-Vol 12-05.16
  // * "Sequences for DisplayPort" > "Disable Sequence" pages 115-116 (Incomplete)
  // * "Sequences for HDMI and DVI" > "Disable Sequence" page 118
  //
  // The transcoder should be turned off only after the associated backlight,
  // audio, and image planes are disabled.
  auto transcoder_config = transcoder_regs.Config().ReadFrom(mmio_space);

  // Our experiments on NUC 11 indicate that the display engine may crash the
  // whole system if the driver sets `enabled_target` to false and writes the
  // transcoder configuration register when the transcoder is already disabled,
  // so we avoid crashing the system by only writing the register when the
  // transcoder is currently enabled. To be on the safe side, we use the same
  // caution on Kaby Lake and Skylake display engines as well.
  if (transcoder_config.enabled()) {
    transcoder_config.set_enabled_target(false).WriteTo(mmio_space);
  } else {
    fdf::trace("ResetTranscoder() skipping already-disabled control for transcoder {}",
               transcoder_id);
    fdf::trace("Transcoder {} control register: {:x}", transcoder_id,
               transcoder_config.reg_value());
  }

  if (platform == registers::Platform::kTigerLake) {
    auto transcoder_chicken = transcoder_regs.Chicken().ReadFrom(mmio_space);
    fdf::trace("ResetTranscoder() - Transcoder {} chicken register: {:x}", transcoder_id,
               transcoder_chicken.reg_value());
    if (transcoder_chicken.override_forward_error_correction_tiger_lake()) {
      fdf::info("Disabling FEC override chicken bit for transcoder {}", transcoder_id);
      transcoder_chicken.set_override_forward_error_correction_tiger_lake(false).WriteTo(
          mmio_space);

      // TODO(https://fxbug.dev/42061773): Remove this warning once we support DisplayPort
      // MST (Multi-Stream).
      fdf::warn("Transcoder {} was using a DisplayPort MST feature. Reset may be incomplete.",
                transcoder_id);
    }
  }

  // Wait for off status in TRANS_CONF, timeout after two frames.
  // Here we wait for 60 msecs, which is enough to guarantee to include two
  // whole frames in ~50 fps.
  constexpr size_t kTransConfStatusWaitTimeoutMs = 60;
  if (!display::PollUntil([&] { return !transcoder_config.ReadFrom(mmio_space).enabled(); },
                          zx::msec(1), kTransConfStatusWaitTimeoutMs)) {
    // Because this is a logical "reset", we only log failures rather than
    // crashing the driver.
    fdf::warn("Failed to reset transcoder");
    return;
  }

  if (platform == registers::Platform::kTigerLake) {
    auto transcoder_variable_rate_refresh_control =
        transcoder_regs.VariableRateRefreshControl().ReadFrom(mmio_space);
    fdf::trace("ResetTranscoder() - Transcoder {} VRR register: {:x}", transcoder_id,
               transcoder_variable_rate_refresh_control.reg_value());
    if (transcoder_variable_rate_refresh_control.enabled()) {
      fdf::info("Disabling VRR (Variable Refresh Rate) for transcoder {}", transcoder_id);
      transcoder_variable_rate_refresh_control.set_enabled(false).WriteTo(mmio_space);
    }
  }

  // Disable transcoder DDI select and clock select.
  auto transcoder_ddi_control = transcoder_regs.DdiControl().ReadFrom(mmio_space);

  // Our experiments on Dell 5420 with Tiger Lake CPU indicate that the display
  // engine may crash the whole system if the driver sets `enabled` to false and
  // writes the transcoder DDI functionality configuration register when the DDI
  // functionality is already disabled. We avoid crashing the system by only
  // writing the register when the transcoder is currently enabled. To be on the
  // safe side, we use the same caution on Kaby Lake and Skylake display engines
  // as well.
  if (transcoder_ddi_control.enabled()) {
    // `set_ddi_tiger_lake()` works on both Tiger Lake and Skylake / Kaby Lake
    // when passed std::nullopt, because nullopt translates to zeroing out all the
    // field's bits, and on Kaby Lake the highest bit of "ddi_tiger_lake" is
    // reserved to be zero, so it is safe to set the whole field to zero.
    transcoder_ddi_control.set_enabled(false).set_ddi_tiger_lake(std::nullopt).WriteTo(mmio_space);
  } else {
    fdf::trace("ResetTranscoder() skipping already-disabled DDI functionality for transcoder {}",
               transcoder_id);
    fdf::trace("Transcoder {} DDI functionality control register: {:x}", transcoder_id,
               transcoder_ddi_control.reg_value());
  }

  if (transcoder_id != TranscoderId::TRANSCODER_EDP) {
    auto transcoder_clock_select = transcoder_regs.ClockSelect().ReadFrom(mmio_space);

    // `set_ddi_tiger_lake()` works on both Tiger Lake and Skylake / Kaby Lake
    // when passed std::nullopt, because nullopt translates to zeroing out all
    // the field's bits, and on Kaby Lake the highest bit of
    // "ddi_clock_tiger_lake" is reserved to be zero, so it is safe to set the
    // whole field to zero.
    transcoder_clock_select.set_ddi_clock_tiger_lake(std::nullopt).WriteTo(mmio_space);
  }
}

void Pipe::Reset() {
  // Follow the steps in "DisplayPort disable sequence" / "HDMI/DVI disable
  // sequence" to disable planes, connected transcoder and scalers (i.e. panel
  // fitter).
  //
  // TODO(https://fxbug.dev/42061773): Currently the procedure is the same for DisplayPort
  // and HDMI/DVI. This may change once DisplayPort Multistream (MST) is
  // supported.
  //
  // Skylake: IHD-OS-SKL-Vol 12-05.16,
  //          DisplayPort: Pages 113-114, "Disable Sequence", Step 2.
  //                       "Disable Planes, Pipe and Transcoder".
  //          HDMI/DVI : Pages 115-116, "Disable Sequence", Step 2.
  //                     "Disable Planes, Pipe and Transcoder".
  //
  // Kaby Lake: IHD-OS-KBL-Vol 12-1.17,
  //            DisplayPort: Pages 115-116, "Disable Sequence",
  //                         Step 2. "Disable Planes, Pipe and Transcoder".
  //            HDMI/DVI : Pages 118, "Disable Sequence", Step 2.
  //                       "Disable Planes, Pipe and Transcoder".
  //
  // Tiger Lake: IHD-OS-TGL-Vol 12-1.22-Rev 2.0,
  //             DisplayPort: Pages 147-148, "Disable Sequence",
  //                          Step 2. "If not in compliance mode: Disable
  //                          Planes, Pipe and Transcoder".
  //             HDMI/DVI: Pages 150, "Disable Sequence",
  //                       Step 2. "Disable Planes, Pipe and Transcoder".

  ResetPlanes();
  ResetActiveTranscoder();
  ResetScaler();
}

void Pipe::ResetPlanes() {
  registers::PipeRegs pipe_regs(pipe_id());

  // Disable planes, bottom color, and cursor
  const int32_t plane_count = platform_ == registers::Platform::kTigerLake ? 7 : 3;
  for (int32_t i = 0; i < plane_count; i++) {
    pipe_regs.PlaneControl(i).FromValue(0).WriteTo(mmio_space_);
    pipe_regs.PlaneSurface(i).FromValue(0).WriteTo(mmio_space_);
  }
  auto cursor_ctrl = pipe_regs.CursorCtrl().ReadFrom(mmio_space_);
  cursor_ctrl.set_mode_select(registers::CursorCtrl::kDisabled);
  cursor_ctrl.WriteTo(mmio_space_);
  pipe_regs.CursorBase().FromValue(0).WriteTo(mmio_space_);
  pipe_regs.PipeBottomColor().FromValue(0).WriteTo(mmio_space_);
}

void Pipe::ResetActiveTranscoder() {
  if (in_use()) {
    ResetTranscoder(connected_transcoder_id(), platform_, mmio_space_);
    fdf::debug("Reset active transcoder {} for pipe {}", connected_transcoder_id(), pipe_id());
  }
}

void Pipe::ResetScaler() {
  registers::PipeRegs pipe_regs(pipe_id());

  // This works for Skylake / Kaby Lake and Tiger Lake.
  // Note that Skylake / Kaby Lake doesn't have PS_CTRL_2_C documented in the
  // PRM, but experiments on Atlas (using Kaby Lake) shows that it does have
  // this scaler, so we use the same value across all generations.
  //
  // TODO(https://fxbug.dev/42071441): Verify the existence of the scaler and document
  // the experiment results.
  const int kScalerCount = 2;

  for (int scaler_num = 0; scaler_num < kScalerCount; scaler_num++) {
    auto pipe_scaler_regs = pipe_regs.PipeScalerRegs(scaler_num);
    pipe_scaler_regs.PipeScalerControlSkylake()
        .ReadFrom(mmio_space_)
        .set_is_enabled(false)
        .WriteTo(mmio_space_);
  }
}

void Pipe::Detach() {
  attached_display_id_ = display::kInvalidDisplayId;
  attached_edp_ = false;
}

void Pipe::AttachToDisplay(display::DisplayId id, bool is_edp) {
  attached_display_id_ = id;
  attached_edp_ = is_edp;
}

void Pipe::ApplyModeConfig(const display::DisplayTiming& mode) {
  registers::TranscoderRegs trans_regs(connected_transcoder_id());

  // Configure the rest of the transcoder
  uint32_t h_active = mode.horizontal_active_px - 1;
  uint32_t h_sync_start = h_active + mode.horizontal_front_porch_px;
  uint32_t h_sync_end = h_sync_start + mode.horizontal_sync_width_px;
  uint32_t h_total = h_active + mode.horizontal_blank_px();

  uint32_t v_active = mode.vertical_active_lines - 1;
  uint32_t v_sync_start = v_active + mode.vertical_front_porch_lines;
  uint32_t v_sync_end = v_sync_start + mode.vertical_sync_width_lines;
  uint32_t v_total = v_active + mode.vertical_blank_lines();

  auto h_total_reg = trans_regs.HTotal().FromValue(0);
  h_total_reg.set_count_total(h_total);
  h_total_reg.set_count_active(h_active);
  h_total_reg.WriteTo(mmio_space_);
  auto v_total_reg = trans_regs.VTotal().FromValue(0);
  v_total_reg.set_count_total(v_total);
  v_total_reg.set_count_active(v_active);
  v_total_reg.WriteTo(mmio_space_);

  auto h_sync_reg = trans_regs.HSync().FromValue(0);
  h_sync_reg.set_sync_start(h_sync_start);
  h_sync_reg.set_sync_end(h_sync_end);
  h_sync_reg.WriteTo(mmio_space_);
  auto v_sync_reg = trans_regs.VSync().FromValue(0);
  v_sync_reg.set_sync_start(v_sync_start);
  v_sync_reg.set_sync_end(v_sync_end);
  v_sync_reg.WriteTo(mmio_space_);

  // Assume it is not interlacing...
  trans_regs.VSyncShift()
      .ReadFrom(mmio_space_)
      .set_second_field_vsync_shift(0)
      .WriteTo(mmio_space_);

  // The Intel docs say that H/VBlank should be programmed with the same H/VTotal
  trans_regs.HBlank().FromValue(h_total_reg.reg_value()).WriteTo(mmio_space_);
  trans_regs.VBlank().FromValue(v_total_reg.reg_value()).WriteTo(mmio_space_);

  registers::PipeRegs pipe_regs(pipe_id());
  auto pipe_size = pipe_regs.PipeSourceSize().FromValue(0);
  pipe_size.set_horizontal_source_size_minus_one(mode.horizontal_active_px - 1);
  pipe_size.set_vertical_source_size_minus_one(mode.vertical_active_lines - 1);
  pipe_size.WriteTo(mmio_space_);
}

void Pipe::LoadActiveMode(display::DisplayTiming* mode) {
  registers::TranscoderRegs trans_regs(connected_transcoder_id());

  auto h_total_reg = trans_regs.HTotal().ReadFrom(mmio_space_);
  uint32_t h_total = h_total_reg.count_total();
  uint32_t h_active = h_total_reg.count_active();
  auto v_total_reg = trans_regs.VTotal().ReadFrom(mmio_space_);
  uint32_t v_total = v_total_reg.count_total();
  uint32_t v_active = v_total_reg.count_active();

  auto h_sync_reg = trans_regs.HSync().ReadFrom(mmio_space_);
  uint32_t h_sync_start = h_sync_reg.sync_start();
  uint32_t h_sync_end = h_sync_reg.sync_end();
  auto v_sync_reg = trans_regs.VSync().ReadFrom(mmio_space_);
  uint32_t v_sync_start = v_sync_reg.sync_start();
  uint32_t v_sync_end = v_sync_reg.sync_end();

  mode->horizontal_active_px = h_active + 1;
  mode->horizontal_front_porch_px = h_sync_start - h_active;
  mode->horizontal_sync_width_px = h_sync_end - h_sync_start;
  mode->horizontal_back_porch_px = h_total - h_sync_end;

  mode->vertical_active_lines = v_active + 1;
  mode->vertical_front_porch_lines = v_sync_start - v_active;
  mode->vertical_sync_width_lines = v_sync_end - v_sync_start;
  mode->vertical_back_porch_lines = v_total - v_sync_end;

  auto transcoder_ddi_control = trans_regs.DdiControl().ReadFrom(mmio_space_);
  mode->fields_per_frame = trans_regs.Config().ReadFrom(mmio_space_).interlaced_display()
                               ? display::FieldsPerFrame::kInterlaced
                               : display::FieldsPerFrame::kProgressive;
  mode->vsync_polarity = transcoder_ddi_control.vsync_polarity_not_inverted()
                             ? display::SyncPolarity::kPositive
                             : display::SyncPolarity::kNegative;
  mode->hsync_polarity = transcoder_ddi_control.hsync_polarity_not_inverted()
                             ? display::SyncPolarity::kPositive
                             : display::SyncPolarity::kNegative;
  mode->vblank_alternates = false;
  mode->pixel_repetition = 0;

  // If we're reusing hardware state, make sure the pipe source size matches
  // the display mode size, since we never scale pipes.
  registers::PipeRegs pipe_regs(pipe_id_);
  auto pipe_size = pipe_regs.PipeSourceSize().FromValue(0);
  pipe_size.set_horizontal_source_size_minus_one(mode->horizontal_active_px - 1);
  pipe_size.set_vertical_source_size_minus_one(mode->vertical_active_lines - 1);
  pipe_size.WriteTo(mmio_space_);
}

void Pipe::SubmitConfiguration(const display::ColorConversion& color_conversion,
                               std::span<const display::DriverLayer> layers,
                               display::DriverConfigStamp config_stamp,
                               const SetupGttImageFunc& setup_gtt_image,
                               const GetImagePixelFormatFunc& get_pixel_format) {
  ZX_ASSERT(config_stamp != display::kInvalidDriverConfigStamp);

  // The values of the config stamps in `pending_eviction_config_stamps_` must
  // be strictly increasing.
  ZX_ASSERT(pending_eviction_config_stamps_.empty() ||
            pending_eviction_config_stamps_.back() < config_stamp);
  pending_eviction_config_stamps_.push_back(config_stamp);

  registers::pipe_arming_regs_t regs;
  registers::PipeRegs pipe_regs(pipe_id_);

  SetColorConversionOffsets(true, color_conversion.preoffsets());
  SetColorConversionOffsets(false, color_conversion.postoffsets());
  for (uint32_t i = 0; i < 3; i++) {
    for (uint32_t j = 0; j < 3; j++) {
      float val = color_conversion.coefficients()[i][j];
      ZX_DEBUG_ASSERT(std::isfinite(val));

      auto reg = pipe_regs.CscCoeff(i, j).ReadFrom(mmio_space_);
      reg.coefficient(i, j).set(float_to_intel_display_csc_coefficient(val));
      reg.WriteTo(mmio_space_);
    }
  }

  regs.csc_mode = pipe_regs.CscMode().ReadFrom(mmio_space_).reg_value();

  auto bottom_color = pipe_regs.PipeBottomColor().FromValue(0);

  const bool should_enable_color_space_conversion =
      color_conversion != display::ColorConversion::kIdentity;
  bottom_color.set_csc_enable(should_enable_color_space_conversion);
  bool has_color_layer = !layers.empty() && (layers[0].image_metadata().dimensions().width() == 0 ||
                                             layers[0].image_metadata().dimensions().height() == 0);
  if (has_color_layer) {
    const display::DriverLayer& layer = layers[0];
    const display::PixelFormat format = layer.fallback_color().format();

    if (format == display::PixelFormat::kB8G8R8A8) {
      bottom_color.set_r(encode_pipe_color_component(layer.fallback_color().bytes()[2]));
      bottom_color.set_g(encode_pipe_color_component(layer.fallback_color().bytes()[1]));
      bottom_color.set_b(encode_pipe_color_component(layer.fallback_color().bytes()[0]));
    } else if (format == display::PixelFormat::kR8G8B8A8) {
      bottom_color.set_r(encode_pipe_color_component(layer.fallback_color().bytes()[0]));
      bottom_color.set_g(encode_pipe_color_component(layer.fallback_color().bytes()[1]));
      bottom_color.set_b(encode_pipe_color_component(layer.fallback_color().bytes()[2]));
    } else {
      // CheckConfig() was supposed to reject this format.
      ZX_DEBUG_ASSERT(false);
    }
    config_stamp_with_color_layer_ = config_stamp;
  } else {
    config_stamp_with_color_layer_ = display::kInvalidDriverConfigStamp;
  }

  regs.pipe_bottom_color = bottom_color.reg_value();

  bool scaler_1_claimed = false;
  for (unsigned plane = 0; plane < 3; plane++) {
    const display::DriverLayer* primary = nullptr;
    for (size_t layer_index = 0; layer_index < layers.size(); ++layer_index) {
      const display::DriverLayer& layer = layers[layer_index];
      if (layer.image_id() != display::kInvalidDriverImageId &&
          layer_index == plane + has_color_layer) {
        primary = &layer;
        break;
      }
    }
    ConfigurePrimaryPlane(plane, primary, /*enable_csc=*/should_enable_color_space_conversion,
                          &scaler_1_claimed, &regs, config_stamp, setup_gtt_image,
                          get_pixel_format);
  }
  DisableCursorPlane(&regs, config_stamp);

  if (platform_ != registers::Platform::kTigerLake) {
    pipe_regs.CscMode().FromValue(regs.csc_mode).WriteTo(mmio_space_);
  }
  pipe_regs.PipeBottomColor().FromValue(regs.pipe_bottom_color).WriteTo(mmio_space_);
  pipe_regs.CursorBase().FromValue(regs.cur_base).WriteTo(mmio_space_);
  pipe_regs.CursorPos().FromValue(regs.cur_pos).WriteTo(mmio_space_);
  for (unsigned i = 0; i < registers::kImagePlaneCount; i++) {
    pipe_regs.PlaneSurface(i).FromValue(regs.plane_surf[i]).WriteTo(mmio_space_);
  }
  pipe_regs.PipeScalerRegs(/* num= */ 0)
      .PipeScalerWindowSize()
      .FromValue(regs.ps_win_sz[0])
      .WriteTo(mmio_space_);
  if (pipe_id_ != PipeId::PIPE_C) {
    pipe_regs.PipeScalerRegs(/* num= */ 1)
        .PipeScalerWindowSize()
        .FromValue(regs.ps_win_sz[1])
        .WriteTo(mmio_space_);
  }
}

void Pipe::ConfigurePrimaryPlane(uint32_t plane_num, const display::DriverLayer* primary,
                                 bool enable_csc, bool* scaler_1_claimed,
                                 registers::pipe_arming_regs_t* regs,
                                 display::DriverConfigStamp config_stamp,
                                 const SetupGttImageFunc& setup_gtt_image,
                                 const GetImagePixelFormatFunc& get_pixel_format) {
  registers::PipeRegs pipe_regs(pipe_id());

  auto plane_ctrl = pipe_regs.PlaneControl(plane_num).ReadFrom(mmio_space_);
  if (primary == nullptr) {
    plane_ctrl.set_plane_enabled(false).WriteTo(mmio_space_);
    regs->plane_surf[plane_num] = 0;
    return;
  }

  ZX_DEBUG_ASSERT(primary != nullptr);
  ZX_DEBUG_ASSERT_MSG(
      primary->image_source_transformation() == display::CoordinateTransformation::kIdentity,
      "Unsupported image source transformation: %" PRIu32,
      primary->image_source_transformation().ValueForLogging());

  const display::ImageMetadata& image_metadata = primary->image_metadata();
  ZX_DEBUG_ASSERT_MSG(image_metadata.tiling_type() == display::ImageTilingType::kLinear,
                      "Unsupported tiling type: %" PRIu32,
                      image_metadata.tiling_type().ValueForLogging());

  plane_ctrl.set_decompress_render_compressed_surfaces(false)
      .set_double_buffer_update_disabling_allowed(true);

  const GttRegion& region = setup_gtt_image(primary->image_metadata(), primary->image_id(),
                                            primary->image_source_transformation());
  const uint32_t base_address = static_cast<uint32_t>(region.base());
  const int32_t plane_width = primary->image_source().width();
  const int32_t plane_height = primary->image_source().height();
  const uint32_t stride = [&]() {
    uint64_t stride = region.bytes_per_row() / get_tile_byte_width(image_metadata.tiling_type());
    ZX_DEBUG_ASSERT_MSG(stride <= std::numeric_limits<uint32_t>::max(), "%lu overflows uint32_t",
                        stride);
    return static_cast<uint32_t>(stride);
  }();
  const int32_t y_offset = primary->image_source().y();
  const int32_t x_offset = primary->image_source().x();

  if (plane_width == primary->display_destination().width() &&
      plane_height == primary->display_destination().height()) {
    auto plane_pos = pipe_regs.PlanePosition(plane_num).FromValue(0);
    plane_pos.set_x_pos(primary->display_destination().x());
    plane_pos.set_y_pos(primary->display_destination().y());
    plane_pos.WriteTo(mmio_space_);

    // If there's a scaler pointed at this plane, immediately disable it
    // in case there's nothing else that will claim it this frame.
    if (scaled_planes_[pipe_id()][plane_num]) {
      int scaler_num = scaled_planes_[pipe_id()][plane_num] - 1;
      registers::PipeScalerRegs pipe_scaler_regs(pipe_id_, scaler_num);
      pipe_scaler_regs.PipeScalerControlSkylake()
          .ReadFrom(mmio_space_)
          .set_is_enabled(false)
          .WriteTo(mmio_space_);
      scaled_planes_[pipe_id()][plane_num] = 0;
      regs->ps_win_sz[scaler_num] = 0;
    }
  } else {
    pipe_regs.PlanePosition(plane_num).FromValue(0).WriteTo(mmio_space_);

    int scaler_num = *scaler_1_claimed ? 1 : 0;
    registers::PipeScalerRegs pipe_scaler_regs(pipe_id_, scaler_num);

    auto ps_ctrl = pipe_scaler_regs.PipeScalerControlSkylake().ReadFrom(mmio_space_);
    ps_ctrl.set_mode(registers::PipeScalerControlSkylake::ScalerMode::kDynamic);
    if (platform_ != registers::Platform::kTigerLake) {
      // The mode bits are different in Tiger Lake.
      if (primary->image_source().width() > 2048) {
        float max_dynamic_height =
            static_cast<float>(plane_height) *
            registers::PipeScalerControlSkylake::kDynamicMaxVerticalRatio2049;
        if (static_cast<int32_t>(max_dynamic_height) < primary->display_destination().height()) {
          // TODO(stevensd): This misses some cases where 7x5 can be used.
          ps_ctrl.set_mode(registers::PipeScalerControlSkylake::ScalerMode::kDynamic);
        }
      }
    }

    ps_ctrl.set_scaled_plane_index(plane_num + 1);
    ps_ctrl.set_is_enabled(1);
    ps_ctrl.WriteTo(mmio_space_);

    auto ps_win_pos = pipe_scaler_regs.PipeScalerWindowPosition().FromValue(0);
    ps_win_pos.set_x_position(static_cast<uint32_t>(primary->display_destination().x()));
    ps_win_pos.set_x_position(static_cast<uint32_t>(primary->display_destination().y()));
    ps_win_pos.WriteTo(mmio_space_);

    auto ps_win_size = pipe_scaler_regs.PipeScalerWindowSize().FromValue(0);
    ps_win_size.set_x_size(static_cast<uint32_t>(primary->display_destination().width()));
    ps_win_size.set_y_size(static_cast<uint32_t>(primary->display_destination().height()));
    regs->ps_win_sz[*scaler_1_claimed] = ps_win_size.reg_value();

    scaled_planes_[pipe_id()][plane_num] = (*scaler_1_claimed) + 1;
    *scaler_1_claimed = true;
  }

  auto plane_size = pipe_regs.PlaneSurfaceSize(plane_num).FromValue(0);
  plane_size.set_width_minus_1(plane_width - 1);
  plane_size.set_height_minus_1(plane_height - 1);
  plane_size.WriteTo(mmio_space_);

  auto plane_offset = pipe_regs.PlaneOffset(plane_num).FromValue(0);
  plane_offset.set_start_x(x_offset);
  plane_offset.set_start_y(y_offset);
  plane_offset.WriteTo(mmio_space_);

  auto stride_reg = pipe_regs.PlaneSurfaceStride(plane_num).FromValue(0);
  stride_reg.set_stride(stride);
  stride_reg.WriteTo(mmio_space_);

  registers::PlaneControlAlphaMode alpha_mode;
  if (primary->alpha_mode() == display::AlphaMode::kDisable) {
    alpha_mode = registers::PlaneControlAlphaMode::kAlphaIgnored;
  } else if (primary->alpha_mode() == display::AlphaMode::kPremultiplied) {
    alpha_mode = registers::PlaneControlAlphaMode::kAlphaPreMultiplied;
  } else {
    ZX_ASSERT(primary->alpha_mode() == display::AlphaMode::kHwMultiply);
    alpha_mode = registers::PlaneControlAlphaMode::kAlphaHardwareMultiply;
  }

  if (platform_ == registers::Platform::kTigerLake) {
    auto plane_color_ctl = pipe_regs.PlaneColorControlTigerLake(plane_num).ReadFrom(mmio_space_);
    plane_color_ctl.set_pipe_gamma_enabled_deprecated(false)
        .set_pipe_csc_enabled_deprecated(enable_csc)
        .set_plane_input_csc_enabled(false)
        .set_pre_csc_gamma_enabled(false)
        .set_post_csc_gamma_disabled(true)
        .set_alpha_mode(alpha_mode)
        .WriteTo(mmio_space_);
  }

  auto plane_key_mask = pipe_regs.PlaneKeyMask(plane_num).FromValue(0);
  if (primary->alpha_mode() != display::AlphaMode::kDisable &&
      !isnan(primary->alpha_coefficient())) {
    plane_key_mask.set_plane_alpha_enable(1);

    uint8_t alpha = static_cast<uint8_t>(round(primary->alpha_coefficient() * 255));

    auto plane_key_max = pipe_regs.PlaneKeyMax(plane_num).FromValue(0);
    plane_key_max.set_plane_alpha_value(alpha);
    plane_key_max.WriteTo(mmio_space_);
  }
  plane_key_mask.WriteTo(mmio_space_);

  plane_ctrl.set_plane_enabled(true);
  if (platform_ != registers::Platform::kTigerLake) {
    plane_ctrl.set_pipe_csc_enabled_kaby_lake(enable_csc).set_alpha_mode_kaby_lake(alpha_mode);
  }
  if (platform_ == registers::Platform::kTigerLake) {
    plane_ctrl.set_source_pixel_format_tiger_lake(
        registers::PlaneControl::ColorFormatTigerLake::kRgb8888);
  } else {
    plane_ctrl.set_source_pixel_format_kaby_lake(
        registers::PlaneControl::ColorFormatKabyLake::kRgb8888);
  }

  PixelFormatAndModifier pixel_format = get_pixel_format(primary->image_id());
  switch (pixel_format.pixel_format) {
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
      plane_ctrl.set_rgb_color_order(registers::PlaneControl::RgbColorOrder::kRgbx);
      break;
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      plane_ctrl.set_rgb_color_order(registers::PlaneControl::RgbColorOrder::kBgrx);
      break;
    default:
      // This should not happen. The sysmem-negotiated pixel format type can
      // only be RGBA or BGRA.
      // TODO(https://fxbug.dev/42076788): Support other formats.
      ZX_ASSERT_MSG(false,
                    "Sysmem-negotiated pixel format %u does not meet the constraints we placed",
                    static_cast<uint32_t>(pixel_format.pixel_format));
  }

  plane_ctrl.set_surface_tiling(registers::PlaneControl::SurfaceTiling::kLinear);
  plane_ctrl.set_rotation(registers::PlaneControl::Rotation::kIdentity);
  plane_ctrl.WriteTo(mmio_space_);

  auto plane_surface = pipe_regs.PlaneSurface(plane_num).ReadFrom(mmio_space_);
  plane_surface.set_surface_base_addr(base_address >> plane_surface.kRShiftCount);
  regs->plane_surf[plane_num] = plane_surface.reg_value();

  latest_config_stamp_with_image_[primary->image_id()] = config_stamp;
}

void Pipe::DisableCursorPlane(registers::pipe_arming_regs* regs,
                              display::DriverConfigStamp config_stamp) {
  registers::PipeRegs pipe_regs(pipe_id());

  auto cursor_ctrl = pipe_regs.CursorCtrl().ReadFrom(mmio_space_);
  cursor_ctrl.set_mode_select(cursor_ctrl.kDisabled).WriteTo(mmio_space_);
  regs->cur_base = regs->cur_pos = 0;
}

display::DriverConfigStamp Pipe::GetVsyncConfigStamp(
    const std::vector<display::DriverImageId>& image_ids) {
  display::DriverConfigStamp oldest_config_stamp = display::kInvalidDriverConfigStamp;

  if (config_stamp_with_color_layer_ != display::kInvalidDriverConfigStamp) {
    oldest_config_stamp = config_stamp_with_color_layer_;
  }
  for (const display::DriverImageId image_id : image_ids) {
    auto config_it = latest_config_stamp_with_image_.find(image_id);
    if (config_it == latest_config_stamp_with_image_.end()) {
      continue;
    }

    if (oldest_config_stamp != display::kInvalidDriverConfigStamp) {
      oldest_config_stamp = std::min(oldest_config_stamp, config_it->second);
    } else {
      oldest_config_stamp = config_it->second;
    }
  }

  if (oldest_config_stamp == display::kInvalidDriverConfigStamp) {
    // Display device may carry garbage contents in the registers, for example
    // if the driver restarted. In that case none of the images stored in the
    // device register will be recognized by the driver, so we just return a
    // null config stamp to ignore it.
    fdf::debug("{}: NO valid images for the display.", __func__);
    return display::kInvalidDriverConfigStamp;
  }
  if (pending_eviction_config_stamps_.empty()) {
    // Vsync signals could be sent to the driver before the first
    // SubmitConfiguration() is called. In that case the Vsync signal should be
    // just ignored by the driver, so we return a null config stamp.
    fdf::debug("{}: No config has been applied.", __func__);
    return display::kInvalidDriverConfigStamp;
  }
  if (pending_eviction_config_stamps_.front() > oldest_config_stamp) {
    fdf::error("{}: Device returns a config ({}) that is already evicted.", __func__,
               oldest_config_stamp.value());
    return display::kInvalidDriverConfigStamp;
  }

  // Evict all pending config stamps older than the current one from Vsync.
  while (!pending_eviction_config_stamps_.empty() &&
         pending_eviction_config_stamps_.front() < oldest_config_stamp) {
    pending_eviction_config_stamps_.pop_front();
  }

  ZX_DEBUG_ASSERT(!pending_eviction_config_stamps_.empty());
  return pending_eviction_config_stamps_.front();
}

void Pipe::SetColorConversionOffsets(bool preoffsets, std::span<const float, 3> vals) {
  registers::PipeRegs pipe_regs(pipe_id());

  for (uint32_t i = 0; i < 3; i++) {
    float offset = vals[i];
    auto offset_reg = pipe_regs.CscOffset(preoffsets, i).FromValue(0);
    if (offset < 0) {
      offset_reg.set_sign(1);
      offset *= -1;
    }
    offset_reg.set_magnitude(float_to_intel_display_csc_offset(offset));
    offset_reg.WriteTo(mmio_space_);
  }
}

}  // namespace intel_display
