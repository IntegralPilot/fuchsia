// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_PIPE_SCALER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_PIPE_SCALER_H_

#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <cmath>
#include <optional>
#include <vector>

#include <hwreg/bitfields.h>

#include "src/graphics/display/drivers/intel-display/hardware-common.h"

namespace registers {

// PS_CTRL (Pipe Scaler Control) for Skylake / Kaby Lake
//
// A Pipe Scaler can be used to scale the output of a display pipe or a display
// plane. It can scale up / down the resolution and / or scale up the
// downsampled chroma pixels or subplanes (for YUV images).
//
// Pipe A and B has two scalers, and Pipe C has one scaler. Each scaler can
// operate on the output of the entire pipe, or on the output of any of its
// universal planes (but not the cursor plane).
//
// This register is double buffered. See PipeScalerWindowSize for arming and
// dis-arming double-buffer updates.
//
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
//
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 2 Pages 647-650
// Skylake: IHD-OS-SKL-Vol 2c-05.16 Part 2, Pages 640-643
class PipeScalerControlSkylake : public hwreg::RegisterBase<PipeScalerControlSkylake, uint32_t> {
 public:
  static constexpr uint32_t kBaseAddr = 0x68180;

  // Enables the scaler.
  //
  // Before enabling the scaler, software should make sure that the scaler
  // configuration fulfills all constraints as specified in PS_CTRL register
  // documentation.
  DEF_BIT(31, is_enabled);

  enum class ScalerMode : uint32_t {
    // Supports filter size 7x5 for up to 2048 horizontal source sizes.
    // For larger size the filter size is automatically switched to 5x3.
    kDynamic = 0b00,

    // Supports filter size 7x5. Other scalers must be disabled in this mode.
    // Only supported by pipe A and pipe B.
    k7x5 = 0b01,

    // Required to feed NV12 data into a plane. Does chroma up-scaling and
    // size scaling.
    // Supports filter size 5x3.
    // This value is only documented in Kaby Lake but also mentioned in Skylake.
    kNv12 = 0b10,
  };
  DEF_ENUM_FIELD(ScalerMode, 29, 28, mode);

  // Selects the index of the plane where the output is scaled. An index of 0
  // indicates that the output of the whole pipe is scaled.
  //
  // The value of `scaled_plane_index` must not exceed the number of
  // planes (excluding cursor plane) supported by the pipe. On Kaby Lake and
  // Skylake, each pipe has 3 non-cursor planes.
  //
  // The caller must guarantee that the plane / pipe fulfills all the
  // constraints below:
  // - The input size of the pipe / plane must be at least 8 scanlines.
  // - IF (Interlaced fetch) mode must be disabled.
  // - Keying can only be enabled if the scaling factor is 1.
  // - The pixel format must not be 8-bit indexed or XR_BIAS.
  // - Floating point source pixel values must be within the [0.0, 1.0] range.
  DEF_FIELD(27, 25, scaled_plane_index);

  enum class FilterSelection : uint32_t {
    // Medium (Median?) filter.
    //
    // It guarantees that the image is unfiltered when the scaling factor is 1.
    //
    // The Programmer's Reference Manual (or The reference documentation) does
    // not have any further details on this filter.
    kMedium = 0b00,

    // Same as `kMedium`.
    //
    // On other display engine models, this value selects programmable
    // filtering.
    kMedium2 = 0b01,

    // Edge Enhance filter.
    //
    // The Programmer's Reference Manual (or The reference documentation) does
    // not have any further details on this filter.
    kEdgeEnhance = 0b10,

    // Bilinear filter. Scales the image by performing bilinear interpolation.
    kBilinear = 0b11,
  };
  DEF_ENUM_FIELD(FilterSelection, 24, 23, filter_selection);

  // Minimum supported horizontal and vertical sizes, in pixels.
  // TODO(https://fxbug.dev/42071707): Move all the pipe scaler limits to a separated
  // class.
  static constexpr uint32_t kMinSrcSizePx = 8;

  // Maximum supported horizontal size, in pixels.
  static constexpr uint32_t kMaxSrcWidthPx = 4096;

  // Number of scalers available in Pipe A and Pipe B.
  // TODO(https://fxbug.dev/42071707): This can be refactored into a static method
  // taking pipe and platform as its input.
  static constexpr uint32_t kPipeABScalersAvailable = 2;

  // Number of scalers available in Pipe C.
  static constexpr uint32_t kPipeCScalersAvailable = 1;

  // Maximum upscaling / downscaling ratio under 7x5 mode.
  // TODO(https://fxbug.dev/42071707): This can be refactored into a static method
  // taking scaling mode and size as its input.
  static constexpr float k7x5MaxRatio = 2.99f;

  // Maximum upscaling / downscaling ratio under dynamic mode, for image sizes
  // up to 2048 pixels.
  static constexpr float kDynamicMaxRatio = 2.99f;

