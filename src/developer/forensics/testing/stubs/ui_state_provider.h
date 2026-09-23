// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_UI_STATE_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_UI_STATE_PROVIDER_H_

#include <fidl/fuchsia.ui.activity/cpp/fidl.h>
#include <fidl/fuchsia.ui.activity/cpp/test_base.h>

#include "src/developer/forensics/testing/stubs/fidl_server.h"
#include "src/developer/forensics/utils/fidl_event_handler.h"

namespace forensics::stubs {

using UIStateProviderBase = SingleBindingFidlServer<fuchsia_ui_activity::Provider>;

class UIStateProvider : public UIStateProviderBase {
 public:
  UIStateProvider(async_dispatcher_t* dispatcher, fuchsia_ui_activity::State state,
                  zx::time_monotonic time);

  void WatchState(WatchStateRequest& request, WatchStateCompleter::Sync& completer) override;
  void SetState(fuchsia_ui_activity::State state, zx::time_monotonic time);
  void UnbindListener();

 private:
  void OnStateChanged();

  async_dispatcher_t* dispatcher_;
  fuchsia_ui_activity::State state_;
  zx::time_monotonic time_;
  std::optional<fidl::Client<fuchsia_ui_activity::Listener>> listener_;
  AsyncEventHandlerClosed<fuchsia_ui_activity::Listener> event_handler_;
};

}  // namespace forensics::stubs

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_UI_STATE_PROVIDER_H_
