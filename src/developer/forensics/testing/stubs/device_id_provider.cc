// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/stubs/device_id_provider.h"

#include <lib/syslog/cpp/macros.h>

namespace forensics {
namespace stubs {

void DeviceIdProviderBase::GetId(GetIdCompleter::Sync& completer) { GetIdInternal(completer); }

void DeviceIdProviderBase::GetIdInternal(GetIdCompleter::Sync& completer) {
  completer_ = completer.ToAsync();
  if (!dirty_) {
    dirty_ = true;
  } else {
    FX_CHECK(device_id_.has_value());
    completer_->Reply(device_id_.value());
    completer_ = std::nullopt;
    dirty_ = false;
  }
}

void DeviceIdProviderBase::SetDeviceId(std::string device_id) {
  device_id_ = std::move(device_id);
  if (dirty_ && completer_.has_value()) {
    completer_->Reply(device_id_.value());
    completer_ = std::nullopt;
  }
  dirty_ = false;
}

void DeviceIdProviderReturnsError::GetId(GetIdCompleter::Sync& completer) {
  completer.Close(error_);
}

}  // namespace stubs
}  // namespace forensics