  // Maximum vertical upscaling / downscaling ratio under dynamic mode, for
  // image sizes greater than 2048 pixels.
  static constexpr float kDynamicMaxVerticalRatio2049 = 1.99f;
};

// PS_CTRL (Pipe Scaler Control) for Tiger Lake
//
// A pipe Scaler can be used to scale the output of a display pipe or a display
// plane. It can scale up / down the resolution and / or scale up the
// downsampled chroma pixels or subplanes (for YUV images).
//
// Each pipe has two scalers and they can be assigned to any plane (except for
// color) or the display output.
//
// This register is double buffered. See PipeScalerWindowSize for arming and
// dis-arming double-buffer updates.
//
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 2, Pages 998-1003
class PipeScalerControlTigerLake
    : public hwreg::RegisterBase<PipeScalerControlTigerLake, uint32_t> {
 public:
  static constexpr uint32_t kBaseAddr = 0x68180;

  // Enables the pipe scaler.
  //
  // Before enabling the scaler, software should make sure that the scaler
  // configuration fulfills all the scaler constraints.
  DEF_BIT(31, is_enabled);

  // If true, the scaler performs YUV chroma upsampling while scaling the
  // output, which is also called "planar mode" in some documentation.
  //
  // This bit must be true if and only if the following conditions are met:
  // - The scaler must be bound to a universal plane instead of a pipe.
  // - The plane it binds to must be an SDR plane (Plane 4 or 5).
  // - The plane's active image must be of YUV 420 format.
  DEF_BIT(29, chroma_upscaling_enabled);

  // Enables the "Adaptive filtering" mode of the pipe scaler.
  //
  // When set to true, a multi-threshold adaptive filter will be used for
  // vertical and horizontal filtering. The `filter_selection` field will be
  // ignored, instead `adaptive_filter_selection` will be used to choose the
  // filter type.
  //
  // There are two sets of adaptive filtering thresholds for each pipe scaler.
  // Threshold values (in `PipeScalerAdaptiveFilterThresholds` register) and
  // threshold set selection (in `*_filter_set` field) must be programmed before
  // this bit being enabled.
  DEF_BIT(28, adaptive_filtering_enabled);

  // Selects the index of the plane where the output is scaled. An index of 0
  // indicates that the output of the whole pipe is scaled.
  //
  // The value of `scaled_plane_index` must not exceed the number of
  // planes supported by the pipe. On Tiger Lake, each pipe has 7 planes.
  //
  // The caller must guarantee that the plane / pipe fulfills all the
  // constraints below:
  // - The input size of the pipe / plane must be at least 8 scanlines.
  // - IF (Interlaced fetch) mode must be disabled.
  // - Keying can only be enabled if the scaling factor is 1.
  // - The pixel format must not be 8-bit indexed or XR_BIAS.
  // - Floating point source pixel values must be within the [0.0, 1.0] range.
  // - For HDR planes (Plane 1 - 3), the `plane_scaling_enabled` field of the
  //   `PLANE_CUS_CTL` register must be enabled.
  DEF_FIELD(27, 25, scaled_plane_index);

  enum class NormalFilterSelection : uint32_t {
    // Medium (Median?) filter.
    //
    // It guarantees that the image is unfiltered when the scaling factor is 1.
    //
    // The Programmer's Reference Manual (or The reference documentation) does
    // not have any further details on this filter.
    kMedium = 0b00,

    // Programmed filter.
    //
    // Drivers can scale images using different scaling filters with hardware
    // acceleration under this mode . A typical usage of programmed filters is
    // to implement nearest-neighbor scaling.
    //
    // The filter has 7 taps and it can have different coefficients on its 17
    // phases; so each coefficient set has totally 119 coefficients, and each
    // scaler can store 2 sets of programmed coefficients. All the coefficients
    // can be set using PS_COEF_INDEX and PS_COEF_DATA registers.
    //
    // Horizontal filters and vertical filters can be configured to use
    // their individual coefficient sets. For YUV planar images, Y plane filters
    // and UV plane filters can also be configured to use their own coefficient
    // sets.
    //
    // Tiger Lake: Section "Scaler Programmed Coefficients",
    // IHD-OS-TGL-Vol 12-1.22-Rev 2.0 Part 2, Pages 255-256.
    kProgrammed = 0b01,

    // Edge Enhance filter.
    //
    // The Programmer's Reference Manual (or The reference documentation) does
    // not have any further details on this filter.
    kEdgeEnhance = 0b10,

    // Bilinear filter. Scales the image by performing bilinear interpolation.
    kBilinear = 0b11,
  };

  // Selects the filter to use for the non-adaptive filter.
  //
  // This field only works on non-adaptive mode. If `adaptive_filtering_enabled`
  // is true, this field will be ignored and `adaptive_filter_selection` will
  // be used instead.
  DEF_ENUM_FIELD(NormalFilterSelection, 24, 23, normal_filter_selection);

  enum class AdaptiveFilterSelection : uint32_t {
    // Medium (Median?) filter. Same as `kMedium` in NormalFilterSelection.
    kMedium = 0b00,

    // Edge enhanced filter. Same as `kEdgeEnhance` in NormalFilterSelection.
    kEdgeEnhance = 0b10,
  };

  // Selects the filter to use for the adaptive filter.
  //
  // This field only works on adaptive mode. If `adaptive_filtering_enabled`
  // is false, this field will be ignored and `normal_filter_selection` will
  // be used instead.
  DEF_ENUM_FIELD(AdaptiveFilterSelection, 22, 22, adaptive_filter_selection);

  // This enum class specifies the scaler interface on the display pipeline that
  // the pipe scaler binds to.
  //
  // Tiger Lake: Section "Pipes", IHD-OS-TGL-Vol 12-1.22-Rev 2.0 Part 2, Pages 117
  enum class PipeScalerLocation : uint32_t {
    // Scale the pipe output after the second (output) color space conversion
    // ("Output CSC" in the display pipeline diagram), which occurs after gamma
    // look-up table process ("Post-CSC Gamma" in the diagram) and other gamma
    // enhancements.
    //
    // The scaling occurs after de-gamma and gamma, thus the luminance value is
    // still gamma-encoded, so the scaler operates in a non-linear color space.
    kAfterOutputColorSpaceConversion = 0b0,

    // Scale the pipe output after de-gamma look-up table (Pre-CSC Gamma) and
    // the first color space conversion (the "CSC" block in the display pipeline
    // diagram).
    //
    // The scaling occurs after de-gamma where the luminance value gets
    // linearized, so the scaler operates in a linear color space.
    kAfterColorSpaceConversion = 0b1,
  };

  // Specifies the scaler interface on the display pipeline that the pipe
  // scaler binds to. See `PipeScalerLocation` for details.
  //
  // This field is effective only when the scaler is bound to the pipe, i.e.
  // `scaler_binding_selection` field is 0.
  DEF_ENUM_FIELD(PipeScalerLocation, 21, 21, pipe_scaler_location);

  DEF_RSVDZ_BIT(18);
  DEF_RSVDZ_BIT(16);
  DEF_RSVDZ_BIT(14);
  DEF_RSVDZ_FIELD(11, 10);

  // If true, double-buffer updates can be disabled for this plane.
  //
  // This field applies when the DOUBLE_BUFFER_CTL register is used to disable
  // the double-buffering of all the resources that allow disabling.
  DEF_BIT(9, double_buffer_update_disabling_allowed);

  // Selects the Y plane surface matching the UV plane selected in
  // `scaled_plane_index`.
  //
  // This field is effective only if planar mode is enabled.
  //
  // Only the following values are valid for this field:
  // - 6 (0b110): indicating Plane 6 is used as input Y plane;
  // - 7 (0b111): indicating Plane 7 is used as input Y plane.
  //
  // `y_plane_binding()` and `set_y_plane_binding()` helper methods can be used
  // to get / set the value of this field.
  DEF_FIELD(7, 5, y_plane_binding_raw);

  // Helper method to get the value of this field with validity check.
  uint32_t y_plane_binding() const {
    switch (y_plane_binding_raw()) {
      case 6:
      case 7:
        // Valid values.
        break;
      default:
        fdf::warn("Scaler bound to an invalid Y plane ({})", y_plane_binding_raw());
        break;
    }
    return y_plane_binding_raw();
  }

  // Helper method to set the value of this field with validity check.
  PipeScalerControlTigerLake& set_y_plane_binding(uint32_t plane_id) {
    ZX_ASSERT_MSG(plane_id == 6 || plane_id == 7,
                  "Cannot bind non-Y plane (%u) for YUV scaling; only Plane 6 and 7 are supported",
                  plane_id);
    return set_y_plane_binding_raw(plane_id);
  }

  // Selects the filter coefficient / threshold set used by the *vertical*
  // filter for the Y semi-plane bound to the scaler in YUV planar mode.
  //
  // It's used only in the following conditions:
  // - When `chrome_upsampling_enabled` is true and `adaptive_filter_enabled`
  //   is true, it indicates the adaptive filter threshold set index for Y
  //   plane scaling filter.
  // - When `chrome_upsampling_enabled` is true, adaptive filtering is
  //   disabled and `normal_filter_select` is `kProgrammed`, it indicates the
  //   programmed filter coefficient set index for Y plane scaling filter.
  // Otherwise, this field will be ignored.
  DEF_BIT(4, y_plane_vertical_filter_set);

  // Selects the filter coefficient / threshold set used by the *horizontal*
  // filter for the Y semi-plane bound to the scaler in YUV planar mode.
  //
  // The semantics of this field is similar to
  // `y_plane_vertical_filter_set_selection` but it applies to the horizontal
  // filter instead.
  DEF_BIT(3, y_plane_horizontal_filter_set);

  // Selects the filter coefficient / threshold set used by the *vertical*
  // filter for the plane (or UV semi-plane, if `chroma_upscaling_enabled`
  // is true) or pipe bound to the scaler.
  //
  // It's used in the following conditions:
  // - If `adaptive_filter_enabled` is true, this field indicates the index of
  //   the adaptive filter threshold set.
  // - If `adaptive_filter_enabled` is false and `normal_filter_select` is
  //   `kProgrammed`, this field indicates the selected filter coefficient set.
  // Otherwise, this field is ignored.
  DEF_BIT(2, vertical_filter_set);

  // Selects the filter coefficient / threshold set used by the *horizontal*
  // filter for the plane (or UV semi-plane, if `chroma_upscaling_enabled` is
  // true) or pipe bound to the scaler.
  //
  // The semantics of this field is similar to
  // `vertical_filter_set_selection` but it applies to the horizontal
  // filter instead.
  DEF_BIT(1, horizontal_filter_set);

  DEF_RSVDZ_BIT(0);
};

// PS_ADAPTIVE_CTRL (Pipe Scaler Adaptive Filter Thresholds)
//
// Each scaler has 2 sets of adaptive filter thresholds that scaler can select
// for filters on each dimension and each semi-plane when adaptive filter is
// enabled for the scaler.
//
// This register is double buffered. See PipeScalerWindowSize for arming and
// dis-arming double-buffer updates.
//
// This register is not documented in Skylake and Kaby Lake.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 2, Pages 989-991.
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2, Pages 1026-1028.
class PipeScalerAdaptiveFilterThresholds
    : public hwreg::RegisterBase<PipeScalerAdaptiveFilterThresholds, uint32_t> {
 public:
  static constexpr uint32_t kBaseAddr = 0x681A8;

  DEF_RSVDZ_FIELD(31, 24);

  // The third threshold value in adaptive filtering.
  // The recommended value of this field is 0x3c.
  DEF_FIELD(23, 16, threshold3);

  // The second threshold value in adaptive filtering.
  // The recommended value of this field is 0x2d.
  DEF_FIELD(15, 8, threshold2);

  // The first threshold value in adaptive filtering.
  // The recommended value of this field is 0x1e.
  DEF_FIELD(7, 0, threshold1);
};

// PS_WIN_POS (Pipe Scaler Window Position)
//
// The scaler outputs the scaled contents to the scaler window, a rectangular
// region specified by `PipeScalerWindowPosition` and `PipeScalerWindowSize`
// on the output surface.
//
// This register specifies the location of the top-left corner. "x" and "y"
// are relative to the display device, thus rotation / flipping of planes and
// pipe doesn't affect the order of x and y.
//
// This register and `PipeScalerWindowSize` must be set so that the whole
// scaler window is within the output surface:
//
// For plane scaling, it outputs to the pipe source, so
//  x_position + x_size <= PipeSourceSize.horizontal_source_size
//  y_position + y_size <= PipeSourceSize.vertical_source_size
// must be satisfied.
//
// For pipe scaling, it outputs to the transcoder, so
//  x_position + x_size <= PipeActiveSize.x_size
//  y_position + y_size <= PipeActiveSize.y_size
//
// If the pipe is not joined to other pipes,
//  PipeActiveSize.x_size = TranscoderHorizontalTotal.active_size
//  PipeActiveSize.y_size = TranscoderVerticalTotal.active_size
//
// TODO(liyl): The documentation is unclear how the pipe active size is defined
// in the case of joined pipes (whether a division is needed).
// otherwise,
//  PipeActiveSize.x_size = TranscoderHorizontalTotal.active_size / #(joined pipes)
//    + PipeSeamExcess.left_excess_amount + PipeSeamExcess.right_excess_amount
//  PipeActiveSize.y_size = TranscoderVerticalTotal.active_size
//  Note that the excess amount must be taken into account when merging
//  pipes' output to a single transcoder.
//
// This register is double buffered. See PipeScalerWindowSize for arming and
// dis-arming double-buffer updates.
//
// This register's reserved fields are all MBZ (must be zero). So, this register
// can be safely written without reading it first.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 2, Pages 1021-1022
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 2, Pages 666-667
class PipeScalerWindowPosition : public hwreg::RegisterBase<PipeScalerWindowPosition, uint32_t> {
 public:
  static constexpr uint32_t kBaseAddr = 0x68170;

