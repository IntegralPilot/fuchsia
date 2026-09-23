// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/build_info/testing/build_info.h"

#include <fidl/fuchsia.buildinfo.test/cpp/fidl.h>
#include <zircon/availability.h>

void FakeProviderImpl::GetBuildInfo(GetBuildInfoCompleter::Sync& completer) {
  fuchsia_buildinfo::BuildInfo build_info;
  if (info_ref_->product_config_.empty()) {
    info_ref_->product_config_ = FakeProviderImpl::kProductNameDefault;
  }
  build_info.product_config(info_ref_->product_config_);

  if (info_ref_->board_config_.empty()) {
    info_ref_->board_config_ = FakeProviderImpl::kBoardNameDefault;
  }
  build_info.board_config(info_ref_->board_config_);

  if (info_ref_->version_.empty()) {
    info_ref_->version_ = FakeProviderImpl::kVersionDefault;
  }
  build_info.version(info_ref_->version_);

  if (info_ref_->latest_commit_date_.empty()) {
    info_ref_->latest_commit_date_ = FakeProviderImpl::kLastCommitDateDefault;
  }
  build_info.latest_commit_date(info_ref_->latest_commit_date_);

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (info_ref_->platform_version_.empty()) {
    info_ref_->platform_version_ = FakeProviderImpl::kPlatformVersionDefault;
  }
  build_info.platform_version(info_ref_->platform_version_);

  if (info_ref_->product_version_.empty()) {
    info_ref_->product_version_ = FakeProviderImpl::kProductVersionDefault;
  }
  build_info.product_version(info_ref_->product_version_);
#endif

  completer.Reply({{.build_info = std::move(build_info)}});
}

void BuildInfoTestControllerImpl::SetBuildInfo(SetBuildInfoRequest& request,
                                               SetBuildInfoCompleter::Sync& completer) {
  auto& build_info = request.build_info();
  info_ref_->product_config_ = build_info.product_config().value_or("");
  info_ref_->board_config_ = build_info.board_config().value_or("");
  info_ref_->version_ = build_info.version().value_or("");
  info_ref_->latest_commit_date_ = build_info.latest_commit_date().value_or("");

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  info_ref_->platform_version_ = build_info.platform_version().value_or("");
  info_ref_->product_version_ = build_info.product_version().value_or("");
#endif

  completer.Reply();
}
