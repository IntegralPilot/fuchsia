// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "build_info.h"

#include <fidl/fuchsia.buildinfo/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/fdio.h>
#include <lib/fdio/namespace.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/pseudo_file.h>
#include <zircon/status.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace {
const char kFuchsiaBuildInfoDirectoryPath[] = "/config/build-info";

const char kProductFileName[] = "product";
const char kBoardFileName[] = "board";
const char kVersionFileName[] = "version";
const char kPlatformVersionFileName[] = "platform_version";
const char kProductVersionFileName[] = "product_version";
const char kLastCommitDateFileName[] = "latest-commit-date";
}  // namespace

class BuildInfoServiceTestFixture : public gtest::TestLoopFixture {
 public:
  BuildInfoServiceTestFixture() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {}

  void SetUp() override {
    TestLoopFixture::SetUp();

    loop_.StartThread();

    // Get the process's namespace.
    fdio_ns_t* ns;
    zx_status_t status = fdio_ns_get_installed(&ns);
    ZX_ASSERT_MSG(status == ZX_OK, "Cannot get namespace: %s\n", zx_status_get_string(status));

    // Create the /config/build-info path in the namespace.
    auto [build_info_client, build_info_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    std::string build_info_directory_path(kFuchsiaBuildInfoDirectoryPath);
    status = fdio_ns_bind(ns, build_info_directory_path.c_str(),
                          build_info_client.TakeChannel().release());
    ZX_ASSERT_MSG(status == ZX_OK, "Cannot bind %s to namespace: %s\n",
                  build_info_directory_path.c_str(), zx_status_get_string(status));

    // Connect the build-info PseudoDir to the /config/build-info path.
    build_info_directory_.Serve(fuchsia_io::wire::kPermReadable | fuchsia_io::wire::kPermWritable,
                                std::move(build_info_server), loop_.dispatcher());
  }

  // Trailing newlines are added because typical file creation (e.g. echo "foo" > file)
  // includes them, and the build info service is expected to trim them.
  void CreateBuildInfoFile(std::string build_info_filename, bool with_trailing_newline = true) {
    std::string file_contents(build_info_filename);

    if (with_trailing_newline) {
      file_contents.append("\n");
    }

    vfs::PseudoFile::ReadHandler versionFileReadFn = [file_contents](std::vector<uint8_t>* output,
                                                                     size_t max_file_size) {
      output->resize(file_contents.length());
      std::copy(file_contents.begin(), file_contents.end(), output->begin());
      return ZX_OK;
    };
    vfs::PseudoFile::WriteHandler versionFileWriteFn;

    std::unique_ptr<vfs::PseudoFile> pseudo_file = std::make_unique<vfs::PseudoFile>(
        file_contents.length(), std::move(versionFileReadFn), std::move(versionFileWriteFn));

    build_info_directory_.AddEntry(std::move(build_info_filename), std::move(pseudo_file));
  }

  void TearDown() override {
    TestLoopFixture::TearDown();
    DestroyBuildInfoFile();
  }

 protected:
  fidl::Client<fuchsia_buildinfo::Provider> GetProxy() {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_buildinfo::Provider>::Create();
    bindings_.AddBinding(dispatcher(), std::move(server_end), &impl_, fidl::kIgnoreBindingClosure);
    return fidl::Client<fuchsia_buildinfo::Provider>(std::move(client_end), dispatcher());
  }

 private:
  void DestroyBuildInfoFile() {
    fdio_ns_t* ns;
    zx_status_t status = fdio_ns_get_installed(&ns);
    ZX_ASSERT_MSG(status == ZX_OK, "Cannot retrieve the namespace: %s\n",
                  zx_status_get_string(status));

    std::string build_info_directory_path(kFuchsiaBuildInfoDirectoryPath);
    status = fdio_ns_unbind(ns, build_info_directory_path.c_str());
    ZX_ASSERT_MSG(status == ZX_OK, "Cannot unbind from a namespace: %s\n",
                  zx_status_get_string(status));

    loop_.Quit();
    loop_.JoinThreads();
  }

  ProviderImpl impl_;
  fidl::ServerBindingGroup<fuchsia_buildinfo::Provider> bindings_;
  vfs::PseudoDir build_info_directory_;
  async::Loop loop_;
};

TEST_F(BuildInfoServiceTestFixture, BuildInfo) {
  CreateBuildInfoFile(kProductFileName);
  CreateBuildInfoFile(kBoardFileName);
  CreateBuildInfoFile(kVersionFileName);
  CreateBuildInfoFile(kPlatformVersionFileName);
  CreateBuildInfoFile(kProductVersionFileName);
  CreateBuildInfoFile(kLastCommitDateFileName);

  auto proxy = GetProxy();
  bool called = false;
  proxy->GetBuildInfo().Then([&](fidl::Result<fuchsia_buildinfo::Provider::GetBuildInfo>& result) {
    ASSERT_TRUE(result.is_ok());
    auto build_info = result.value().build_info();

    EXPECT_TRUE(build_info.product_config().has_value());
    EXPECT_EQ(build_info.product_config().value(), kProductFileName);
    EXPECT_TRUE(build_info.board_config().has_value());
    EXPECT_EQ(build_info.board_config().value(), kBoardFileName);
    EXPECT_TRUE(build_info.version().has_value());
    EXPECT_EQ(build_info.version().value(), kVersionFileName);
    EXPECT_TRUE(build_info.platform_version().has_value());
    EXPECT_EQ(build_info.platform_version().value(), kPlatformVersionFileName);
    EXPECT_TRUE(build_info.product_version().has_value());
    EXPECT_EQ(build_info.product_version().value(), kProductVersionFileName);
    EXPECT_TRUE(build_info.latest_commit_date().has_value());
    EXPECT_EQ(build_info.latest_commit_date().value(), kLastCommitDateFileName);
    called = true;
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(called);
}

TEST_F(BuildInfoServiceTestFixture, EmptyBuildInfo) {
  CreateBuildInfoFile("");
  CreateBuildInfoFile("");
  CreateBuildInfoFile("");
  CreateBuildInfoFile("");

  auto proxy = GetProxy();
  bool called = false;
  proxy->GetBuildInfo().Then([&](fidl::Result<fuchsia_buildinfo::Provider::GetBuildInfo>& result) {
    ASSERT_TRUE(result.is_ok());
    auto build_info = result.value().build_info();

    EXPECT_FALSE(build_info.product_config().has_value());
    EXPECT_FALSE(build_info.board_config().has_value());
    EXPECT_FALSE(build_info.version().has_value());
    EXPECT_FALSE(build_info.platform_version().has_value());
    EXPECT_FALSE(build_info.product_version().has_value());
    EXPECT_FALSE(build_info.latest_commit_date().has_value());
    called = true;
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(called);
}

TEST_F(BuildInfoServiceTestFixture, NonPresentBuildInfo) {
  auto proxy = GetProxy();
  bool called = false;
  proxy->GetBuildInfo().Then([&](fidl::Result<fuchsia_buildinfo::Provider::GetBuildInfo>& result) {
    ASSERT_TRUE(result.is_ok());
    auto build_info = result.value().build_info();

    EXPECT_FALSE(build_info.product_config().has_value());
    EXPECT_FALSE(build_info.board_config().has_value());
    EXPECT_FALSE(build_info.version().has_value());
    EXPECT_FALSE(build_info.platform_version().has_value());
    EXPECT_FALSE(build_info.product_version().has_value());
    EXPECT_FALSE(build_info.latest_commit_date().has_value());
    called = true;
  });

  RunLoopUntilIdle();
  EXPECT_TRUE(called);
}