  DEF_RSVDZ_FIELD(31, 29);

  // Horizontal (x) position of the left of the rectangle.
  // Must be even for YUV planes / pipes.
  DEF_FIELD(28, 16, x_position);

  DEF_RSVDZ_FIELD(15, 13);

  // Vertical (y) position of the top of the rectangle.
  // Must be even for YUV planes / pipes, or for interlaced output.
  DEF_FIELD(12, 0, y_position);
};

// Pipe Scaler Programmable Coefficient Data Format (SCALER_COEFFICIENT_FORMAT)
//
// The pipe scaler coefficients use SCALER_COEFFICIENT_FORMAT to store
// coefficient values. This is a signed floating point format with 2 exponent
// bits and 9 mantissa bits, and uses complement representation same as the
// scaled number for negative values.
//
// The coefficient value =
//  (-1) ^ (is_negative) * (mantissa / 2 ^ 9) * (1 - exponent).
//
// Tiger Lake: IHD-OS-TGL-Vol 2d-12.21, Page 868.
// Lakefield: IHD-OS-LKF-Vol 2d-5.21, Page 899.
class PipeScalerCoefficientFormat
    : public hwreg::RegisterBase<PipeScalerCoefficientFormat, uint32_t> {
 public:
  // Sign bit.
  DEF_BIT(15, is_negative);

  // Unsigned 2-bit exponent field.
  // The exponent part is 2 ^ (1 - exponent).
  DEF_FIELD(13, 12, exponent);

  // Mantissa.
  // The mantissa part is 0.bbbbbbbbb (uint32(mantissa) / 2 ^ 9).
  DEF_FIELD(11, 3, mantissa);

  int x2048() const {
    return (static_cast<int32_t>(mantissa()) << (3 - exponent())) *
           (1 - 2 * static_cast<int32_t>(is_negative()));
  }
};

// PS_COEF_DATA (Pipe Scaler Programmable Coefficient Data)
//
// Each scaler has 2 sets of programmable filter coefficients, each of which
// containing 17 (phases) x 7 (taps) = 119 coefficient values. Each coefficient
// value has 16 bits in storage and is of type PipeScalerCoefficientFormat.
//
// To read / write the value of the coefficients, the PS_COEF_INDEX register of
// the scaler coefficient set must be set (or auto-incremented) first, and then
// a pair of 16-bit coefficient values can be read from / written to this
// register.
//
// The coefficients must be written before its value being used or the
// programmable filter being used. Otherwise the filter behavior is undefined.
//
// The coefficient data arrays are double buffered. Writing to the index / data
// register will update the buffered value, and writing to
// `PipeScalerWindowSize` will trigger the double buffering arm so that the
// buffered values will be applied at the start of the next vertical blank.
//
// This register's reserved fields are all MBZ (must be zero). So, this register
// can be safely written without reading it first.
//
// Helper class `PipeScalerCoefficients` can be used to read from / write to
// the whole coefficient array.
//
// This register is available in Tiger Lake only.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 2, Pages 992-994
class PipeScalerCoefficientData : public hwreg::RegisterBase<PipeScalerCoefficientData, uint32_t> {
 public:
  static constexpr uint32_t kBaseAddr = 0x6819C;

