// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_VIRTUALIZATION_LIB_GUEST_INTERACTION_INTERACTIVE_GUEST_INTERACTIVE_GUEST_IMPL_H_
#define SRC_VIRTUALIZATION_LIB_GUEST_INTERACTION_INTERACTIVE_GUEST_INTERACTIVE_GUEST_IMPL_H_

#include <fidl/fuchsia.virtualization.guest.interaction/cpp/fidl.h>
#include <fidl/fuchsia.virtualization/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>

#include <optional>

#include "src/virtualization/lib/guest_interaction/interactive_guest/running_guest.h"

namespace interactive_guest {

class InteractiveGuestImpl
    : public fidl::Server<fuchsia_virtualization_guest_interaction::InteractiveGuest> {
 public:
  explicit InteractiveGuestImpl(async::Loop& loop);
  ~InteractiveGuestImpl();

  // Implements `fuchsia_virtualization_guest_interaction::InteractiveGuest`.
  void Start(StartRequest& request, StartCompleter::Sync& completer) override;
  // Implements `fuchsia_virtualization_guest_interaction::InteractiveGuest`.
  void Shutdown(ShutdownCompleter::Sync& completer) override;
  // Implements `fuchsia_virtualization::Interaction`.
  void PutFile(PutFileRequest& request, PutFileCompleter::Sync& completer) override;
  // Implements `fuchsia_virtualization::Interaction`.
  void GetFile(GetFileRequest& request, GetFileCompleter::Sync& completer) override;
  // Implements `fuchsia_virtualization::Interaction`.
  void ExecuteCommand(ExecuteCommandRequest& request,
                      ExecuteCommandCompleter::Sync& completer) override;

 private:
  void DoShutdown();

  async::Loop& loop_;
  fidl::SyncClient<fuchsia_virtualization::GuestManager> guest_manager_sync_;
  std::optional<RunningGuest> running_guest_;
};

}  // namespace interactive_guest

#endif  // SRC_VIRTUALIZATION_LIB_GUEST_INTERACTION_INTERACTIVE_GUEST_INTERACTIVE_GUEST_IMPL_H_
