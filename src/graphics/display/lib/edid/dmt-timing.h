// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_EDID_DMT_TIMING_H_
#define SRC_GRAPHICS_DISPLAY_LIB_EDID_DMT_TIMING_H_

#include <cinttypes>
#include <span>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/edid/timings.h"

namespace edid::internal {

// References
//
// The code contains references to the following documents.
//
// - Display Monitor Timing Standard, Video Electronics Standards Association
//   (VESA), Version 1.0, Rev. 13, dated February 8, 2013.
//   Referenced as "DMT standard".
//   Available at https://vesa.org/vesa-standards/ .

// Display timing parameters as defined in the DMT standard.
//
// This struct has data members for most numbers provided by the DMT standard.
// Some data members are redundant. Including these members makes it easier to
// compare our data against the standard. The redundant values are checked in
// automated tests, which helps protect against some minor errors.
struct DmtTiming {
  // The ID of the timing used in EDID.
  // Also known as "DMT ID" in the DMT standard.
  int32_t id;

  // Average number of fields displayed in a second, rounded to the nearest
  // millihertz (0.001 Hz).
  // Also known as "Ver Frequency" in the DMT standard.
  int32_t vertical_field_refresh_rate_millihertz;

  // Frequency of pixels transmitted to the display, rounded to the nearest
  // kilohertz (1000 Hz).
  // Also known as "Pixel Clock" in the DMT standard.
  int32_t pixel_clock_khz;

  // Also known as "Scan Type" in the DMT standard.
  display::FieldsPerFrame fields_per_frame;

  // Also known as "Hor Sync Polarity" in the DMT standard.
  display::SyncPolarity horizontal_sync_polarity;

  // Also known as "Ver Sync Polarity" in the DMT standard.
  display::SyncPolarity vertical_sync_polarity;

  // Number of all pixels in a video line.
  // Also known as "Hor Total Time" in the DMT standard.
  int32_t horizontal_total_px;

  // Number of pixels on a video line that are visible on the display (excluding
  // borders).
  // Also known as "Hor Addr Time" in the DMT standard.
  int32_t horizontal_active_px;

  // Number of pixels between the start of the active image (including borders)
  // and the start of the horizontal blank (excluding borders).
  // Also known as "Hor Blank Start" in the DMT standard.
  int32_t horizontal_blank_start_px;

  // Number of blanking pixels in a video line (excluding borders).
  // Also known as "Hor Blank Time" in the DMT standard.
  int32_t horizontal_blank_px;

  // Number of pixels between the start of the active image (including borders)
  // and the start of the horizontal sync pulse.
  // Also known as "Hor Sync Start" in the DMT standard.
  int32_t horitontal_sync_start_px;

  // Number of the pixels in the right border of the active image.
  // Also known as "H Right Border" in the DMT standard.
  int32_t horizontal_right_border_px;

  // Number of blanking pixels between active pixels (including the border)
  // and the beginning of the horizontal sync pulse.
  // Also known as "H Front Porch" in the DMT standard.
  int32_t horizontal_front_porch_px;

  // Number of blanking pixels during the horizontal sync pulse.
  // Also known as "Hor Sync Time" in the DMT standard.
  int32_t horizontal_sync_width_px;

  // Number of blanking pixels between the end of the horizontal sync pulse and
  // the beginning of the active pixels (including the border).
  // Also known as "H Back Porch" in the DMT standard.
  int32_t horizontal_back_porch_px;

  // Number of the pixels in the left border of the active image.
  // Also known as "H Left Border" in the DMT standard.
  int32_t horizontal_left_border_px;

  // Number of all video lines on the display for each frame.
  // Also known as "Ver Total Time" in the DMT standard.
  int32_t vertical_total_lines;

  // Number of video lines that are visible on the display (excluding borders)
  // for each frame.
  // Also known as "Ver Addr Time" in the DMT standard.
  int32_t vertical_active_lines;

  // Number of video lines between the start of the active image (including
  // borders) and the start of the vertical blank (excluding borders).
  // Also known as "Ver Blank Start" in the DMT standard.
  int32_t vertical_blank_start_lines;

  // Number of blanking lines on the display for each field (excluding borders).
  // Also known as "Ver Blank Time" in the DMT standard.
  int32_t vertical_blank_lines;

  // Number of video lines between the start of the active image (including
  // borders) and the start of the vertical sync pulse (excluding borders).
  // Also known as "Ver Sync Start" in the DMT standard.
  int32_t vertical_sync_start_lines;

  // Number of the video lines in the bottom border of the active image.
  // Also known as "V Bottom Border" in the DMT standard.
  int32_t vertical_bottom_border_lines;

  // Number of blanking lines between active lines (including the border)
  // and the beginning of the vertical sync pulse.
  // Also known as "V Front Porch" in the DMT standard.
  int32_t vertical_front_porch_lines;

  // Number of blanking lines during the vertical sync pulse.
  // Also known as "Ver Sync Width" in the DMT standard.
  int32_t vertical_sync_width_lines;

  // Number of blanking lines between the end of the vertical sync pulse and
  // the beginning of the active lines (including the border).
  // Also known as "V Back Porch" in the DMT standard.
  int32_t vertical_back_porch_lines;