  // Stores the second coefficient stored in this register.
  //
  // The value is stored in PipeScalerCoefficientFormat format.
  // Helper methods GetCoefficient2() and SetCoefficient2() can be used to get
  // set the value of this field.
  DEF_FIELD(31, 16, coefficient2_raw);

  // Helper method to get value of the second coefficient.
  PipeScalerCoefficientFormat coefficient2() const {
    return PipeScalerCoefficientFormat().set_reg_value(coefficient2_raw());
  }

  // Helper method to set value of the second coefficient.
  PipeScalerCoefficientData& set_coefficient2(const PipeScalerCoefficientFormat& coefficient) {
    return set_coefficient2_raw(coefficient.reg_value());
  }

  // Stores the first coefficient stored in this register.
  //
  // The value is stored in PipeScalerCoefficientFormat format.
  // Helper methods GetCoefficient1() and SetCoefficient1() can be used to get
  // set the value of this field.
  DEF_FIELD(15, 0, coefficient1_raw);

  // Helper method to get value of the first coefficient.
  PipeScalerCoefficientFormat coefficient1() const {
    return PipeScalerCoefficientFormat().set_reg_value(coefficient1_raw());
  }

  // Helper method to set value of the first coefficient.
  PipeScalerCoefficientData& set_coefficient1(const PipeScalerCoefficientFormat& coefficient) {
    return set_coefficient1_raw(coefficient.reg_value());
  }
};

// PS_COEF_INDEX (Pipe Scaler Programmable Coefficient Index)
//
// See PipeScalerCoefficientData class for the representation of scaler
// coefficients.
//
// This register is used together with PipeScalerCoefficientData to read / write
// the value of the coefficients. Before writing to the data register, software
// must set the initial index value (and optionally enable auto-increment)
// first.
//
// The coefficient data arrays are double buffered. Writing to the index / data
// register will update the buffered value, and writing to
// `PipeScalerWindowSize` will trigger the double buffering arm so that the
// buffered values will be applied at the start of the next vertical blank.
//
// This register's reserved fields are all MBZ (must be zero). So, this register
// can be safely written without reading it first.
//
// Helper class `PipeScalerCoefficients` can be used to read from / write to
// the whole coefficient array.
//
// This register is available in Tiger Lake only.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 2, Pages 995-997
class PipeScalerCoefficientIndex
    : public hwreg::RegisterBase<PipeScalerCoefficientIndex, uint32_t> {
 public:
  static constexpr uint32_t kBaseAddr = 0x68198;

  DEF_RSVDZ_FIELD(31, 11);

  // Enables auto increment feature of the index register. If it is enabled,
  // the index value will automatically increment by 1 every time
  // PipeScalerCoefficientData register is read or written.
  DEF_BIT(10, index_auto_increment_enabled);

  DEF_RSVDZ_FIELD(9, 6);

  // The index value used to read / write data registers.
  // The index value must be between [0, 59] (inclusive).
  DEF_FIELD(5, 0, index_raw);
  PipeScalerCoefficientIndex& set_index(int index) {
    ZX_ASSERT_MSG(index >= 0 && index <= 59, "Invalid index (%d); index must be in range [0, 59]",
                  index);
    return set_index_raw(index);
  }
};

// Pipe Scaler Coefficients Helper
//
// A helper class that software can use to write PipeScalerCoefficientFormat
// values to the data register, or read the data register contents back as a
// vector.
//
// This helper class only updates the values to the buffered registers, and
// software must trigger the double buffer arm register (PipeScalerWindowSize)
// by themselves to apply the value update.
//
// TODO(https://fxbug.dev/42071700): Move this alongside with scaler management logic,
// as this is a layer above the register definitions.
class PipeScalerCoefficients {
 public:
  PipeScalerCoefficients(hwreg::RegisterAddr<PipeScalerCoefficientIndex> index_address,
                         hwreg::RegisterAddr<PipeScalerCoefficientData> data_address)
      : index_address_(index_address), data_address_(data_address) {}

