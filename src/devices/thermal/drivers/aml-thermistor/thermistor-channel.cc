// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "thermistor-channel.h"

#include <lib/ddk/debug.h>

#include <ddktl/fidl.h>

namespace thermal {

void ThermistorChannel::GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) {
  float norm;
  {
    auto result = adc_->GetNormalizedSample();
    if (!result.ok()) {
      zxlogf(ERROR, "GetNormalizedSample failed %s", result.FormatDescription().c_str());
      completer.Reply(ZX_ERR_INTERNAL, 0.0f);
      return;
    }
    if (result->is_error()) {
      zxlogf(ERROR, "GetNormalizedSample failed %d", result->error_value());
      completer.Reply(result->error_value(), 0.0f);
      return;
    }
    norm = static_cast<float>(result->value()->value);
  }

  float temperature;
  auto status = ntc_.GetTemperatureCelsius(norm, &temperature);
  completer.Reply(status, temperature);
}

void ThermistorChannel::GetSensorName(GetSensorNameCompleter::Sync& completer) {
  completer.Reply(fidl::StringView::FromExternal(name_));
}

}  // namespace thermal
