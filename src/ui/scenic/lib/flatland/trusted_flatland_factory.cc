// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/trusted_flatland_factory.h"

#include <lib/syslog/cpp/macros.h>

namespace flatland {

TrustedFlatlandFactoryImpl::TrustedFlatlandFactoryImpl(
    std::shared_ptr<FlatlandManager> flatland_manager)
    : flatland_manager_(std::move(flatland_manager)) {}

// static
bool TrustedFlatlandFactoryImpl::IsValidConfig(
    const fuchsia_ui_composition::TrustedFlatlandConfig& config) {
  // Currently all TrustedFlatlandConfig table fields are valid.
  // Validation logic for field combinations can be added here as needed.
  return true;
}

// static
FlatlandConfig TrustedFlatlandFactoryImpl::ToInternalConfig(
    const fuchsia_ui_composition::TrustedFlatlandConfig& config) {
  return FlatlandConfig{
      .schedule_asap = config.schedule_asap().value_or(false),
      .pass_acquire_fences = config.pass_acquire_fences().value_or(false),
      .skips_present_credits = config.skips_present_credits().value_or(false),
      .skips_on_frame_presented = config.skips_on_frame_presented().value_or(false),
      .use_flatland2 = config.use_flatland2_api().value_or(false),
      .use_trusted_flatland_api = true,
  };
}

void TrustedFlatlandFactoryImpl::CreateFlatland(CreateFlatlandRequest& request,
                                                CreateFlatlandCompleter::Sync& completer) {
  if (!IsValidConfig(request.config())) {
    FX_LOGS(WARNING) << "CreateFlatland called with invalid config.";
    completer.Reply(fit::error(fuchsia_ui_composition::TrustedFlatlandFactoryError::kBadOperation));
    return;
  }

  std::optional<scheduling::SessionId> session_id = flatland_manager_->CreateFlatland(
      std::move(request.server_end()), ToInternalConfig(request.config()));
  if (!session_id.has_value()) {
    completer.Reply(fit::error(fuchsia_ui_composition::TrustedFlatlandFactoryError::kBadOperation));
    return;
  }
  completer.Reply(fit::ok());
}

}  // namespace flatland