  // Write values of `coefficients` to the coefficient array using index / data
  // registers.
  // The size of `coefficients` must be 17 * 7 = 119.
  template <typename T>
  void WriteToRegister(std::span<const PipeScalerCoefficientFormat> coefficients, T* reg_io) {
    ZX_ASSERT(coefficients.size() == kNumCoefficients);
    auto index_reg = index_address_.FromValue(0);
    index_reg.set_reg_value(0).set_index(0).set_index_auto_increment_enabled(true).WriteTo(reg_io);
    for (size_t i = 0; i < kNumCoefficients; i += 2) {
      auto data_reg = data_address_.FromValue(0);
      data_reg.set_coefficient1(coefficients[i]);
      if (i + 1 < kNumCoefficients) {
        data_reg.set_coefficient2(coefficients[i + 1]);
      }
      data_reg.WriteTo(reg_io);
    }
  }

  // Read values of the coefficient array using index / data registers.
  // Returns a coefficient array of size 17 * 7 = 119.
  template <typename T>
  std::vector<PipeScalerCoefficientFormat> ReadFromRegister(T* reg_io) {
    std::vector<PipeScalerCoefficientFormat> coefficients;
    coefficients.reserve(kNumCoefficients);

    auto index_reg = index_address_.FromValue(0);
    index_reg.set_reg_value(0).set_index(0).set_index_auto_increment_enabled(true).WriteTo(reg_io);

    for (size_t i = kNumCoefficients % 2; i < kNumCoefficients; i += 2) {
      auto data_reg = data_address_.ReadFrom(reg_io);
      coefficients.push_back(
          PipeScalerCoefficientFormat().set_reg_value(data_reg.coefficient1_raw()));
      coefficients.push_back(
          PipeScalerCoefficientFormat().set_reg_value(data_reg.coefficient2_raw()));
    }
    if (kNumCoefficients % 2 == 1) {
      auto data_reg = data_address_.ReadFrom(reg_io);
      coefficients.push_back(
          PipeScalerCoefficientFormat().set_reg_value(data_reg.coefficient1_raw()));
    }

    ZX_DEBUG_ASSERT(coefficients.size() == kNumCoefficients);
    return coefficients;
  }

 private:
  static constexpr int kNumCoefficients = 17 * 7;

