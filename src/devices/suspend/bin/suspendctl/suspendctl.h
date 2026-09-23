// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SUSPEND_BIN_SUSPENDCTL_SUSPENDCTL_H_
#define SRC_DEVICES_SUSPEND_BIN_SUSPENDCTL_SUSPENDCTL_H_

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.power.suspend/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power.suspend/cpp/natural_ostream.h>
#include <fidl/fuchsia.hardware.power.suspend/cpp/natural_types.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>

#include <ostream>
#include <string>
#include <vector>

namespace suspendctl {

namespace fh_suspend = fuchsia_hardware_power_suspend;

enum class Action {
  Suspend,
  PrintHelp,
  Error,
};

Action ParseArgs(const std::vector<std::string>& argv);
int DoSuspend(fidl::SyncClient<fh_suspend::Suspender> client, std::ostream& out, std::ostream& err);
int run(int argc, char* argv[]);

}  // namespace suspendctl

#endif  // SRC_DEVICES_SUSPEND_BIN_SUSPENDCTL_SUSPENDCTL_H_
