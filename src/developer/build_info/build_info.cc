// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "build_info.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/io.h>
#include <lib/syslog/cpp/macros.h>

#include <fbl/unique_fd.h>

#include "src/lib/files/file.h"
#include "zircon/status.h"

namespace {
const char kProductFilePath[] = "/config/build-info/product";
const char kBoardFilePath[] = "/config/build-info/board";
const char kVersionFilePath[] = "/config/build-info/version";
const char kPlatformVersionFilePath[] = "/config/build-info/platform_version";
const char kProductVersionFilePath[] = "/config/build-info/product_version";
const char kLatestCommitDateFilePath[] = "/config/build-info/latest-commit-date";

// Returns the contents of |file_path| with any trailing whitespace removed.
std::string ContentsOfFileAtPath(const std::string& file_path) {
  std::string file_contents;
  if (!files::ReadFileToString(file_path, &file_contents)) {
    FX_LOGS(ERROR) << "Error reading " << file_path;
  }

  // Trim trailing whitespace.
  const std::string whitespace = " \n\r\t";
  size_t end = file_contents.find_last_not_of(whitespace);

  return (end == std::string::npos) ? "" : file_contents.substr(0, end + 1);
}

}  // namespace

void ProviderImpl::GetBuildInfo(GetBuildInfoCompleter::Sync& completer) {
  fuchsia_buildinfo::BuildInfo build_info;

  if (!product_config_) {
    product_config_ = std::make_unique<std::string>(ContentsOfFileAtPath(kProductFilePath));
  }

  if (!product_config_->empty()) {
    build_info.product_config(*product_config_);
  }

  if (!board_config_) {
    board_config_ = std::make_unique<std::string>(ContentsOfFileAtPath(kBoardFilePath));
  }

  if (!board_config_->empty()) {
    build_info.board_config(*board_config_);
  }

  if (!version_) {
    version_ = std::make_unique<std::string>(ContentsOfFileAtPath(kVersionFilePath));
  }

  if (!version_->empty()) {
    build_info.version(*version_);
  }

  if (!platform_version_) {
    platform_version_ =
        std::make_unique<std::string>(ContentsOfFileAtPath(kPlatformVersionFilePath));
  }

  if (!platform_version_->empty()) {
    build_info.platform_version(*platform_version_);
  }

  if (!product_version_) {
    product_version_ = std::make_unique<std::string>(ContentsOfFileAtPath(kProductVersionFilePath));
  }

  if (!product_version_->empty()) {
    build_info.product_version(*product_version_);
  }

  if (!latest_commit_date_) {
    latest_commit_date_ =
        std::make_unique<std::string>(ContentsOfFileAtPath(kLatestCommitDateFilePath));
  }

  if (!latest_commit_date_->empty()) {
    build_info.latest_commit_date(*latest_commit_date_);
  }

  completer.Reply({{.build_info = std::move(build_info)}});
}