  hwreg::RegisterAddr<PipeScalerCoefficientIndex> index_address_;
  hwreg::RegisterAddr<PipeScalerCoefficientData> data_address_;
};

// PS_HPHASE (Pipe Scaler Horizontal Phase)
//
// Programs the scaler's horizontal filtering initial phase.
//
// The phase sample position must be adjusted for YUV 420 to YUV 444 upscaling
// so that the chroma sample position lands in the right spot. Note that this
// should not be set for HDR planes where Pipe Scaler doesn't upscale the
// chroma.
//
// The initial phase within the -0.5 to 1.5 range is supported.
//
// The phases are programmed as follows in the following YUV upscaling
// scenarios:
//
// YUV 420 Chroma Siting
// Top Left:               Horizontal Phase = 0.25
// Bottom Right  (MPEG-1): Horizontal Phase = -0.25
// Bottom Center (MPEG-2): Horizontal Phase = 0
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 2, Pages 1006-1007
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 2, Pages 653-654
class PipeScalerHorizontalInitialPhase
    : public hwreg::RegisterBase<PipeScalerHorizontalInitialPhase, uint32_t> {
 public:
  static constexpr uint32_t kBaseAddr = 0x68194;

  // This specifies the integer part of the initial phase of the Y semiplane
  // filter.
  //
  // y_initial_phase_int = int(initial_phase) if initial_phase >= 0
  //                     = 0                  if initial_phase < 0
  //
  // A helper method `SetYInitialPhase()` should be used to set the value of
  // this field.
  //
  // This field is ignored for non-YUV420 pixel formats.
  DEF_FIELD(31, 30, y_initial_phase_int);

  // This specifies the most significant 13 bits of the fractional part of the
  // initial phase of the Y semiplane filter.
  //
  // y_initial_phase_fraction
  //  = int(initial_phase - int(initial_phase)) * 2 ^ 13                 if initial_phase >= 0
  //  = int((1 - abs(initial_phase)) * 2 ^ 13) if initial_phase < 0
  //
  // A helper method `SetYInitialPhase()` should be used to set the value of
  // this field.
  //
  // This field is ignored for non-YUV420 pixel formats.
  DEF_FIELD(29, 17, y_initial_phase_fraction);

  // Specifies whether the initial trip of the Y semiplane horizontal filtering
  // may occur.
  //
  // y_initial_phase_trip_enabled = true    if initial_phase >= 0
  //                              = false   if initial_phase < 0
  //
  // A helper method `SetYInitialPhase()` should be used to set the value of
  // this field.
  //
  // This field is ignored for non-YUV420 pixel formats.
  DEF_BIT(16, y_initial_phase_trip_enabled);

  // A helper method that sets the value of initial phases of the Y semiplane.
  //
  // `initial_phase` must be a finite float number between [-0.5, 1.5].
  //
  // Only the most significant 13 bits of the fractional part are used to set
  // the bitfields, and the software should be aware of the possible precision
  // loss.
  PipeScalerHorizontalInitialPhase& SetYInitialPhase(float initial_phase) {
    ZX_ASSERT(std::isfinite(initial_phase));
    ZX_ASSERT(initial_phase <= 1.5);
    ZX_ASSERT(initial_phase >= -0.5);
    if (initial_phase > 0) {
      int integer = static_cast<int>(floor(initial_phase));
      int fraction = static_cast<int>((initial_phase - static_cast<float>(integer)) * (1 << 13));
      return set_y_initial_phase_trip_enabled(true)
          .set_y_initial_phase_int(integer)
          .set_y_initial_phase_fraction(fraction);
    }
    if (initial_phase == 0) {
      return set_y_initial_phase_trip_enabled(true)
          .set_y_initial_phase_int(0)
          .set_y_initial_phase_fraction(0);
    }
    ZX_DEBUG_ASSERT(initial_phase < 0);
    uint32_t fraction = static_cast<uint32_t>((1 - fabs(initial_phase)) * (1 << 13));
    return set_y_initial_phase_trip_enabled(false)
        .set_y_initial_phase_int(0)
        .set_y_initial_phase_fraction(fraction);
  }

  // This specifies the integer part of the initial phase of the UV semiplane
  // (for YUV420) or RGB filter.
  //
  // uv_initial_phase_int = int(initial_phase) if initial_phase >= 0
  //                      = 0                  if initial_phase < 0
  DEF_FIELD(15, 14, uv_initial_phase_int);

  // This specifies the most significant 13 bits of the fractional part of the
  // initial phase of the UV semiplane (for YUV420) or RGB filter.
  //
  // uv_initial_phase_fraction
  //  = int(initial_phase - int(initial_phase)) * 2 ^ 13  if initial_phase >= 0
  //  = int(1 - abs(initial_phase)) * 2 ^ 13              if initial_phase < 0
  DEF_FIELD(13, 1, uv_initial_phase_fraction);

  // Specifies whether the initial trip of the UV semiplane (for YUV420) or
  // RGB horizontal filtering may occur.
  //
  // y_initial_phase_trip_enabled = true    if initial_phase >= 0
  //                              = false   if initial_phase < 0
  DEF_BIT(0, uv_initial_phase_trip_enabled);

  // A helper method that sets the value of initial phases of the UV semiplane
  // (for YUV420) or RGB.
  //
  // `initial_phase` must be a finite float number between [-0.5, 1.5].
  //
  // Only the most significant 13 bits of the fractional part are used to set
  // the bitfields, and the software should be aware of the possible precision
  // loss.
  PipeScalerHorizontalInitialPhase& SetUvInitialPhase(float initial_phase) {
    ZX_ASSERT(std::isfinite(initial_phase));
    ZX_ASSERT(initial_phase <= 1.5);
    ZX_ASSERT(initial_phase >= -0.5);
    if (initial_phase > 0) {
      int integer = static_cast<int>(floor(initial_phase));
      int fraction = static_cast<int>((initial_phase - static_cast<float>(integer)) * (1 << 13));
      return set_uv_initial_phase_trip_enabled(true)
          .set_uv_initial_phase_int(integer)
          .set_uv_initial_phase_fraction(fraction);
    }
    if (initial_phase == 0) {
      return set_uv_initial_phase_trip_enabled(true)
          .set_uv_initial_phase_int(0)
          .set_uv_initial_phase_fraction(0);
    }
    ZX_DEBUG_ASSERT(initial_phase < 0);
    uint32_t fraction = static_cast<uint32_t>((1 - fabs(initial_phase)) * (1 << 13));
    return set_uv_initial_phase_trip_enabled(false)
        .set_uv_initial_phase_int(0)
        .set_uv_initial_phase_fraction(fraction);
  }
};

// PS_VPHASE (Pipe Scaler Vertical Phase)
//
// Programs the scaler's vertical filtering initial phase.
//
// The phase sample position must be adjusted for YUV 420 to YUV 444 upscaling
// so that the chroma sample position lands in the right spot. Note that this
// should not be set for HDR planes where Pipe Scaler doesn't upscale the
// chroma.
//
// The initial phase within the -0.5 to 1.5 range is supported.
//
// The register only works in Progressive Fetch - Progressive Display (PF-PD)
// mode and will be ignored in Progressive Fetch - Interlaced Display (PF-ID)
// mode.
//
// The phases are programmed as follows in the following YUV upscaling
// scenarios:
//
// YUV 420 Chroma Siting
// Top Left:               Vertical Phase = 0.25
// Bottom Right  (MPEG-1): Vertical Phase = -0.25
// Bottom Center (MPEG-2): Vertical Phase = 0
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 2, Pages 1016-1018
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 2, Pages 659-661
class PipeScalerVerticalInitialPhase
    : public hwreg::RegisterBase<PipeScalerVerticalInitialPhase, uint32_t> {
 public:
  static constexpr uint32_t kBaseAddr = 0x68188;

  // This specifies the integer part of the initial phase of the Y semiplane
  // filter.
  //
  // y_initial_phase_int = int(initial_phase) if initial_phase >= 0
  //                     = 0                  if initial_phase < 0
  //
  // A helper method `SetYInitialPhase()` should be used to set the value of
  // this field.
  //
  // This field is ignored for non-YUV420 pixel formats.
  DEF_FIELD(31, 30, y_initial_phase_int);

  // This specifies the most significant 13 bits of the fractional part of the
  // initial phase of the Y semiplane filter.
  //
  // y_initial_phase_fraction
  //  = int(initial_phase - int(initial_phase)) * 2 ^ 13  if initial_phase >= 0
  //  = int(1 - abs(initial_phase)) * 2 ^ 13              if initial_phase < 0
  //
  // A helper method `SetYInitialPhase()` should be used to set the value of
  // this field.
  //
  // This field is ignored for non-YUV420 pixel formats.
  DEF_FIELD(29, 17, y_initial_phase_fraction);

  // Specifies whether the initial trip of the Y semiplane vertical filtering
  // may occur.
  //
  // y_initial_phase_trip_enabled = true    if initial_phase >= 0
  //                              = false   if initial_phase < 0
  //
  // A helper method `SetYInitialPhase()` should be used to set the value of
  // this field.
  //
  // This field is ignored for non-YUV420 pixel formats.
  DEF_BIT(16, y_initial_phase_trip_enabled);

  // A helper method that sets the value of initial phases of the Y semiplane.
  //
  // `initial_phase` must be a finite float number between [-0.5, 1.5].
  //
  // Only the most significant 13 bits of the fractional part are used to set
  // the bitfields, and the software should be aware of the possible precision
  // loss.
  PipeScalerVerticalInitialPhase& SetYInitialPhase(float initial_phase) {
    ZX_ASSERT(std::isfinite(initial_phase));
    ZX_ASSERT(initial_phase <= 1.5);
    ZX_ASSERT(initial_phase >= -0.5);
    if (initial_phase > 0) {
      int integer = static_cast<int>(floor(initial_phase));
      int fraction = static_cast<int>((initial_phase - static_cast<float>(integer)) * (1 << 13));
      return set_y_initial_phase_trip_enabled(true)
          .set_y_initial_phase_int(integer)
          .set_y_initial_phase_fraction(fraction);
    }
    if (initial_phase == 0) {
      return set_y_initial_phase_trip_enabled(true)
          .set_y_initial_phase_int(0)
          .set_y_initial_phase_fraction(0);
    }
    ZX_DEBUG_ASSERT(initial_phase < 0);
    uint32_t fraction = static_cast<uint32_t>((1 - fabs(initial_phase)) * (1 << 13));
    return set_y_initial_phase_trip_enabled(false)
        .set_y_initial_phase_int(0)
        .set_y_initial_phase_fraction(fraction);
  }

  // This specifies the integer part of the initial phase of the UV semiplane
  // (for YUV420) or RGB filter.
  //
  // uv_initial_phase_int = int(initial_phase) if initial_phase >= 0
  //                      = 0                  if initial_phase < 0
  DEF_FIELD(15, 14, uv_initial_phase_int);

  // This specifies the most significant 13 bits of the fractional part of the
  // initial phase of the UV semiplane (for YUV420) or RGB filter.
  //
  // uv_initial_phase_fraction
  //  = int(initial_phase - int(initial_phase)) * 2 ^ 13  if initial_phase >= 0
  //  = int(1 - fabs(initial_phase)) * 2 ^ 13             if initial_phase < 0
  DEF_FIELD(13, 1, uv_initial_phase_fraction);

  // Specifies whether the initial trip of the UV semiplane (for YUV420) or
  // RGB vertical filter may occur.
  //
  // y_initial_phase_trip_enabled = true    if initial_phase >= 0
  //                              = false   if initial_phase < 0
  DEF_BIT(0, uv_initial_phase_trip_enabled);

  // A helper method that sets the value of initial phases of the UV semiplane
  // (for YUV420) or RGB.
  //
  // `initial_phase` must be a finite float number between [-0.5, 1.5].
  //
  // Only the most significant 13 bits of the fractional part are used to set
  // the bitfields, and the software should be aware of the possible precision
  // loss.
  PipeScalerVerticalInitialPhase& SetUvInitialPhase(float initial_phase) {
    ZX_ASSERT(std::isfinite(initial_phase));
    ZX_ASSERT(initial_phase <= 1.5);
    ZX_ASSERT(initial_phase >= -0.5);
    if (initial_phase > 0) {
      int integer = static_cast<int>(floor(initial_phase));
      int fraction = static_cast<int>((initial_phase - static_cast<float>(integer)) * (1 << 13));
      return set_uv_initial_phase_trip_enabled(true)
          .set_uv_initial_phase_int(integer)
          .set_uv_initial_phase_fraction(fraction);
    }
    if (initial_phase == 0) {
      return set_uv_initial_phase_trip_enabled(true)
          .set_uv_initial_phase_int(0)
          .set_uv_initial_phase_fraction(0);
    }
    ZX_DEBUG_ASSERT(initial_phase < 0);
    uint32_t fraction = static_cast<uint32_t>((1 - fabs(initial_phase)) * (1 << 13));
    return set_uv_initial_phase_trip_enabled(false)
        .set_uv_initial_phase_int(0)
        .set_uv_initial_phase_fraction(fraction);
  }
};

// PS_PWR_GATE (Pipe Scaler Power Gate Control)
//
// Controls the power gate for pipe scaler memory.
//
// This register is double buffered. See PipeScalerWindowSize for arming and
// dis-arming double-buffer updates.
//
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0, Page 1014-1015
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 2, Pages 657-658
class PipeScalerPowerGateControl
    : public hwreg::RegisterBase<PipeScalerPowerGateControl, uint32_t> {
 public:
  static constexpr uint32_t kBaseAddr = 0x68160;

  DEF_RSVDZ_FIELD(30, 6);

  // Disables the dynamic power gate of unused memory embedded blocks (EBB)
  // when processing low resolution source images.
  //
  // This bit is only available on Tiger Lake. On Kaby Lake and Skylake,
  // this bit must be zero.
  DEF_BIT(5, dynamic_power_gate_disabled_tiger_lake);

  // Time for RAMs in a given filter group to settle after they are powered up.
  enum class FilterGroupRamSettlingTime : uint32_t {
    k32CoreDisplayClockPeriods = 0b00,
    k64CoreDisplayClockPeriods = 0b01,
    k96CoreDisplayClockPeriods = 0b10,
    k128CoreDisplayClockPeriods = 0b11,
  };
  // This field is only available on Kaby Lake and Skylake. On Tiger Lake, this
  // field is reserved.
  DEF_ENUM_FIELD(FilterGroupRamSettlingTime, 4, 3, filter_group_ram_settling_time_skylake);

  // Delay between sleep enables of individual banks of RAMs.
  enum class SleepEnableDelayTime : uint32_t {
    k8CoreDisplayClockPeriods = 0b00,
    k16CoreDisplayClockPeriods = 0b01,
    k24CoreDisplayClockPeriods = 0b10,
    k32CoreDisplayClockPeriods = 0b11,
  };
  // This field is only available on Kaby Lake and Skylake. On Tiger Lake, this
  // field is reserved.
  DEF_ENUM_FIELD(SleepEnableDelayTime, 1, 0, sleep_enable_delay_time_skylake);
};

// PS_HSCALE / PS_VSCALE (Pipe Scaler Horizontal / Vertical Scaling Factor)
//
// The pipe / plane scaling factor. The register uses a fixed-point
// representation to store the value, which can be converted to C float type
// by using helper method ToFloat().
//
// If the pipe / transcoder works in progressive display mode (PF-PD),
//     Horizontal scaling factor = Source width  / Destination width
//     Vertical   scaling factor = Source height / Destination height.
//
// If the pipe / transcoder works in interlaced display mode (PF-ID),
//     Horizontal scaling factor = 2 * Source width  / Destination width
//     Vertical   scaling factor = 2 * Source height / Destination height.
//
// This register is read only.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 2, Pages 1008-1009, 1019-1020
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 2, Pages 655-656, 662-663
// Skylake: IHD-OS-SKL-Vol 2c-05.16 Part 2, Pages 648-649, 655-656
class PipeScalerScalingFactor : public hwreg::RegisterBase<PipeScalerScalingFactor, uint32_t> {
 public:
  static constexpr uint32_t kHorizontalBaseAddr = 0x68190;
  static constexpr uint32_t kVerticalBaseAddr = 0x68184;

