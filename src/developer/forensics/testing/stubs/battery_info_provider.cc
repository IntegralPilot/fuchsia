// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/stubs/battery_info_provider.h"

#include <lib/syslog/cpp/macros.h>

namespace forensics::stubs {

void StubBatteryInfoProvider::GetBatteryInfo(GetBatteryInfoCompleter::Sync& completer) {
  completer.Reply(info_);
}

void StubBatteryInfoProvider::Watch(WatchRequest& request, WatchCompleter::Sync& completer) {
  FX_NOTIMPLEMENTED() << "BatteryManager::Watch is not implemented";
}

}  // namespace forensics::stubs
