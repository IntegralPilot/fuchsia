// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DISPLAY_TIMING_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DISPLAY_TIMING_H_

#include <zircon/assert.h>

#include <cstdint>
#include <limits>

// References
//
// The code contains references to the following documents.
//
// - Display Monitor Timing Standard, Video Electronics Standards Association
//   (VESA), Version 1.0, Rev. 13, dated February 8, 2013.
//   Referenced as "DMT standard".
//   Available at https://vesa.org/vesa-standards/ .
//
// - VESA Enhanced Extended Display Identification Data (E-EDID) Standard,
//   Video Electronics Standards Association (VESA), Release A, Revision 2,
//   dated September 25, 2006, revised December 31, 2020.
//   Referenced as "E-EDID standard".
//   Available at https://vesa.org/vesa-standards/ .
//
// - ANSI/CTA-861-I: A DTV Profile for Uncompressed High Speed Digital
//   Interfaces, Consumer Technology Association (CTA), dated February 2023.
//   Referenced as "CTA-861 standard".
//   Available at https://shop.cta.tech/products/cta-861 .

namespace display {

// Describes how a logic signal represents a sync pulse.
//
// The DMT standard Sections 3.1 to 3.4 have figures that illustrate
// both polarities for horizontal and vertical sync signals.
enum class SyncPolarity : uint8_t {
  // The sync signal is active-low. The pulse is low (logic 0).
  kNegative = 0,

  // The sync signal is active-high. The pulse is high (logic 1).
  kPositive = 1,
};

// Describes how a display frame is composed of and scanned.
enum class FieldsPerFrame : uint8_t {
  // Each frame is composed of only one field. The frame is scanned all at once.
  kProgressive = 0,

  // Each frame is composed of two fields interlaced line by line. The odd-
  // numbered lines are scanned the first and the even-numbered lines are
  // scanned the second.
  kInterlaced = 1,
};

// Maximum value of display timing in pixels / lines.
constexpr int32_t kMaxTimingValue = (1 << 16) - 1;

// Maximum value of display pixel clock in Hz.
constexpr int64_t kMaxPixelClockHz = int64_t{std::numeric_limits<int32_t>::max()} * 1'000;

// Maximum value of display refresh rate in millihertz (0.001 Hz).
constexpr int32_t kMaxRefreshRateMillihertz = ((1 << 16) - 1) * 1000;

// Display timing parameters as defined in:
//
// - DMT standard, Section 3. "DMT Video Timing Parameter Definitions",
//   pages 14-16.
// - CTA-861 standard, Section 4. "Video Formats and Waveform Timings",
//   pages 42-52.
// - E-EDID standard, Section 3.12 "Note Regarding Borders", pages 51-52.
//
// The struct uses signed `int32_t` values for the timing value fields instead
// of the unsigned `uint32` used by its FIDL / banjo counterparts.
struct DisplayTiming {
  // Number of pixels on a video line that are visible on the display.
  //
  // Also known as "HActive", "HACT" (horizontal active), "horizontal
  // addressable time" and "horizontal resolution".
  //
  // The DMT, DisplayID and E-EDID standards specify the active area as
  // the sum of the addressable area and two borders. For modern displays, the
  // borders are always zero, so the active area is the same as the addressable
  // area. In the rare case of legacy display modes with non-zero borders, we
  // store the addressable pixels in this field and merge borders into front /
  // back porches.
  //
  // Must be >= 0 and <= kMaxTimingValue.
  int32_t horizontal_active_px;

  // Number of blanking pixels between active pixels and the beginning of the
  // horizontal sync pulse.
  //
  // Also known as "Hfront", "HFP" and "horizontal offset".
  //
  // For legacy DMT standard formats with non-zero borders, this is the sum of
  // the horizontal right border and the horizontal front porch.
  //
  // Must be >= 0 and <= kMaxTimingValue.
  int32_t horizontal_front_porch_px;

  // Number of blanking pixels during the horizontal sync pulse.
  //
  // Also known as "Hsync", "HSA" (horizontal sync active) and "horizontal sync
  // time".
  //
  // Must be >= 0 and <= kMaxTimingValue.
  int32_t horizontal_sync_width_px;

  // Number of blanking pixels between the end of the horizontal sync pulse and
  // the active pixels.
  //
  // Also known as "Hback" and "HBP".
  //
  // For legacy DMT standard formats with non-zero borders, this is the sum of
  // the horizontal back porch and the horizontal left border.
  //
  // Must be >= 0 and <= kMaxTimingValue.
  int32_t horizontal_back_porch_px;