  // Number of the video lines in the top border of the active image.
  // Also known as "V Top Border" in the DMT standard.
  int32_t vertical_top_border_lines;
};

constexpr display::DisplayTiming ToDisplayTiming(const DmtTiming& dmt) {
  const bool is_interlaced = dmt.fields_per_frame == display::FieldsPerFrame::kInterlaced;
  const int32_t total_vertical_blank_including_borders_lines_per_frame =
      dmt.vertical_total_lines - dmt.vertical_active_lines;
  const bool vblank_alternates =
      is_interlaced && total_vertical_blank_including_borders_lines_per_frame % 2 == 1;

  return display::DisplayTiming{
      .horizontal_active_px = dmt.horizontal_active_px,
      // The borders are included in the front and back porches in DisplayTiming
      // but not in DmtTiming, so we need to recalculate them to include
      // borders.
      .horizontal_front_porch_px = dmt.horizontal_right_border_px + dmt.horizontal_front_porch_px,
      .horizontal_sync_width_px = dmt.horizontal_sync_width_px,
      .horizontal_back_porch_px = dmt.horizontal_back_porch_px + dmt.horizontal_left_border_px,
      .vertical_active_lines = dmt.vertical_active_lines,
      .vertical_front_porch_lines =
          dmt.vertical_bottom_border_lines + dmt.vertical_front_porch_lines,
      .vertical_sync_width_lines = dmt.vertical_sync_width_lines,
      .vertical_back_porch_lines = dmt.vertical_back_porch_lines + dmt.vertical_top_border_lines,
      .pixel_clock_frequency_hz = int64_t{dmt.pixel_clock_khz} * 1'000,
      .fields_per_frame = dmt.fields_per_frame,
      .hsync_polarity = dmt.horizontal_sync_polarity,
      .vsync_polarity = dmt.vertical_sync_polarity,
      .vblank_alternates = vblank_alternates,
      // DMT formats never repeat the pixels.
      .pixel_repetition = false,
  };
}

// Timings taken from the DMT standard.
constexpr DmtTiming kDmtTimingArray[] = {
    DmtTiming{
        .id = 0x01,  // Page 18

        .vertical_field_refresh_rate_millihertz = 85'080,
        .pixel_clock_khz = 31'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 832,
        .horizontal_active_px = 640,
        .horizontal_blank_start_px = 640,
        .horizontal_blank_px = 192,
        .horitontal_sync_start_px = 672,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 32,
        .horizontal_sync_width_px = 64,
        .horizontal_back_porch_px = 96,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 445,
        .vertical_active_lines = 350,
        .vertical_blank_start_lines = 350,
        .vertical_blank_lines = 95,
        .vertical_sync_start_lines = 382,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 32,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 60,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x02,  // Page 19

        .vertical_field_refresh_rate_millihertz = 85'080,
        .pixel_clock_khz = 31'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 832,
        .horizontal_active_px = 640,
        .horizontal_blank_start_px = 640,
        .horizontal_blank_px = 192,
        .horitontal_sync_start_px = 672,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 32,
        .horizontal_sync_width_px = 64,
        .horizontal_back_porch_px = 96,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 445,
        .vertical_active_lines = 400,
        .vertical_blank_start_lines = 400,
        .vertical_blank_lines = 45,
        .vertical_sync_start_lines = 401,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 41,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x03,  // Page 20

        .vertical_field_refresh_rate_millihertz = 85'039,
        .pixel_clock_khz = 35'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 936,
        .horizontal_active_px = 720,
        .horizontal_blank_start_px = 720,
        .horizontal_blank_px = 216,
        .horitontal_sync_start_px = 756,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 36,
        .horizontal_sync_width_px = 72,
        .horizontal_back_porch_px = 108,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 446,
        .vertical_active_lines = 400,
        .vertical_blank_start_lines = 400,
        .vertical_blank_lines = 46,
        .vertical_sync_start_lines = 401,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 42,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x04,  // Page 21

        .vertical_field_refresh_rate_millihertz = 59'940,
        .pixel_clock_khz = 25'175,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 800,
        .horizontal_active_px = 640,
        .horizontal_blank_start_px = 648,
        .horizontal_blank_px = 144,
        .horitontal_sync_start_px = 656,

        .horizontal_right_border_px = 8,
        .horizontal_front_porch_px = 8,
        .horizontal_sync_width_px = 96,
        .horizontal_back_porch_px = 40,
        .horizontal_left_border_px = 8,

        .vertical_total_lines = 525,
        .vertical_active_lines = 480,
        .vertical_blank_start_lines = 488,
        .vertical_blank_lines = 29,
        .vertical_sync_start_lines = 490,

        .vertical_bottom_border_lines = 8,
        .vertical_front_porch_lines = 2,
        .vertical_sync_width_lines = 2,
        .vertical_back_porch_lines = 25,
        .vertical_top_border_lines = 8,
    },
    DmtTiming{
        .id = 0x05,  // Page 22

        .vertical_field_refresh_rate_millihertz = 72'809,
        .pixel_clock_khz = 31'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 832,
        .horizontal_active_px = 640,
        .horizontal_blank_start_px = 648,
        .horizontal_blank_px = 176,
        .horitontal_sync_start_px = 664,

        .horizontal_right_border_px = 8,
        .horizontal_front_porch_px = 16,
        .horizontal_sync_width_px = 40,
        .horizontal_back_porch_px = 120,
        .horizontal_left_border_px = 8,

        .vertical_total_lines = 520,
        .vertical_active_lines = 480,
        .vertical_blank_start_lines = 488,
        .vertical_blank_lines = 24,
        .vertical_sync_start_lines = 489,

        .vertical_bottom_border_lines = 8,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 20,
        .vertical_top_border_lines = 8,
    },
    DmtTiming{
        .id = 0x06,  // Page 23

        .vertical_field_refresh_rate_millihertz = 75'000,
        .pixel_clock_khz = 31'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 840,
        .horizontal_active_px = 640,
        .horizontal_blank_start_px = 640,
        .horizontal_blank_px = 200,
        .horitontal_sync_start_px = 656,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 16,
        .horizontal_sync_width_px = 64,
        .horizontal_back_porch_px = 120,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 500,
        .vertical_active_lines = 480,
        .vertical_blank_start_lines = 480,
        .vertical_blank_lines = 20,
        .vertical_sync_start_lines = 481,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 16,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x07,  // Page 24

        .vertical_field_refresh_rate_millihertz = 85'008,
        .pixel_clock_khz = 36'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 832,
        .horizontal_active_px = 640,
        .horizontal_blank_start_px = 640,
        .horizontal_blank_px = 192,
        .horitontal_sync_start_px = 696,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 56,
        .horizontal_sync_width_px = 56,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 509,
        .vertical_active_lines = 480,
        .vertical_blank_start_lines = 480,
        .vertical_blank_lines = 29,
        .vertical_sync_start_lines = 481,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 25,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x08,  // Page 25

        .vertical_field_refresh_rate_millihertz = 56'250,
        .pixel_clock_khz = 36'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1024,
        .horizontal_active_px = 800,
        .horizontal_blank_start_px = 800,
        .horizontal_blank_px = 224,
        .horitontal_sync_start_px = 824,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 24,
        .horizontal_sync_width_px = 72,
        .horizontal_back_porch_px = 128,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 625,
        .vertical_active_lines = 600,
        .vertical_blank_start_lines = 600,
        .vertical_blank_lines = 25,
        .vertical_sync_start_lines = 601,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 2,
        .vertical_back_porch_lines = 22,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x09,  // Page 26

        .vertical_field_refresh_rate_millihertz = 60'317,
        .pixel_clock_khz = 40'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1056,
        .horizontal_active_px = 800,
        .horizontal_blank_start_px = 800,
        .horizontal_blank_px = 256,
        .horitontal_sync_start_px = 840,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 40,
        .horizontal_sync_width_px = 128,
        .horizontal_back_porch_px = 88,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 628,
        .vertical_active_lines = 600,
        .vertical_blank_start_lines = 600,
        .vertical_blank_lines = 28,
        .vertical_sync_start_lines = 601,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 23,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x0A,  // Page 27

        .vertical_field_refresh_rate_millihertz = 72'188,
        .pixel_clock_khz = 50'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1040,
        .horizontal_active_px = 800,
        .horizontal_blank_start_px = 800,
        .horizontal_blank_px = 240,
        .horitontal_sync_start_px = 856,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 56,
        .horizontal_sync_width_px = 120,
        .horizontal_back_porch_px = 64,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 666,
        .vertical_active_lines = 600,
        .vertical_blank_start_lines = 600,
        .vertical_blank_lines = 66,
        .vertical_sync_start_lines = 637,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 37,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 23,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x0B,  // Page 28

        .vertical_field_refresh_rate_millihertz = 75'000,
        .pixel_clock_khz = 49'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1056,
        .horizontal_active_px = 800,
        .horizontal_blank_start_px = 800,
        .horizontal_blank_px = 256,
        .horitontal_sync_start_px = 816,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 16,
        .horizontal_sync_width_px = 80,
        .horizontal_back_porch_px = 160,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 625,
        .vertical_active_lines = 600,
        .vertical_blank_start_lines = 600,
        .vertical_blank_lines = 25,
        .vertical_sync_start_lines = 601,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 21,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x0C,  // Page 29

        .vertical_field_refresh_rate_millihertz = 85'061,
        .pixel_clock_khz = 56'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1048,
        .horizontal_active_px = 800,
        .horizontal_blank_start_px = 800,
        .horizontal_blank_px = 248,
        .horitontal_sync_start_px = 832,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 32,
        .horizontal_sync_width_px = 64,
        .horizontal_back_porch_px = 152,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 631,
        .vertical_active_lines = 600,
        .vertical_blank_start_lines = 600,
        .vertical_blank_lines = 31,
        .vertical_sync_start_lines = 601,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 27,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x0D,  // Page 30

        .vertical_field_refresh_rate_millihertz = 119'972,
        .pixel_clock_khz = 73'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 960,
        .horizontal_active_px = 800,
        .horizontal_blank_start_px = 800,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 848,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 636,
        .vertical_active_lines = 600,
        .vertical_blank_start_lines = 600,
        .vertical_blank_lines = 36,
        .vertical_sync_start_lines = 603,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 29,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x0E,  // Page 31

        .vertical_field_refresh_rate_millihertz = 60'000,
        .pixel_clock_khz = 33'750,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1088,
        .horizontal_active_px = 848,
        .horizontal_blank_start_px = 848,
        .horizontal_blank_px = 240,
        .horitontal_sync_start_px = 864,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 16,
        .horizontal_sync_width_px = 112,
        .horizontal_back_porch_px = 112,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 517,
        .vertical_active_lines = 480,
        .vertical_blank_start_lines = 480,
        .vertical_blank_lines = 37,
        .vertical_sync_start_lines = 486,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 6,
        .vertical_sync_width_lines = 8,
        .vertical_back_porch_lines = 23,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x0F,  // Page 32

        // The DMT standard states that the vertical frequency is 86.957 Hz per
        // field. However, the actual vertical refresh rate is 86.957532 Hz per
        // field which should be rounded to 86.958 Hz instead.
        .vertical_field_refresh_rate_millihertz = 86'958,
        .pixel_clock_khz = 44'900,

        .fields_per_frame = display::FieldsPerFrame::kInterlaced,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1264,
        .horizontal_active_px = 1024,
        .horizontal_blank_start_px = 1024,
        .horizontal_blank_px = 240,
        .horitontal_sync_start_px = 1032,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 8,
        .horizontal_sync_width_px = 176,
        .horizontal_back_porch_px = 56,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 817,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 24,
        .vertical_sync_start_lines = 768,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 0,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 20,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x10,  // Page 33

        .vertical_field_refresh_rate_millihertz = 60'004,
        .pixel_clock_khz = 65'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1344,
        .horizontal_active_px = 1024,
        .horizontal_blank_start_px = 1024,
        .horizontal_blank_px = 320,
        .horitontal_sync_start_px = 1048,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 24,
        .horizontal_sync_width_px = 136,
        .horizontal_back_porch_px = 160,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 806,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 38,
        .vertical_sync_start_lines = 771,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 29,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x11,  // Page 34

        .vertical_field_refresh_rate_millihertz = 70'069,
        .pixel_clock_khz = 75'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1328,
        .horizontal_active_px = 1024,
        .horizontal_blank_start_px = 1024,
        .horizontal_blank_px = 304,
        .horitontal_sync_start_px = 1048,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 24,
        .horizontal_sync_width_px = 136,
        .horizontal_back_porch_px = 144,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 806,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 38,
        .vertical_sync_start_lines = 771,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 29,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x12,  // Page 35

        .vertical_field_refresh_rate_millihertz = 75'029,
        .pixel_clock_khz = 78'750,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1312,
        .horizontal_active_px = 1024,
        .horizontal_blank_start_px = 1024,
        .horizontal_blank_px = 288,
        .horitontal_sync_start_px = 1040,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 16,
        .horizontal_sync_width_px = 96,
        .horizontal_back_porch_px = 176,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 800,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 32,
        .vertical_sync_start_lines = 769,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 28,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x13,  // Page 36

        .vertical_field_refresh_rate_millihertz = 84'997,
        .pixel_clock_khz = 94'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1376,
        .horizontal_active_px = 1024,
        .horizontal_blank_start_px = 1024,
        .horizontal_blank_px = 352,
        .horitontal_sync_start_px = 1072,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 96,
        .horizontal_back_porch_px = 208,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 808,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 40,
        .vertical_sync_start_lines = 769,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 36,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x14,  // Page 37

        .vertical_field_refresh_rate_millihertz = 119'989,
        .pixel_clock_khz = 115'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1184,
        .horizontal_active_px = 1024,
        .horizontal_blank_start_px = 1024,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1072,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 813,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 45,
        .vertical_sync_start_lines = 771,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 38,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x15,  // Page 38

        .vertical_field_refresh_rate_millihertz = 75'000,
        .pixel_clock_khz = 108'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1600,
        .horizontal_active_px = 1152,
        .horizontal_blank_start_px = 1152,
        .horizontal_blank_px = 448,
        .horitontal_sync_start_px = 1216,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 64,
        .horizontal_sync_width_px = 128,
        .horizontal_back_porch_px = 256,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 900,
        .vertical_active_lines = 864,
        .vertical_blank_start_lines = 864,
        .vertical_blank_lines = 36,
        .vertical_sync_start_lines = 865,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 32,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x16,  // Page 40

        .vertical_field_refresh_rate_millihertz = 59'995,
        .pixel_clock_khz = 68'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1440,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1328,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 790,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 22,
        .vertical_sync_start_lines = 771,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 7,
        .vertical_back_porch_lines = 12,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x17,  // Page 41

        .vertical_field_refresh_rate_millihertz = 59'870,
        .pixel_clock_khz = 79'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1664,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 384,
        .horitontal_sync_start_px = 1344,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 64,
        .horizontal_sync_width_px = 128,
        .horizontal_back_porch_px = 192,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 798,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 30,
        .vertical_sync_start_lines = 771,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 7,
        .vertical_back_porch_lines = 20,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x18,  // Page 42

        .vertical_field_refresh_rate_millihertz = 74'893,
        .pixel_clock_khz = 102'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1696,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 416,
        .horitontal_sync_start_px = 1360,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 80,
        .horizontal_sync_width_px = 128,
        .horizontal_back_porch_px = 208,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 805,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 37,
        .vertical_sync_start_lines = 771,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 7,
        .vertical_back_porch_lines = 27,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x19,  // Page 43

        .vertical_field_refresh_rate_millihertz = 84'837,
        .pixel_clock_khz = 117'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1712,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 432,
        .horitontal_sync_start_px = 1360,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 80,
        .horizontal_sync_width_px = 136,
        .horizontal_back_porch_px = 216,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 809,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 41,
        .vertical_sync_start_lines = 771,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 7,
        .vertical_back_porch_lines = 31,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x1A,  // Page 44

        .vertical_field_refresh_rate_millihertz = 119'798,
        .pixel_clock_khz = 140'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1440,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1328,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 813,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 45,
        .vertical_sync_start_lines = 771,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 7,
        .vertical_back_porch_lines = 35,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x1B,  // Page 45

        .vertical_field_refresh_rate_millihertz = 59'910,
        .pixel_clock_khz = 71'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1440,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1328,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 823,
        .vertical_active_lines = 800,
        .vertical_blank_start_lines = 800,
        .vertical_blank_lines = 23,
        .vertical_sync_start_lines = 803,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 14,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x1C,  // Page 46

        .vertical_field_refresh_rate_millihertz = 59'810,
        .pixel_clock_khz = 83'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1680,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 400,
        .horitontal_sync_start_px = 1352,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 72,
        .horizontal_sync_width_px = 128,
        .horizontal_back_porch_px = 200,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 831,
        .vertical_active_lines = 800,
        .vertical_blank_start_lines = 800,
        .vertical_blank_lines = 31,
        .vertical_sync_start_lines = 803,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 22,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x1D,  // Page 47

        .vertical_field_refresh_rate_millihertz = 74'934,
        .pixel_clock_khz = 106'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1696,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 416,
        .horitontal_sync_start_px = 1360,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 80,
        .horizontal_sync_width_px = 128,
        .horizontal_back_porch_px = 208,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 838,
        .vertical_active_lines = 800,
        .vertical_blank_start_lines = 800,
        .vertical_blank_lines = 38,
        .vertical_sync_start_lines = 803,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 29,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x1E,  // Page 48

        .vertical_field_refresh_rate_millihertz = 84'880,
        .pixel_clock_khz = 122'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1712,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 432,
        .horitontal_sync_start_px = 1360,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 80,
        .horizontal_sync_width_px = 136,
        .horizontal_back_porch_px = 216,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 843,
        .vertical_active_lines = 800,
        .vertical_blank_start_lines = 800,
        .vertical_blank_lines = 43,
        .vertical_sync_start_lines = 803,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 34,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x1F,  // Page 49

        .vertical_field_refresh_rate_millihertz = 119'909,
        .pixel_clock_khz = 146'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1440,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1328,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 847,
        .vertical_active_lines = 800,
        .vertical_blank_start_lines = 800,
        .vertical_blank_lines = 47,
        .vertical_sync_start_lines = 803,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 38,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x20,  // Page 50

        .vertical_field_refresh_rate_millihertz = 60'000,
        .pixel_clock_khz = 108'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1800,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 520,
        .horitontal_sync_start_px = 1376,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 96,
        .horizontal_sync_width_px = 112,
        .horizontal_back_porch_px = 312,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1000,
        .vertical_active_lines = 960,
        .vertical_blank_start_lines = 960,
        .vertical_blank_lines = 40,
        .vertical_sync_start_lines = 961,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 36,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x21,  // Page 51

        .vertical_field_refresh_rate_millihertz = 85'002,
        .pixel_clock_khz = 148'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1728,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 448,
        .horitontal_sync_start_px = 1344,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 64,
        .horizontal_sync_width_px = 160,
        .horizontal_back_porch_px = 224,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1011,
        .vertical_active_lines = 960,
        .vertical_blank_start_lines = 960,
        .vertical_blank_lines = 51,
        .vertical_sync_start_lines = 961,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 47,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x22,  // Page 52

        .vertical_field_refresh_rate_millihertz = 119'838,
        .pixel_clock_khz = 175'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1440,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1328,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1017,
        .vertical_active_lines = 960,
        .vertical_blank_start_lines = 960,
        .vertical_blank_lines = 57,
        .vertical_sync_start_lines = 963,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 50,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x23,  // Page 53

        .vertical_field_refresh_rate_millihertz = 60'020,
        .pixel_clock_khz = 108'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1688,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 408,
        .horitontal_sync_start_px = 1328,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 112,
        .horizontal_back_porch_px = 248,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1066,
        .vertical_active_lines = 1024,
        .vertical_blank_start_lines = 1024,
        .vertical_blank_lines = 42,
        .vertical_sync_start_lines = 1025,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 38,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x24,  // Page 54

        .vertical_field_refresh_rate_millihertz = 75'025,
        .pixel_clock_khz = 135'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1688,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 408,
        .horitontal_sync_start_px = 1296,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 16,
        .horizontal_sync_width_px = 144,
        .horizontal_back_porch_px = 248,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1066,
        .vertical_active_lines = 1024,
        .vertical_blank_start_lines = 1024,
        .vertical_blank_lines = 42,
        .vertical_sync_start_lines = 1025,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 38,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x25,  // Page 55

        .vertical_field_refresh_rate_millihertz = 85'024,
        .pixel_clock_khz = 157'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1728,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 448,
        .horitontal_sync_start_px = 1344,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 64,
        .horizontal_sync_width_px = 160,
        .horizontal_back_porch_px = 224,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1072,
        .vertical_active_lines = 1024,
        .vertical_blank_start_lines = 1024,
        .vertical_blank_lines = 48,
        .vertical_sync_start_lines = 1025,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 44,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x26,  // Page 56

        .vertical_field_refresh_rate_millihertz = 119'958,
        .pixel_clock_khz = 187'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1440,
        .horizontal_active_px = 1280,
        .horizontal_blank_start_px = 1280,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1328,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1084,
        .vertical_active_lines = 1024,
        .vertical_blank_start_lines = 1024,
        .vertical_blank_lines = 60,
        .vertical_sync_start_lines = 1027,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 7,
        .vertical_back_porch_lines = 50,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x27,  // Page 57

        .vertical_field_refresh_rate_millihertz = 60'015,
        .pixel_clock_khz = 85'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1792,
        .horizontal_active_px = 1360,
        .horizontal_blank_start_px = 1360,
        .horizontal_blank_px = 432,
        .horitontal_sync_start_px = 1424,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 64,
        .horizontal_sync_width_px = 112,
        .horizontal_back_porch_px = 256,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 795,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 27,
        .vertical_sync_start_lines = 771,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 18,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x28,  // Page 58

        .vertical_field_refresh_rate_millihertz = 119'967,
        .pixel_clock_khz = 148'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1520,
        .horizontal_active_px = 1360,
        .horizontal_blank_start_px = 1360,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1408,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 813,
        .vertical_active_lines = 768,
        .vertical_blank_start_lines = 768,
        .vertical_blank_lines = 45,
        .vertical_sync_start_lines = 771,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 5,
        .vertical_back_porch_lines = 37,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x29,  // Page 61

        .vertical_field_refresh_rate_millihertz = 59'948,
        .pixel_clock_khz = 101'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1560,
        .horizontal_active_px = 1400,
        .horizontal_blank_start_px = 1400,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1448,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1080,
        .vertical_active_lines = 1050,
        .vertical_blank_start_lines = 1050,
        .vertical_blank_lines = 30,
        .vertical_sync_start_lines = 1053,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 23,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x2A,  // Page 62

        .vertical_field_refresh_rate_millihertz = 59'978,
        .pixel_clock_khz = 121'750,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1864,
        .horizontal_active_px = 1400,
        .horizontal_blank_start_px = 1400,
        .horizontal_blank_px = 464,
        .horitontal_sync_start_px = 1488,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 88,
        .horizontal_sync_width_px = 144,
        .horizontal_back_porch_px = 232,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1089,
        .vertical_active_lines = 1050,
        .vertical_blank_start_lines = 1050,
        .vertical_blank_lines = 39,
        .vertical_sync_start_lines = 1053,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 32,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x2B,  // Page 63

        .vertical_field_refresh_rate_millihertz = 74'867,
        .pixel_clock_khz = 156'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1896,
        .horizontal_active_px = 1400,
        .horizontal_blank_start_px = 1400,
        .horizontal_blank_px = 496,
        .horitontal_sync_start_px = 1504,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 104,
        .horizontal_sync_width_px = 144,
        .horizontal_back_porch_px = 248,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1099,
        .vertical_active_lines = 1050,
        .vertical_blank_start_lines = 1050,
        .vertical_blank_lines = 49,
        .vertical_sync_start_lines = 1053,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 42,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x2C,  // Page 64

        .vertical_field_refresh_rate_millihertz = 84'960,
        .pixel_clock_khz = 179'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1912,
        .horizontal_active_px = 1400,
        .horizontal_blank_start_px = 1400,
        .horizontal_blank_px = 512,
        .horitontal_sync_start_px = 1504,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 104,
        .horizontal_sync_width_px = 152,
        .horizontal_back_porch_px = 256,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1105,
        .vertical_active_lines = 1050,
        .vertical_blank_start_lines = 1050,
        .vertical_blank_lines = 55,
        .vertical_sync_start_lines = 1053,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 48,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x2D,  // Page 65

        .vertical_field_refresh_rate_millihertz = 119'904,
        .pixel_clock_khz = 208'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1560,
        .horizontal_active_px = 1400,
        .horizontal_blank_start_px = 1400,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1448,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1112,
        .vertical_active_lines = 1050,
        .vertical_blank_start_lines = 1050,
        .vertical_blank_lines = 62,
        .vertical_sync_start_lines = 1053,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 55,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x2E,  // Page 66

        .vertical_field_refresh_rate_millihertz = 59'901,
        .pixel_clock_khz = 88'750,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1600,
        .horizontal_active_px = 1440,
        .horizontal_blank_start_px = 1440,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1488,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 926,
        .vertical_active_lines = 900,
        .vertical_blank_start_lines = 900,
        .vertical_blank_lines = 26,
        .vertical_sync_start_lines = 903,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 17,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x2F,  // Page 67

        .vertical_field_refresh_rate_millihertz = 59'887,
        .pixel_clock_khz = 106'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1904,
        .horizontal_active_px = 1440,
        .horizontal_blank_start_px = 1440,
        .horizontal_blank_px = 464,
        .horitontal_sync_start_px = 1520,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 80,
        .horizontal_sync_width_px = 152,
        .horizontal_back_porch_px = 232,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 934,
        .vertical_active_lines = 900,
        .vertical_blank_start_lines = 900,
        .vertical_blank_lines = 34,
        .vertical_sync_start_lines = 903,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 25,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x30,  // Page 68

        .vertical_field_refresh_rate_millihertz = 74'984,
        .pixel_clock_khz = 136'750,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1936,
        .horizontal_active_px = 1440,
        .horizontal_blank_start_px = 1440,
        .horizontal_blank_px = 496,
        .horitontal_sync_start_px = 1536,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 96,
        .horizontal_sync_width_px = 152,
        .horizontal_back_porch_px = 248,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 942,
        .vertical_active_lines = 900,
        .vertical_blank_start_lines = 900,
        .vertical_blank_lines = 42,
        .vertical_sync_start_lines = 903,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 33,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x31,  // Page 69

        .vertical_field_refresh_rate_millihertz = 84'842,
        .pixel_clock_khz = 157'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 1952,
        .horizontal_active_px = 1440,
        .horizontal_blank_start_px = 1440,
        .horizontal_blank_px = 512,
        .horitontal_sync_start_px = 1544,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 104,
        .horizontal_sync_width_px = 152,
        .horizontal_back_porch_px = 256,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 948,
        .vertical_active_lines = 900,
        .vertical_blank_start_lines = 900,
        .vertical_blank_lines = 48,
        .vertical_sync_start_lines = 903,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 39,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x32,  // Page 70

        .vertical_field_refresh_rate_millihertz = 119'852,
        .pixel_clock_khz = 182'750,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1600,
        .horizontal_active_px = 1440,
        .horizontal_blank_start_px = 1440,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1488,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 953,
        .vertical_active_lines = 900,
        .vertical_blank_start_lines = 900,
        .vertical_blank_lines = 53,
        .vertical_sync_start_lines = 903,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 44,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x33,  // Page 72

        .vertical_field_refresh_rate_millihertz = 60'000,
        .pixel_clock_khz = 162'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2160,
        .horizontal_active_px = 1600,
        .horizontal_blank_start_px = 1600,
        .horizontal_blank_px = 560,
        .horitontal_sync_start_px = 1664,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 64,
        .horizontal_sync_width_px = 192,
        .horizontal_back_porch_px = 304,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1250,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 50,
        .vertical_sync_start_lines = 1201,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 46,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x34,  // Page 73

        .vertical_field_refresh_rate_millihertz = 65'000,
        .pixel_clock_khz = 175'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2160,
        .horizontal_active_px = 1600,
        .horizontal_blank_start_px = 1600,
        .horizontal_blank_px = 560,
        .horitontal_sync_start_px = 1664,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 64,
        .horizontal_sync_width_px = 192,
        .horizontal_back_porch_px = 304,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1250,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 50,
        .vertical_sync_start_lines = 1201,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 46,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x35,  // Page 74

        .vertical_field_refresh_rate_millihertz = 70'000,
        .pixel_clock_khz = 189'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2160,
        .horizontal_active_px = 1600,
        .horizontal_blank_start_px = 1600,
        .horizontal_blank_px = 560,
        .horitontal_sync_start_px = 1664,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 64,
        .horizontal_sync_width_px = 192,
        .horizontal_back_porch_px = 304,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1250,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 50,
        .vertical_sync_start_lines = 1201,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 46,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x36,  // Page 75

        .vertical_field_refresh_rate_millihertz = 75'000,
        .pixel_clock_khz = 202'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2160,
        .horizontal_active_px = 1600,
        .horizontal_blank_start_px = 1600,
        .horizontal_blank_px = 560,
        .horitontal_sync_start_px = 1664,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 64,
        .horizontal_sync_width_px = 192,
        .horizontal_back_porch_px = 304,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1250,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 50,
        .vertical_sync_start_lines = 1201,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 46,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x37,  // Page 76

        .vertical_field_refresh_rate_millihertz = 85'000,
        .pixel_clock_khz = 229'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2160,
        .horizontal_active_px = 1600,
        .horizontal_blank_start_px = 1600,
        .horizontal_blank_px = 560,
        .horitontal_sync_start_px = 1664,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 64,
        .horizontal_sync_width_px = 192,
        .horizontal_back_porch_px = 304,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1250,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 50,
        .vertical_sync_start_lines = 1201,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 46,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x38,  // Page 77

        .vertical_field_refresh_rate_millihertz = 119'917,
        .pixel_clock_khz = 268'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1760,
        .horizontal_active_px = 1600,
        .horizontal_blank_start_px = 1600,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1648,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1271,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 71,
        .vertical_sync_start_lines = 1203,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 64,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x39,  // Page 78

        .vertical_field_refresh_rate_millihertz = 59'883,
        .pixel_clock_khz = 119'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1840,
        .horizontal_active_px = 1680,
        .horizontal_blank_start_px = 1680,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1728,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1080,
        .vertical_active_lines = 1050,
        .vertical_blank_start_lines = 1050,
        .vertical_blank_lines = 30,
        .vertical_sync_start_lines = 1053,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 21,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x3A,  // Page 79

        .vertical_field_refresh_rate_millihertz = 59'954,
        .pixel_clock_khz = 146'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2240,
        .horizontal_active_px = 1680,
        .horizontal_blank_start_px = 1680,
        .horizontal_blank_px = 560,
        .horitontal_sync_start_px = 1784,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 104,
        .horizontal_sync_width_px = 176,
        .horizontal_back_porch_px = 280,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1089,
        .vertical_active_lines = 1050,
        .vertical_blank_start_lines = 1050,
        .vertical_blank_lines = 39,
        .vertical_sync_start_lines = 1053,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 30,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x3B,  // Page 80

        .vertical_field_refresh_rate_millihertz = 74'892,
        .pixel_clock_khz = 187'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2272,
        .horizontal_active_px = 1680,
        .horizontal_blank_start_px = 1680,
        .horizontal_blank_px = 592,
        .horitontal_sync_start_px = 1800,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 120,
        .horizontal_sync_width_px = 176,
        .horizontal_back_porch_px = 296,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1099,
        .vertical_active_lines = 1050,
        .vertical_blank_start_lines = 1050,
        .vertical_blank_lines = 49,
        .vertical_sync_start_lines = 1053,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 40,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x3C,  // Page 81

        .vertical_field_refresh_rate_millihertz = 84'941,
        .pixel_clock_khz = 214'750,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2288,
        .horizontal_active_px = 1680,
        .horizontal_blank_start_px = 1680,
        .horizontal_blank_px = 608,
        .horitontal_sync_start_px = 1808,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 128,
        .horizontal_sync_width_px = 176,
        .horizontal_back_porch_px = 304,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1105,
        .vertical_active_lines = 1050,
        .vertical_blank_start_lines = 1050,
        .vertical_blank_lines = 55,
        .vertical_sync_start_lines = 1053,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 46,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x3D,  // Page 82

        .vertical_field_refresh_rate_millihertz = 119'986,
        .pixel_clock_khz = 245'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1840,
        .horizontal_active_px = 1680,
        .horizontal_blank_start_px = 1680,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1728,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1112,
        .vertical_active_lines = 1050,
        .vertical_blank_start_lines = 1050,
        .vertical_blank_lines = 62,
        .vertical_sync_start_lines = 1053,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 53,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x3E,  // Page 83

        .vertical_field_refresh_rate_millihertz = 60'000,
        .pixel_clock_khz = 204'750,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2448,
        .horizontal_active_px = 1792,
        .horizontal_blank_start_px = 1792,
        .horizontal_blank_px = 656,
        .horitontal_sync_start_px = 1920,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 128,
        .horizontal_sync_width_px = 200,
        .horizontal_back_porch_px = 328,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1394,
        .vertical_active_lines = 1344,
        .vertical_blank_start_lines = 1344,
        .vertical_blank_lines = 50,
        .vertical_sync_start_lines = 1345,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 46,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x3F,  // Page 84

        .vertical_field_refresh_rate_millihertz = 74'997,
        .pixel_clock_khz = 261'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2456,
        .horizontal_active_px = 1792,
        .horizontal_blank_start_px = 1792,
        .horizontal_blank_px = 664,
        .horitontal_sync_start_px = 1888,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 96,
        .horizontal_sync_width_px = 216,
        .horizontal_back_porch_px = 352,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1417,
        .vertical_active_lines = 1344,
        .vertical_blank_start_lines = 1344,
        .vertical_blank_lines = 73,
        .vertical_sync_start_lines = 1345,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 69,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x40,  // Page 85

        .vertical_field_refresh_rate_millihertz = 119'974,
        .pixel_clock_khz = 333'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 1952,
        .horizontal_active_px = 1792,
        .horizontal_blank_start_px = 1792,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1840,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1423,
        .vertical_active_lines = 1344,
        .vertical_blank_start_lines = 1344,
        .vertical_blank_lines = 79,
        .vertical_sync_start_lines = 1347,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 72,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x41,  // Page 86

        .vertical_field_refresh_rate_millihertz = 59'995,
        .pixel_clock_khz = 218'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2528,
        .horizontal_active_px = 1856,
        .horizontal_blank_start_px = 1856,
        .horizontal_blank_px = 672,
        .horitontal_sync_start_px = 1952,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 96,
        .horizontal_sync_width_px = 224,
        .horizontal_back_porch_px = 352,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1439,
        .vertical_active_lines = 1392,
        .vertical_blank_start_lines = 1392,
        .vertical_blank_lines = 47,
        .vertical_sync_start_lines = 1393,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 43,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x42,  // Page 87

        .vertical_field_refresh_rate_millihertz = 75'000,
        .pixel_clock_khz = 288'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2560,
        .horizontal_active_px = 1856,
        .horizontal_blank_start_px = 1856,
        .horizontal_blank_px = 704,
        .horitontal_sync_start_px = 1984,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 128,
        .horizontal_sync_width_px = 224,
        .horizontal_back_porch_px = 352,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1500,
        .vertical_active_lines = 1392,
        .vertical_blank_start_lines = 1392,
        .vertical_blank_lines = 108,
        .vertical_sync_start_lines = 1393,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 104,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x43,  // Page 88

        .vertical_field_refresh_rate_millihertz = 119'970,
        .pixel_clock_khz = 356'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 2016,
        .horizontal_active_px = 1856,
        .horizontal_blank_start_px = 1856,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1904,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1474,
        .vertical_active_lines = 1392,
        .vertical_blank_start_lines = 1392,
        .vertical_blank_lines = 82,
        .vertical_sync_start_lines = 1395,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 75,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x44,  // Page 90

        .vertical_field_refresh_rate_millihertz = 59'950,
        .pixel_clock_khz = 154'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 2080,
        .horizontal_active_px = 1920,
        .horizontal_blank_start_px = 1920,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1968,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1235,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 35,
        .vertical_sync_start_lines = 1203,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 26,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x45,  // Page 91

        .vertical_field_refresh_rate_millihertz = 59'885,
        .pixel_clock_khz = 193'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2592,
        .horizontal_active_px = 1920,
        .horizontal_blank_start_px = 1920,
        .horizontal_blank_px = 672,
        .horitontal_sync_start_px = 2056,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 136,
        .horizontal_sync_width_px = 200,
        .horizontal_back_porch_px = 336,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1245,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 45,
        .vertical_sync_start_lines = 1203,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 36,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x46,  // Page 92

        .vertical_field_refresh_rate_millihertz = 74'930,
        .pixel_clock_khz = 245'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2608,
        .horizontal_active_px = 1920,
        .horizontal_blank_start_px = 1920,
        .horizontal_blank_px = 688,
        .horitontal_sync_start_px = 2056,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 136,
        .horizontal_sync_width_px = 208,
        .horizontal_back_porch_px = 344,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1255,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 55,
        .vertical_sync_start_lines = 1203,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 46,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x47,  // Page 93

        .vertical_field_refresh_rate_millihertz = 84'932,
        .pixel_clock_khz = 281'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2624,
        .horizontal_active_px = 1920,
        .horizontal_blank_start_px = 1920,
        .horizontal_blank_px = 704,
        .horitontal_sync_start_px = 2064,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 144,
        .horizontal_sync_width_px = 208,
        .horizontal_back_porch_px = 352,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1262,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 62,
        .vertical_sync_start_lines = 1203,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 53,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x48,  // Page 94

        .vertical_field_refresh_rate_millihertz = 119'909,
        .pixel_clock_khz = 317'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 2080,
        .horizontal_active_px = 1920,
        .horizontal_blank_start_px = 1920,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1968,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1271,
        .vertical_active_lines = 1200,
        .vertical_blank_start_lines = 1200,
        .vertical_blank_lines = 71,
        .vertical_sync_start_lines = 1203,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 62,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x49,  // Page 95

        .vertical_field_refresh_rate_millihertz = 60'000,
        .pixel_clock_khz = 234'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2600,
        .horizontal_active_px = 1920,
        .horizontal_blank_start_px = 1920,
        .horizontal_blank_px = 680,
        .horitontal_sync_start_px = 2048,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 128,
        .horizontal_sync_width_px = 208,
        .horizontal_back_porch_px = 344,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1500,
        .vertical_active_lines = 1440,
        .vertical_blank_start_lines = 1440,
        .vertical_blank_lines = 60,
        .vertical_sync_start_lines = 1441,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 56,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x4A,  // Page 96

        .vertical_field_refresh_rate_millihertz = 75'000,
        .pixel_clock_khz = 297'000,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 2640,
        .horizontal_active_px = 1920,
        .horizontal_blank_start_px = 1920,
        .horizontal_blank_px = 720,
        .horitontal_sync_start_px = 2064,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 144,
        .horizontal_sync_width_px = 224,
        .horizontal_back_porch_px = 352,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1500,
        .vertical_active_lines = 1440,
        .vertical_blank_start_lines = 1440,
        .vertical_blank_lines = 60,
        .vertical_sync_start_lines = 1441,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 1,
        .vertical_sync_width_lines = 3,
        .vertical_back_porch_lines = 56,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x4B,  // Page 97

        .vertical_field_refresh_rate_millihertz = 119'956,
        .pixel_clock_khz = 380'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 2080,
        .horizontal_active_px = 1920,
        .horizontal_blank_start_px = 1920,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 1968,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1525,
        .vertical_active_lines = 1440,
        .vertical_blank_start_lines = 1440,
        .vertical_blank_lines = 85,
        .vertical_sync_start_lines = 1443,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 4,
        .vertical_back_porch_lines = 78,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x4C,  // Page 99

        .vertical_field_refresh_rate_millihertz = 59'972,
        .pixel_clock_khz = 268'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 2720,
        .horizontal_active_px = 2560,
        .horizontal_blank_start_px = 2560,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 2608,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1646,
        .vertical_active_lines = 1600,
        .vertical_blank_start_lines = 1600,
        .vertical_blank_lines = 46,
        .vertical_sync_start_lines = 1603,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 37,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x4D,  // Page 100

        .vertical_field_refresh_rate_millihertz = 59'987,
        .pixel_clock_khz = 348'500,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 3504,
        .horizontal_active_px = 2560,
        .horizontal_blank_start_px = 2560,
        .horizontal_blank_px = 944,
        .horitontal_sync_start_px = 2752,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 192,
        .horizontal_sync_width_px = 280,
        .horizontal_back_porch_px = 472,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1658,
        .vertical_active_lines = 1600,
        .vertical_blank_start_lines = 1600,
        .vertical_blank_lines = 58,
        .vertical_sync_start_lines = 1603,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 49,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x4E,  // Page 101

        .vertical_field_refresh_rate_millihertz = 74'972,
        .pixel_clock_khz = 443'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 3536,
        .horizontal_active_px = 2560,
        .horizontal_blank_start_px = 2560,
        .horizontal_blank_px = 976,
        .horitontal_sync_start_px = 2768,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 208,
        .horizontal_sync_width_px = 280,
        .horizontal_back_porch_px = 488,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1672,
        .vertical_active_lines = 1600,
        .vertical_blank_start_lines = 1600,
        .vertical_blank_lines = 72,
        .vertical_sync_start_lines = 1603,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 63,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x4F,  // Page 102

        .vertical_field_refresh_rate_millihertz = 84'951,
        .pixel_clock_khz = 505'250,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kNegative,
        .vertical_sync_polarity = display::SyncPolarity::kPositive,

        .horizontal_total_px = 3536,
        .horizontal_active_px = 2560,
        .horizontal_blank_start_px = 2560,
        .horizontal_blank_px = 976,
        .horitontal_sync_start_px = 2768,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 208,
        .horizontal_sync_width_px = 280,
        .horizontal_back_porch_px = 488,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1682,
        .vertical_active_lines = 1600,
        .vertical_blank_start_lines = 1600,
        .vertical_blank_lines = 82,
        .vertical_sync_start_lines = 1603,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 73,
        .vertical_top_border_lines = 0,
    },
    DmtTiming{
        .id = 0x50,  // Page 103

        .vertical_field_refresh_rate_millihertz = 119'963,
        .pixel_clock_khz = 552'750,

        .fields_per_frame = display::FieldsPerFrame::kProgressive,
        .horizontal_sync_polarity = display::SyncPolarity::kPositive,
        .vertical_sync_polarity = display::SyncPolarity::kNegative,

        .horizontal_total_px = 2720,
        .horizontal_active_px = 2560,
        .horizontal_blank_start_px = 2560,
        .horizontal_blank_px = 160,
        .horitontal_sync_start_px = 2608,

        .horizontal_right_border_px = 0,
        .horizontal_front_porch_px = 48,
        .horizontal_sync_width_px = 32,
        .horizontal_back_porch_px = 80,
        .horizontal_left_border_px = 0,

        .vertical_total_lines = 1694,
        .vertical_active_lines = 1600,
        .vertical_blank_start_lines = 1600,
        .vertical_blank_lines = 94,
        .vertical_sync_start_lines = 1603,

        .vertical_bottom_border_lines = 0,
        .vertical_front_porch_lines = 3,
        .vertical_sync_width_lines = 6,
        .vertical_back_porch_lines = 85,
        .vertical_top_border_lines = 0,
    },
};
constexpr std::span<const DmtTiming> kDmtTimings(kDmtTimingArray);

}  // namespace edid::internal

#endif  // SRC_GRAPHICS_DISPLAY_LIB_EDID_DMT_TIMING_H_
