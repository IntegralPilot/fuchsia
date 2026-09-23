// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lights-cli.h"

#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>

#include <fbl/unique_fd.h>

const char* capabilities[3] = {"Brightness", "Rgb", "Simple"};

zx_status_t LightsCli::PrintValue(uint32_t idx) {
  auto result = client_->GetInfo(idx);
  if ((result.status() != ZX_OK) || result.value().is_error()) {
    printf("Could not get info\n");
    return std::min(result.status(), ZX_ERR_INTERNAL);
  }

  const char* const name = result->value()->info.name.begin();

  std::optional<double> brightness;
  if (const auto result = client_->GetCurrentBrightnessValue(idx); result.ok() && result->is_ok()) {
    brightness.emplace(result->value()->value);
  }

  std::optional<fuchsia_hardware_light::wire::Rgb> rgb;
  if (const auto result = client_->GetCurrentRgbValue(idx); result.ok() && result->is_ok()) {
    rgb.emplace(result->value()->value);
  }

  if (brightness && rgb) {
    printf("Value of %s: Brightness %f RGB %f %f %f\n", name, *brightness, rgb->red, rgb->green,
           rgb->blue);
  } else if (brightness) {
    printf("Value of %s: Brightness %f\n", name, *brightness);
  } else if (rgb) {
    printf("Value of %s: RGB %f %f %f\n", name, rgb->red, rgb->green, rgb->blue);
  } else {
    printf("Could not get brightness or RGB for light number %u\n", idx);
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

int LightsCli::SetBrightness(uint32_t idx, double brightness) {
  auto result = client_->SetBrightnessValue(idx, brightness);
  if ((result.status() != ZX_OK) || result.value().is_error()) {
    printf("Could not set brightness\n");
    return std::min(result.status(), ZX_ERR_INTERNAL);
  }

  return ZX_OK;
}

zx_status_t LightsCli::SetRgb(uint32_t idx, double red, double green, double blue) {
  const fuchsia_hardware_light::wire::Rgb rgb{
      .red = red,
      .green = green,
      .blue = blue,
  };

  auto result = client_->SetRgbValue(idx, rgb);
  if ((result.status() != ZX_OK) || result.value().is_error()) {
    printf("Could not set RGB\n");
    return std::min(result.status(), ZX_ERR_INTERNAL);
  }
  return ZX_OK;
}

zx_status_t LightsCli::Summary() {
  auto result1 = client_->GetNumLights();
  if (result1.status() != ZX_OK) {
    printf("Could not get count\n");
    return result1.status();
  }

  printf("Total %u lights\n", result1.value().count);
  for (uint32_t i = 0; i < result1.value().count; i++) {
    auto result2 = client_->GetInfo(i);
    if ((result2.status() != ZX_OK) || result2.value().is_error()) {
      printf("Could not get capability for light number %u. Skipping.\n", i);
      continue;
    }

    switch (result2->value()->info.capability) {
      case fuchsia_hardware_light::wire::Capability::kBrightness:
      case fuchsia_hardware_light::wire::Capability::kRgb:
        PrintValue(i);
        break;
      case fuchsia_hardware_light::wire::Capability::kSimple:
        break;
      default:
        printf("Unknown capability %u for light number %u.\n",
               static_cast<unsigned int>(result2->value()->info.capability), i);
        continue;
    };

    printf("    Capabilities: %s\n",
           capabilities[static_cast<uint8_t>(result2->value()->info.capability) - 1]);
  }

  return ZX_OK;
}
