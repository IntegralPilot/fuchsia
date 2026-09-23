// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/pressure_signaler/debugger.h"

#include <lib/syslog/cpp/macros.h>

namespace pressure_signaler {

MemoryDebugger::MemoryDebugger(PressureNotifier* notifier) : notifier_(notifier) {
  FX_CHECK(notifier_);
}

void MemoryDebugger::Signal(SignalRequest& request, SignalCompleter::Sync& completer) {
  notifier_->DebugNotify(request.level());
}

}  // namespace pressure_signaler