  DEF_FIELD(17, 15, integer_part);
  static constexpr uint32_t kIntegerPartSize = 3;

  DEF_FIELD(14, 0, fraction_part);
  static constexpr uint32_t kFractionPartSize = 15;

  // Converts the scaling factor to float format.
  //
  // The scaling factor is stored as an unsigned fixed point format with 3
  // integer bits and 15 fractional bits.
  //
  // On platforms using IEEE 754 standard, all the valid values stored in this
  // register can be converted to |float| without losing precision.
  float ToFloat() const {
    int x32768 = (static_cast<int>(integer_part()) << 15) + static_cast<int>(fraction_part());
    return static_cast<float>(x32768) / 32768.0f;
  }
};

// PS_WIN_SZ (Pipe Scaler Window Size)
//
// The scaler outputs the scaled contents to the scaler window, a rectangular
// region specified by `PipeScalerWindowPosition` and `PipeScalerWindowSize`
// on the output surface.
//
// This register specifies the size of the scaler window. "x" and "y" are
// relative to the display device, thus rotation / flipping of planes and pipe
// doesn't affect the order of x and y.
//
// This register and `PipeScalerWindowPosition` must be set so that the whole
// scaler window is within the output surface; see documentation of
// `PipeScalerWindowPosition` for the details of the constraints.
//
// This register is double buffered. Writing to this register will trigger
// double buffering arms to this register and all the other Pipe Scaler control
// registers so that they are updated at the start of the next vertical blank.
// So this register must be written at the end of all the pipe scaler
// configuration procedures.
//
// This register's reserved fields are all MBZ (must be zero). So, this register
// can be safely written without reading it first.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 2, Pages 1023-1024
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 2, Pages 664-665
class PipeScalerWindowSize : public hwreg::RegisterBase<PipeScalerWindowSize, uint32_t> {
 public:
  static constexpr uint32_t kBaseAddr = 0x68174;

