// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "light_stress.h"

#include <fidl/fuchsia.hardware.light/cpp/wire.h>
#include <fuchsia/hardware/light/cpp/fidl.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include "args.h"
#include "status.h"
#include "util.h"

namespace hwstress {

namespace {

zx_status_t LightErrorToZxStatus(fuchsia::hardware::light::LightError error) {
  switch (error) {
    case fuchsia::hardware::light::LightError::OK:
      return ZX_ERR_INTERNAL;
    case fuchsia::hardware::light::LightError::INVALID_INDEX:
      return ZX_ERR_OUT_OF_RANGE;
    case fuchsia::hardware::light::LightError::NOT_SUPPORTED:
      return ZX_ERR_NOT_SUPPORTED;
    case fuchsia::hardware::light::LightError::FAILED:
      return ZX_ERR_IO;
    default:
      return ZX_ERR_INTERNAL;
  }
}

}  // namespace

bool operator==(const LightInfo& a, const LightInfo& b) {
  return a.name == b.name && a.index == b.index && a.capability == b.capability;
}
bool operator!=(const LightInfo& a, const LightInfo& b) { return !(a == b); }

zx::result<std::vector<LightInfo>> GetLights(const fuchsia::hardware::light::LightSyncPtr& light) {
  uint32_t count;
  zx_status_t status = light->GetNumLights(&count);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  std::vector<LightInfo> lights;
  lights.reserve(count);
  for (uint32_t i = 0; i < count; i++) {
    fuchsia::hardware::light::Light_GetInfo_Result result;
    status = light->GetInfo(i, &result);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    if (result.is_err()) {
      return zx::error(LightErrorToZxStatus(result.err()));
    }

    const fuchsia::hardware::light::Info& info = result.response().info;
    if (info.capability == fuchsia::hardware::light::Capability::BRIGHTNESS ||
        info.capability == fuchsia::hardware::light::Capability::RGB ||
        info.capability == fuchsia::hardware::light::Capability::SIMPLE) {
      lights.push_back(LightInfo{
          .name = info.name,
          .index = i,
          .capability = info.capability,
      });
    } else {
      fprintf(stderr, "Light %u '%s' is unsupported.\n", i, info.name.c_str());
    }
  }

  return zx::ok(std::move(lights));
}

zx::result<> TurnOnLight(const fuchsia::hardware::light::LightSyncPtr& light,
                         const LightInfo& info) {
  switch (info.capability) {
    case fuchsia::hardware::light::Capability::SIMPLE: {
      fuchsia::hardware::light::Light_SetSimpleValue_Result result;
      zx_status_t status = light->SetSimpleValue(info.index, true, &result);
      if (status != ZX_OK) {
        return zx::error(status);
      }
      if (result.is_err()) {
        return zx::error(LightErrorToZxStatus(result.err()));
      }
      break;
    }
    case fuchsia::hardware::light::Capability::BRIGHTNESS: {
      fuchsia::hardware::light::Light_SetBrightnessValue_Result result;
      zx_status_t status = light->SetBrightnessValue(info.index, 1.0, &result);
      if (status != ZX_OK) {
        return zx::error(status);
      }
      if (result.is_err()) {
        return zx::error(LightErrorToZxStatus(result.err()));
      }
      break;
    }
    case fuchsia::hardware::light::Capability::RGB: {
      fuchsia::hardware::light::Light_SetRgbValue_Result result;
      fuchsia::hardware::light::Rgb rgb{.red = 1.0, .green = 1.0, .blue = 1.0};
      zx_status_t status = light->SetRgbValue(info.index, rgb, &result);
      if (status != ZX_OK) {
        return zx::error(status);
      }
      if (result.is_err()) {
        return zx::error(LightErrorToZxStatus(result.err()));
      }
      break;
    }
    default:
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok();
}

zx::result<> TurnOffLight(const fuchsia::hardware::light::LightSyncPtr& light,
                          const LightInfo& info) {
  switch (info.capability) {
    case fuchsia::hardware::light::Capability::SIMPLE: {
      fuchsia::hardware::light::Light_SetSimpleValue_Result result;
      zx_status_t status = light->SetSimpleValue(info.index, false, &result);
      if (status != ZX_OK) {
        return zx::error(status);
      }
      if (result.is_err()) {
        return zx::error(LightErrorToZxStatus(result.err()));
      }
      break;
    }
    case fuchsia::hardware::light::Capability::BRIGHTNESS: {
      fuchsia::hardware::light::Light_SetBrightnessValue_Result result;
      zx_status_t status = light->SetBrightnessValue(info.index, 0.0, &result);
      if (status != ZX_OK) {
        return zx::error(status);
      }
      if (result.is_err()) {
        return zx::error(LightErrorToZxStatus(result.err()));
      }
      break;
    }
    case fuchsia::hardware::light::Capability::RGB: {
      fuchsia::hardware::light::Light_SetRgbValue_Result result;
      fuchsia::hardware::light::Rgb rgb{.red = 0.0, .green = 0.0, .blue = 0.0};
      zx_status_t status = light->SetRgbValue(info.index, rgb, &result);
      if (status != ZX_OK) {
        return zx::error(status);
      }
      if (result.is_err()) {
        return zx::error(LightErrorToZxStatus(result.err()));
      }
      break;
    }
    default:
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok();
}

bool StressLight(StatusLine* status, const CommandLineArgs& args, zx::duration duration) {
  component::SyncServiceMemberWatcher<fuchsia_hardware_light::LightService::Light> watcher;
  auto client_end = watcher.GetNextInstance(/*stop_at_idle = */ true);

  if (client_end.is_error()) {
    status->Log("Could not open device: %s\n", client_end.status_string());
    return false;
  }
  fuchsia::hardware::light::LightSyncPtr light_dev{};
  light_dev.Bind(client_end.value().TakeChannel());
  // Fetch information about the lights.
  zx::result<std::vector<LightInfo>> lights_or = GetLights(light_dev);
  if (lights_or.is_error()) {
    status->Log("Could not query lights: %s\n", lights_or.status_string());
    return false;
  }
  std::vector<LightInfo> lights = std::move(lights_or).value();

  // If there are no lights, abort.
  if (lights.empty()) {
    status->Log("No supported lights found.");
    return false;
  }

  // Print out information about lights.
  status->Log("Found %zu light(s):", lights.size());
  for (const LightInfo& light : lights) {
    status->Log("  %s (%u)", light.name.c_str(), light.index);
  }

  // Turn lights on and off until time runs out.
  zx::time start_time = zx::clock::get_monotonic();
  zx::time end_time = start_time + duration;
  while (zx::clock::get_monotonic() < end_time) {
    // Turn lights on.
    for (const LightInfo& light : lights) {
      zx::result<> result = TurnOnLight(light_dev, light);
      if (result.is_error()) {
        status->Log("Could not turn on light %u '%s': %s", light.index, light.name.c_str(),
                    result.status_string());
      }
    }

    zx::nanosleep(zx::deadline_after(SecsToDuration(args.light_on_time_seconds)));

    // Turn all lights off.
    for (const LightInfo& light : lights) {
      zx::result<> result = TurnOffLight(light_dev, light);
      if (result.is_error()) {
        status->Log("Could not turn off light %u '%s': %s", light.index, light.name.c_str(),
                    result.status_string());
      }
    }

    zx::nanosleep(zx::deadline_after(SecsToDuration(args.light_off_time_seconds)));
  }

  return true;
}

}  // namespace hwstress
