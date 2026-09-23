// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_FIDL_EVENT_HANDLER_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_FIDL_EVENT_HANDLER_H_

#include <lib/fidl/cpp/unified_messaging.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/syslog/cpp/macros.h>

namespace forensics {

// An asynchronous event handler for open or ajar protocols. A closed protocol does not need to
// define handle_unknown_event.
template <typename Protocol>
class AsyncEventHandlerOpen : public fidl::AsyncEventHandler<Protocol> {
 public:
  void on_fidl_error(const fidl::UnbindInfo error) override { FX_LOGS(ERROR) << error; }

  void handle_unknown_event(const fidl::UnknownEventMetadata<Protocol> metadata) override {
    FX_LOGS(ERROR) << "Unexpected event ordinal: " << metadata.event_ordinal;
  }
};

// An asynchronous event handler for closed protocols.
template <typename Protocol>
class AsyncEventHandlerClosed : public fidl::AsyncEventHandler<Protocol> {
 public:
  void on_fidl_error(const fidl::UnbindInfo error) override { FX_LOGS(ERROR) << error; }
};

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_FIDL_EVENT_HANDLER_H_
