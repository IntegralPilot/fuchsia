// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_BUILD_INFO_BUILD_INFO_H_
#define SRC_DEVELOPER_BUILD_INFO_BUILD_INFO_H_

#include <fidl/fuchsia.buildinfo/cpp/fidl.h>

// Returns system build information.
class ProviderImpl : public fidl::Server<fuchsia_buildinfo::Provider> {
 public:
  // Returns product, board, version, and timestamp information used at build time.
  void GetBuildInfo(GetBuildInfoCompleter::Sync& completer) override;

 private:
  std::unique_ptr<std::string> product_config_;
  std::unique_ptr<std::string> board_config_;
  std::unique_ptr<std::string> version_;
  std::unique_ptr<std::string> platform_version_;
  std::unique_ptr<std::string> product_version_;
  std::unique_ptr<std::string> latest_commit_date_;
};

#endif  // SRC_DEVELOPER_BUILD_INFO_BUILD_INFO_H_
