// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIGHT_BIN_LIGHTS_CLI_LIGHTS_CLI_H_
#define SRC_UI_LIGHT_BIN_LIGHTS_CLI_LIGHTS_CLI_H_

#include <fidl/fuchsia.hardware.light/cpp/wire.h>

#include <memory>

class LightsCli {
 public:
  explicit LightsCli(fidl::ClientEnd<fuchsia_hardware_light::Light> client_end)
      : client_(std::move(client_end)) {}
  zx_status_t PrintValue(uint32_t idx);
  zx_status_t SetBrightness(uint32_t idx, double brightness);
  zx_status_t SetRgb(uint32_t idx, double red, double green, double blue);
  zx_status_t Summary();

 private:
  fidl::WireSyncClient<fuchsia_hardware_light::Light> client_;
};

#endif  // SRC_UI_LIGHT_BIN_LIGHTS_CLI_LIGHTS_CLI_H_