  // Number of video lines that are visible on the display.
  //
  // Also known as "VActive", "vertical addressable time" and "vertical
  // resolution".
  //
  // For legacy DMT standard formats with non-zero borders, this is the
  // "vertical addressable lines" rather than the "vertical active lines" which
  // includes borders.
  //
  // Must be >= 0 and <= kMaxTimingValue.
  int32_t vertical_active_lines;

  // Number of blanking lines between active lines and the beginning of the
  // vertical sync pulse.
  //
  // Also known as "Vfront" and "vertical offset".
  //
  // For legacy DMT standard formats with non-zero borders, this is the sum of
  // the vertical bottom border and the vertical front porch.
  //
  // Must be >= 0 and <= kMaxTimingValue.
  int32_t vertical_front_porch_lines;

  // Number of blanking lines during the vertical sync pulse.
  //
  // Also known as "Vsync", "VSA" (vertical sync active) and "vertical sync
  // time".
  //
  // Must be >= 0 and <= kMaxTimingValue.
  int32_t vertical_sync_width_lines;

  // Number of blanking lines between the end of the vertical sync pulse and
  // the active lines.
  //
  // Also known as "Vback" and "VBP".
  //
  // For legacy DMT standard formats with non-zero borders, this is the sum of
  // the vertical blank and the vertical top border.
  //
  // Must be >= 0 and <= kMaxTimingValue.
  int32_t vertical_back_porch_lines;

  // Frequency of pixels transmitted to the display.
  //
  // Must be >= 0 and <= kMaxPixelClockHz.
  int64_t pixel_clock_frequency_hz;

  FieldsPerFrame fields_per_frame = FieldsPerFrame::kProgressive;
  SyncPolarity hsync_polarity = SyncPolarity::kNegative;
  SyncPolarity vsync_polarity = SyncPolarity::kNegative;

  // If false, the vertical blank porch is integral and does not change over
  // time.
  // If true, the vertical blank porch has a fractional part of 0.5; for
  // the generated signal, both its vertical front porch and vertical back
  // porch must alternate between `vertical_{front,back}_porch_lines` and
  // `vertical_{front,back}_porch_lines + 1` on consecutive fields, as described
  // in:
  // - CTA-861 standard, Table 1. "Video Format Timings - Detailed Timing
  //   Information", Note a, page 45.
  // - CTA-861 standard, Figure 3-4. "General Interlaced Video Format Timing",
  //   pages 50-51.
  // This only applies to video formats specified in CTA-861.
  bool vblank_alternates = false;

  // If zero, pixels are not repeated. Otherwise, pixels are repeated for
  // `pixel_repetition + 1` times.
  //
  // A unique active pixel is formed by systematically repeating the preceding
  // Active Pixel, so the effective horizontal resolution is
  // `horizontal_addressable_px / (pixel_repetition + 1)`.
  //
  // This is used in some CTA-861 display formats. For more details, see:
  // - CTA-861 standard, Table 1. "Video Format Timings - Detailed Timing
  //   Information", Note b, page 45.
  // - CTA-861 standard, Table 2, "Video Formats -- Video ID Code and Aspect
  //   Ratios", Note b-d, page 57.
  // - CTA-861 standard, Section 6.4 "Format of Version 2, 3 and 4 AVI
  //   InfoFrames", pages 76-77.
  //
  // TODO(https://fxbug.dev/42084909): Support other `pixel_repetition` values allowed by
  // the AVI InfoFrame.
  //
  // Must be 0 or 1.
  int pixel_repetition = 0;

  constexpr bool IsValid() const;

  // Functionally equivalent to asserting on the return value from IsValid(),
  // but provides more actionable errors on failure.
  constexpr void DebugAssertIsValid() const;

  // Number of blanking pixels in a video line.
  //
  // Also known as "Hblank" and "horizontal blank time".
  //
  // For legacy DMT standard formats with non-zero borders, this is the sum of
  // the horizontal left border, horizontal right border and the horizontal
  // blank.
  constexpr int horizontal_blank_px() const {
    return horizontal_front_porch_px + horizontal_sync_width_px + horizontal_back_porch_px;
  }

  // Number of blanking lines on the display.
  //
  // Also known as "Vblank" and "vertical blank time".
  //
  // For legacy DMT standard formats with non-zero borders, this is the sum of
  // the vertical top border, vertical bottom bordea cvr and the vertical blank.
  constexpr int vertical_blank_lines() const {
    return vertical_front_porch_lines + vertical_sync_width_lines + vertical_back_porch_lines;
  }

  // Number of all pixels in a video line.
  //
  // Also known as "Htotal".
  //
  // If `IsValid()` is true, the value is guaranteed to be >= 0 and
  // <= kMaxTimingValue.
  //
  // TODO(https://fxbug.dev/42086614): The current display limits may not support some
  // timings allowed by the VESA DisplayID standard.
  constexpr int horizontal_total_px() const { return horizontal_active_px + horizontal_blank_px(); }

