// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/adb/bin/adb/state-controller.h"

#include "src/developer/adb/third_party/adb/adb-protocol.h"

namespace adb {

void StateControllerServer::SetSystemType(SetSystemTypeRequest& request,
                                          SetSystemTypeCompleter::Sync& completer) {
  ConnectionState new_state;
  if (request.system_type() == fuchsia_hardware_adb::SystemType::kSideload) {
    new_state = kCsSideload;
  } else if (request.system_type() == fuchsia_hardware_adb::SystemType::kRecovery) {
    new_state = kCsRecovery;
  } else {
    new_state = kCsDevice;
  }

  if (get_system_type() != new_state) {
    set_system_type(new_state);
    if (reset_callback_) {
      reset_callback_();
    }
  }
  completer.Reply(fit::ok());
}

void StateControllerServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_adb::StateController> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

}  // namespace adb
