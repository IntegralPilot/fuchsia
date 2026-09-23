// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_GRAPHICS_DISPLAY_LIB_DEVICE_PROTOCOL_DISPLAY_INCLUDE_LIB_DEVICE_PROTOCOL_DISPLAY_PANEL_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DEVICE_PROTOCOL_DISPLAY_INCLUDE_LIB_DEVICE_PROTOCOL_DISPLAY_PANEL_H_

#include <zircon/types.h>

namespace display {

// Display devices supported by Fuchsia platform.
enum class PanelType : uint32_t {
  // Panel: BOE TV070WSM-TG1
  // DDIC: Fitipower JD9364
  // Board: Astro
  // Touch IC: FocalTech FT3x27 (Astro)
  kBoeTv070wsmFitipowerJd9364Astro = 0x00,

  // Panel: Innolux P070ACB-DB0
  // DDIC: Fitipower JD9364
  // Board: Astro
  // Touch IC: Goodix GT9293
  kInnoluxP070acbFitipowerJd9364 = 0x01,

  // Panel: BOE TV101WXM-AG0
  // DDIC: Fitipower JD9364
  // Board: Sherlock
  // Touch IC: FocalTech FT5726
  kBoeTv101wxmFitipowerJd9364 = 0x02,

  // Panel: Innolux P101DEZ-DD0
  // DDIC: Fitipower JD9364
  // Board: Sherlock
  // Touch IC: FocalTech FT5726
  kInnoluxP101dezFitipowerJd9364 = 0x03,

  // 0x04 was for kIli9881c
  // 0x05 was for kSt7701s
  // 0x06 was for kTv080wxmFt

  // Panel: BOE TV101WXM-AG0
  // DDIC: Fitipower JD9365
  // Board: Sherlock
  // Touch IC: FocalTech FT5726
  kBoeTv101wxmFitipowerJd9365 = 0x07,

  // Panel: BOE TV070WSM-TG1
  // DDIC: Fitipower JD9365
  // Board: Nelson
  // Touch IC: Goodix GT6853 (Nelson)
  kBoeTv070wsmFitipowerJd9365 = 0x08,

  // Panel: K&D KD070D82-39TI-A010
  // DDIC: Fitipower JD9364
  // Board: Nelson
  // Touch IC: Goodix GT6853
  kKdKd070d82FitipowerJd9364 = 0x09,

  // Panel: K&D KD070D82-39TI-A010
  // DDIC: Fitipower JD9365
  // Board: Nelson
  // Touch IC: Goodix GT6853
  kKdKd070d82FitipowerJd9365 = 0x0a,

  // 0x0b was for kTv070wsmSt7703i

  // Khadas TS-050
  // Panel: Microtech MTF050FHDI-03
  // DDIC: Novatek NT35596
  // Board: VIM3
  // Touch IC: FocalTech FT5336
  kMicrotechMtf050fhdi03NovatekNt35596 = 0x0c,

  // Panel: BOE TV070WSM-TG1
  // DDIC: Fitipower JD9364
  // Board: Nelson
  // Touch IC: Goodix GT6853 (Nelson)
  kBoeTv070wsmFitipowerJd9364Nelson = 0x0d,

  // TODO(https://fxbug.dev/425473640): Remove `kUnknown`.
  kUnknown = 0xff,
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DEVICE_PROTOCOL_DISPLAY_INCLUDE_LIB_DEVICE_PROTOCOL_DISPLAY_PANEL_H_
