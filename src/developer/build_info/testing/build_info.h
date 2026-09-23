// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_BUILD_INFO_TESTING_BUILD_INFO_H_
#define SRC_DEVELOPER_BUILD_INFO_TESTING_BUILD_INFO_H_

#include <fidl/fuchsia.buildinfo.test/cpp/fidl.h>
#include <fidl/fuchsia.buildinfo/cpp/fidl.h>

// Stores the build information values set by the test controller.
struct fake_info {
  std::string product_config_;
  std::string board_config_;
  std::string version_;
  std::string platform_version_;
  std::string product_version_;
  std::string latest_commit_date_;
};

// Sets fake system build information. Used for testing.
class BuildInfoTestControllerImpl
    : public fidl::Server<fuchsia_buildinfo_test::BuildInfoTestController> {
 public:
  explicit BuildInfoTestControllerImpl(std::shared_ptr<struct fake_info> info_ref)
      : info_ref_(std::move(info_ref)) {}

  // Set the value to be returned by GetBuildInfo() in the Provider.
  void SetBuildInfo(SetBuildInfoRequest& request, SetBuildInfoCompleter::Sync& completer) override;

 private:
  std::shared_ptr<struct fake_info> info_ref_;
};

// Returns fake system build information. Used for testing.
class FakeProviderImpl : public fidl::Server<fuchsia_buildinfo::Provider> {
 public:
  static constexpr auto kProductNameDefault = "core";
  static constexpr auto kBoardNameDefault = "chromebook-x64";
  static constexpr auto kVersionDefault = "2019-03-28T09:00:20+00:00";
  static constexpr auto kPlatformVersionDefault = "2024-03-28T09:00:20+00:00";
  static constexpr auto kProductVersionDefault = "2024-04-28T09:00:20+00:00";
  static constexpr auto kLastCommitDateDefault = "2019-03-28T09:00:20+00:00";

  explicit FakeProviderImpl(std::shared_ptr<struct fake_info> info_ref)
      : info_ref_(std::move(info_ref)) {}

  // Returns a fake product, board, version, and timestamp information used at build time.
  void GetBuildInfo(GetBuildInfoCompleter::Sync& completer) override;

 private:
  std::shared_ptr<struct fake_info> info_ref_;
};
#endif  // SRC_DEVELOPER_BUILD_INFO_TESTING_BUILD_INFO_H_
