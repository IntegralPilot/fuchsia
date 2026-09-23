// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/fakes/crash_introspect.h"

namespace forensics::fakes {

void CrashIntrospect::FindComponentByThreadKoid(
    FindComponentByThreadKoidRequest& request,
    FindComponentByThreadKoidCompleter::Sync& completer) {
  fuchsia_sys2::ComponentCrashInfo info;
  info.moniker("test-moniker");
  info.url("test-url");

  completer.Reply(fit::ok(info));
}

void CrashIntrospect::FindDriverCrash(FindDriverCrashRequest& request,
                                      FindDriverCrashCompleter::Sync& completer) {
  fuchsia_driver_crash::DriverCrashInfo info;
  info.node_moniker("test-moniker");
  info.url("test-url");

  completer.Reply(fit::ok(info));
}

}  // namespace forensics::fakes