  DEF_RSVDZ_FIELD(31, 29);

  // Horizontal (x) size of the rectangular area.
  //
  // Must not exceed 5120 for unjoined pipes and 7680 / #(joined pipes) for
  // joined pipes.
  // Must be even for planar YUV planes / pipes.
  DEF_FIELD(28, 16, x_size);

  DEF_RSVDZ_FIELD(15, 13);

  // Vertical (y) size of the rectangular area.
  //
  // Must not exceed 4096 for unjoined pipes and 4320 for joined pipes.
  // Must be even for planar YUV planes / pipes, or for interlaced output.
  DEF_FIELD(12, 0, y_size);
};

// An instance of PipeScalerRegs represents the registers for a particular pipe
// scaler.
class PipeScalerRegs {
 public:
  explicit PipeScalerRegs(intel_display::PipeId pipe_id, int scaler_index)
      : pipe_id_(pipe_id), scaler_index_(scaler_index) {}

  hwreg::RegisterAddr<PipeScalerControlSkylake> PipeScalerControlSkylake() {
    return hwreg::RegisterAddr<registers::PipeScalerControlSkylake>(
        PipeScalerControlSkylake::kBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  hwreg::RegisterAddr<PipeScalerControlTigerLake> PipeScalerControlTigerLake() {
    return hwreg::RegisterAddr<registers::PipeScalerControlTigerLake>(
        PipeScalerControlTigerLake::kBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  hwreg::RegisterAddr<PipeScalerAdaptiveFilterThresholds> PipeScalerAdaptiveFilterThresholds() {
    return hwreg::RegisterAddr<registers::PipeScalerAdaptiveFilterThresholds>(
        PipeScalerAdaptiveFilterThresholds::kBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  hwreg::RegisterAddr<PipeScalerCoefficientData> PipeScalerCoefficientData() {
    return hwreg::RegisterAddr<registers::PipeScalerCoefficientData>(
        PipeScalerCoefficientData::kBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  hwreg::RegisterAddr<PipeScalerCoefficientIndex> PipeScalerCoefficientIndex() {
    return hwreg::RegisterAddr<registers::PipeScalerCoefficientIndex>(
        PipeScalerCoefficientIndex::kBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  PipeScalerCoefficients PipeScalerCoefficients() {
    return registers::PipeScalerCoefficients(PipeScalerCoefficientIndex(),
                                             PipeScalerCoefficientData());
  }

  hwreg::RegisterAddr<PipeScalerHorizontalInitialPhase> PipeScalerHorizontalInitialPhase() {
    return hwreg::RegisterAddr<registers::PipeScalerHorizontalInitialPhase>(
        PipeScalerHorizontalInitialPhase::kBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  hwreg::RegisterAddr<PipeScalerVerticalInitialPhase> PipeScalerVerticalInitialPhase() {
    return hwreg::RegisterAddr<registers::PipeScalerVerticalInitialPhase>(
        PipeScalerVerticalInitialPhase::kBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  hwreg::RegisterAddr<PipeScalerPowerGateControl> PipeScalerPowerGateControl() {
    return hwreg::RegisterAddr<registers::PipeScalerPowerGateControl>(
        PipeScalerPowerGateControl::kBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  hwreg::RegisterAddr<PipeScalerScalingFactor> PipeScalerHorizontalScalingFactor() {
    return hwreg::RegisterAddr<registers::PipeScalerScalingFactor>(
        PipeScalerScalingFactor::kHorizontalBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  hwreg::RegisterAddr<PipeScalerScalingFactor> PipeScalerVerticalScalingFactor() {
    return hwreg::RegisterAddr<registers::PipeScalerScalingFactor>(
        PipeScalerScalingFactor::kVerticalBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  hwreg::RegisterAddr<PipeScalerWindowPosition> PipeScalerWindowPosition() {
    return hwreg::RegisterAddr<registers::PipeScalerWindowPosition>(
        PipeScalerWindowPosition::kBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

  hwreg::RegisterAddr<PipeScalerWindowSize> PipeScalerWindowSize() {
    return hwreg::RegisterAddr<registers::PipeScalerWindowSize>(
        PipeScalerWindowSize::kBaseAddr + 0x800 * pipe_id_ + scaler_index_ * 0x100);
  }

 private:
  intel_display::PipeId pipe_id_;
  int scaler_index_;
};

}  // namespace registers

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_PIPE_SCALER_H_
