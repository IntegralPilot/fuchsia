// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/flatland_factory.h"

#include <lib/syslog/cpp/macros.h>

namespace flatland {

FlatlandFactoryImpl::FlatlandFactoryImpl(std::shared_ptr<FlatlandManager> flatland_manager)
    : flatland_manager_(std::move(flatland_manager)) {
  FX_CHECK(flatland_manager_);
}

// static
bool FlatlandFactoryImpl::IsValidConfig(const fuchsia_ui_composition::FlatlandConfig& config) {
  // Currently all FlatlandConfig table fields are valid (use_flatland2 is an optional bool).
  // Validation logic for future table fields can be added here as new parameters are introduced.
  return true;
}

// static
FlatlandConfig FlatlandFactoryImpl::ToInternalConfig(
    const fuchsia_ui_composition::FlatlandConfig& config) {
  const bool use_flatland2 = config.use_flatland2().value_or(false);
  return FlatlandConfig{
      // Flatland2 deprecates OnFramePresented in favor of Zircon frame presentation fencing.
      .skips_on_frame_presented = use_flatland2,
      .use_flatland2 = use_flatland2,
  };
}

void FlatlandFactoryImpl::CreateFlatland(CreateFlatlandRequest& request,
                                         CreateFlatlandCompleter::Sync& completer) {
  if (!IsValidConfig(request.config())) {
    FX_LOGS(WARNING) << "CreateFlatland called with invalid config.";
    completer.Reply(fit::error(fuchsia_ui_composition::FlatlandFactoryError::kInvalidArgs));
    return;
  }

  std::optional<scheduling::SessionId> session_id = flatland_manager_->CreateFlatland(
      std::move(request.server_end()), ToInternalConfig(request.config()));
  if (!session_id.has_value()) {
    completer.Reply(fit::error(fuchsia_ui_composition::FlatlandFactoryError::kInternal));
    return;
  }
  completer.Reply(fit::ok());
}

}  // namespace flatland
