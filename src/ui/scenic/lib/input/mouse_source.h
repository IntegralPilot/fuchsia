// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_MOUSE_SOURCE_H_
#define SRC_UI_SCENIC_LIB_INPUT_MOUSE_SOURCE_H_

#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/fit/function.h>

#include "src/ui/scenic/lib/input/mouse_source_base.h"

namespace scenic_impl::input {

// Implementation of the |fidl::Server<fuchsia_ui_pointer::MouseSource>| interface. One instance per
// channel.
class MouseSource : public MouseSourceBase, public fidl::Server<fuchsia_ui_pointer::MouseSource> {
 public:
  MouseSource(fidl::ServerEnd<fuchsia_ui_pointer::MouseSource> event_provider,
              fit::function<void()> error_handler,
              async_dispatcher_t* dispatcher = async_get_default_dispatcher());

  ~MouseSource() override = default;

  // |fidl::Server<fuchsia_ui_pointer::MouseSource>|
  void Watch(WatchCompleter::Sync& completer) override {
    MouseSourceBase::WatchBase([completer = completer.ToAsync()](
                                   std::vector<fuchsia_ui_pointer::MouseEvent> events) mutable {
      completer.Reply({{.events = std::move(events)}});
    });
  }

 private:
  void CloseChannel(zx_status_t epitaph);

  fidl::ServerBinding<fuchsia_ui_pointer::MouseSource> binding_;
};

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_MOUSE_SOURCE_H_