  // Number of all lines on the display.
  //
  // Also known as "Vtotal".
  //
  // If `IsValid()` is true, the value is guaranteed to be >= 0 and
  // <= kMaxTimingValue.
  //
  // TODO(https://fxbug.dev/42086614): The current display limits may not support some
  // timings allowed by the VESA DisplayID standard.
  constexpr int vertical_total_lines() const {
    // Interlaced display mode has 2 blanks.
    if (fields_per_frame == FieldsPerFrame::kInterlaced) {
      int total_vblanks = 2 * vertical_blank_lines() + (vblank_alternates ? 1 : 0);
      return vertical_active_lines + total_vblanks;
    }
    return vertical_active_lines + vertical_blank_lines();
  }

  // The number of fields the display device can display in 1,000 seconds.
  //
  // Rounded to the nearest millihertz (0.001 Hz).
  //
  // If `IsValid()` is true, the value is guaranteed to be >= 0 and
  // <= kMaxRefreshRateMillihertz.
  constexpr int32_t vertical_field_refresh_rate_millihertz() const;
};

constexpr bool DisplayTiming::IsValid() const {
  if (horizontal_active_px < 0 || horizontal_active_px > kMaxTimingValue) {
    return false;
  }
  if (horizontal_front_porch_px < 0 || horizontal_front_porch_px > kMaxTimingValue) {
    return false;
  }
  if (horizontal_sync_width_px < 0 || horizontal_sync_width_px > kMaxTimingValue) {
    return false;
  }
  if (horizontal_back_porch_px < 0 || horizontal_back_porch_px > kMaxTimingValue) {
    return false;
  }
  if (vertical_active_lines < 0 || vertical_active_lines > kMaxTimingValue) {
    return false;
  }
  if (vertical_front_porch_lines < 0 || vertical_front_porch_lines > kMaxTimingValue) {
    return false;
  }
  if (vertical_sync_width_lines < 0 || vertical_sync_width_lines > kMaxTimingValue) {
    return false;
  }
  if (vertical_back_porch_lines < 0 || vertical_back_porch_lines > kMaxTimingValue) {
    return false;
  }
  if (pixel_clock_frequency_hz < 0 || pixel_clock_frequency_hz > kMaxPixelClockHz) {
    return false;
  }
  if (pixel_repetition < 0 || pixel_repetition > 1) {
    return false;
  }
  if (horizontal_total_px() < 0 || horizontal_total_px() > kMaxTimingValue) {
    return false;
  }
  if (vertical_total_lines() < 0 || vertical_total_lines() > kMaxTimingValue) {
    return false;
  }
  if (vertical_field_refresh_rate_millihertz() < 0 ||
      vertical_field_refresh_rate_millihertz() > kMaxRefreshRateMillihertz) {
    return false;
  }
  return true;
}

constexpr void DisplayTiming::DebugAssertIsValid() const {
  ZX_DEBUG_ASSERT(horizontal_active_px >= 0);
  ZX_DEBUG_ASSERT(horizontal_active_px <= kMaxTimingValue);

  ZX_DEBUG_ASSERT(horizontal_front_porch_px >= 0);
  ZX_DEBUG_ASSERT(horizontal_front_porch_px <= kMaxTimingValue);

  ZX_DEBUG_ASSERT(horizontal_sync_width_px >= 0);
  ZX_DEBUG_ASSERT(horizontal_sync_width_px <= kMaxTimingValue);

  ZX_DEBUG_ASSERT(horizontal_back_porch_px >= 0);
  ZX_DEBUG_ASSERT(horizontal_back_porch_px <= kMaxTimingValue);

  ZX_DEBUG_ASSERT(vertical_active_lines >= 0);
  ZX_DEBUG_ASSERT(vertical_active_lines <= kMaxTimingValue);

  ZX_DEBUG_ASSERT(vertical_front_porch_lines >= 0);
  ZX_DEBUG_ASSERT(vertical_front_porch_lines <= kMaxTimingValue);

  ZX_DEBUG_ASSERT(vertical_sync_width_lines >= 0);
  ZX_DEBUG_ASSERT(vertical_sync_width_lines <= kMaxTimingValue);

  ZX_DEBUG_ASSERT(vertical_back_porch_lines >= 0);
  ZX_DEBUG_ASSERT(vertical_back_porch_lines <= kMaxTimingValue);

  ZX_DEBUG_ASSERT(pixel_clock_frequency_hz >= 0);
  ZX_DEBUG_ASSERT(pixel_clock_frequency_hz <= kMaxPixelClockHz);

  ZX_DEBUG_ASSERT(pixel_repetition >= 0);
  ZX_DEBUG_ASSERT(pixel_repetition <= 1);

  ZX_DEBUG_ASSERT(horizontal_total_px() >= 0);
  ZX_DEBUG_ASSERT(horizontal_total_px() <= kMaxTimingValue);

  ZX_DEBUG_ASSERT(vertical_total_lines() >= 0);
  ZX_DEBUG_ASSERT(vertical_total_lines() <= kMaxTimingValue);

  ZX_DEBUG_ASSERT(vertical_field_refresh_rate_millihertz() >= 0);
  ZX_DEBUG_ASSERT(vertical_field_refresh_rate_millihertz() <= kMaxRefreshRateMillihertz);
}

constexpr int32_t DisplayTiming::vertical_field_refresh_rate_millihertz() const {
  constexpr int64_t kMillihertzPerHertz = 1'000;

  // The multiplication won't overflow, which would cause an undefined
  // behavior.
  //
  // `pixel_clock_frequency_hz` < 2^41 and `kMillihertzPerHertz` < 2^10.
  // So, the multiplication result is less than 2^51, which falls within the
  // valid range of an int64_t number.
  const int64_t pixel_clock_millihertz = pixel_clock_frequency_hz * kMillihertzPerHertz;

  // The multiplication won't overflow, which would cause an undefined
  // behavior.
  //
  // Both `horizontal_total` and `vertical_total` won't exceed 2^16, thus
  // the multiplication result is less than 2^32, which falls within the
  // valid range of an int64_t number.
  const int64_t pixels_per_frame = int64_t{horizontal_total_px()} * vertical_total_lines();

  const int num_fields_per_frame = (fields_per_frame == FieldsPerFrame::kInterlaced) ? 2 : 1;

  // The formula below is correct, which can be proved as follows:
  //
  //    Vertical frame refresh rate = Pixel clock / Pixels per frame.
  //
  // Thus,
  //
  //    Vertical field refresh rate =
  //  = Vertical frame refresh rate * Fields per frame
  //  = Pixel clock * Fields per frame / Pixels per frame.
  //
  // Taking units and rounding into consideration,
  //
  //    Vertical field refresh rate (millihertz)
  //  = round(Pixel clock (millihertz) * Fields per frame / Pixels per frame)
  //  = (Pixel clock (millihertz) * Fields per frame + Pixels per frame / 2) div Pixels per frame
  //
  // Because Pixel clock (millihertz) < 2^51, Pixels per frame < 2^32 and
  // Fields per frame = 1 or 2, the intermediate and final arithmetic results
  // will all fit in int64_t numbers.
  const int64_t vertical_field_refresh_rate_millihertz =
      (pixel_clock_millihertz * num_fields_per_frame + pixels_per_frame / 2) / pixels_per_frame;

  ZX_DEBUG_ASSERT(vertical_field_refresh_rate_millihertz >= 0);
  ZX_DEBUG_ASSERT(vertical_field_refresh_rate_millihertz <= kMaxRefreshRateMillihertz);

  // Since `vertical_field_refresh_rate_millihertz` is guaranteed to be within
  // [0, kMaxRefreshRateMillihertz] where the lower and upper bounds are int32_t
  // values, it's safe to cast it to int32_t.
  return static_cast<int32_t>(vertical_field_refresh_rate_millihertz);
}

constexpr bool operator==(const DisplayTiming& lhs, const DisplayTiming& rhs) {
  return lhs.horizontal_active_px == rhs.horizontal_active_px &&
         lhs.horizontal_front_porch_px == rhs.horizontal_front_porch_px &&
         lhs.horizontal_sync_width_px == rhs.horizontal_sync_width_px &&
         lhs.horizontal_back_porch_px == rhs.horizontal_back_porch_px &&
         lhs.vertical_active_lines == rhs.vertical_active_lines &&
         lhs.vertical_front_porch_lines == rhs.vertical_front_porch_lines &&
         lhs.vertical_sync_width_lines == rhs.vertical_sync_width_lines &&
         lhs.vertical_back_porch_lines == rhs.vertical_back_porch_lines &&
         lhs.pixel_clock_frequency_hz == rhs.pixel_clock_frequency_hz &&
         lhs.fields_per_frame == rhs.fields_per_frame && lhs.hsync_polarity == rhs.hsync_polarity &&
         lhs.vsync_polarity == rhs.vsync_polarity &&
         lhs.vblank_alternates == rhs.vblank_alternates &&
         lhs.pixel_repetition == rhs.pixel_repetition;
}

constexpr bool operator!=(const DisplayTiming& lhs, const DisplayTiming& rhs) {
  return !(lhs == rhs);
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DISPLAY_TIMING_H_
