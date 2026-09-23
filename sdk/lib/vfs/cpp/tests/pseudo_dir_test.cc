// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// These tests verify basic functionality of `vfs::PseudoDir`. For more comprehensive tests, see
// //src/storage/lib/vfs/cpp and //src/storage/conformance.

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/vmo_file.h>
#include <zircon/status.h>

#include <memory>
#include <string>
#include <string_view>

#include <fbl/unique_fd.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace {

constexpr std::string_view kFileContents = "See you, Space Cowboy...";

// Fixture sets up the following hierarchy, where subdir_a/b are the *same* Vnode:
//
//    root/
//      subdir_a/
//        unique_file
//      subdir_b/
//        unique_file
//
class PseudoDirTest : public ::gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    root_ = std::make_unique<vfs::PseudoDir>();

    zx::vmo vmo;
    ASSERT_EQ(zx::vmo::create(kFileContents.size(), 0, &vmo), ZX_OK);
    ASSERT_EQ(vmo.write(kFileContents.data(), 0, kFileContents.size()), ZX_OK);
    subdir_ = std::make_shared<vfs::PseudoDir>();
    auto unique_file = std::make_unique<vfs::VmoFile>(std::move(vmo), 100);
    ASSERT_EQ(subdir_->AddEntry("unique_file", std::move(unique_file)), ZX_OK);

    ASSERT_EQ(root_->AddSharedEntry("subdir_a", subdir_), ZX_OK);
    ASSERT_EQ(root_->AddSharedEntry("subdir_b", subdir_), ZX_OK);
  }

  vfs::PseudoDir* root() { return root_.get(); }
  vfs::PseudoDir* subdir() { return subdir_.get(); }

 private:
  std::unique_ptr<vfs::PseudoDir> root_;
  std::shared_ptr<vfs::PseudoDir> subdir_;
};

TEST_F(PseudoDirTest, Lookup) {
  // Non-existing node should not be found.
  ASSERT_EQ(root()->Lookup("does_not_exist", nullptr), ZX_ERR_NOT_FOUND);

  // Both subdir_a and subdir_b should point to the same node.
  vfs::Node* subdir_a;
  ASSERT_EQ(root()->Lookup("subdir_a", &subdir_a), ZX_OK);
  ASSERT_EQ(subdir_a, subdir());

  vfs::Node* subdir_b;
  ASSERT_EQ(root()->Lookup("subdir_b", &subdir_b), ZX_OK);
  ASSERT_EQ(subdir_b, subdir());

  vfs::Node* unique_file;
  ASSERT_EQ(subdir()->Lookup("unique_file", &unique_file), ZX_OK);
  vfs::Node* unique_file_a;
  ASSERT_EQ(static_cast<vfs::PseudoDir*>(subdir_a)->Lookup("unique_file", &unique_file_a), ZX_OK);
  vfs::Node* unique_file_b;
  ASSERT_EQ(static_cast<vfs::PseudoDir*>(subdir_b)->Lookup("unique_file", &unique_file_b), ZX_OK);

  // The entry for `unique_file` should be the same node in both sub directories.
  ASSERT_EQ(unique_file, unique_file_a);
  ASSERT_EQ(unique_file, unique_file_b);
}

TEST_F(PseudoDirTest, RemoveEntry) {
  ASSERT_EQ(root()->RemoveEntry("does_not_exist"), ZX_ERR_NOT_FOUND);
  ASSERT_EQ(root()->RemoveEntry("subdir_a"), ZX_OK);
  ASSERT_EQ(root()->RemoveEntry("subdir_a"), ZX_ERR_NOT_FOUND);
  ASSERT_EQ(root()->RemoveEntry("subdir_b", /*node*/ root()), ZX_ERR_NOT_FOUND);
  ASSERT_EQ(root()->RemoveEntry("subdir_b", /*node*/ subdir()), ZX_OK);
}

TEST_F(PseudoDirTest, Serve) {
  auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  ASSERT_EQ(root()->Serve(fuchsia_io::kPermReadable, std::move(root_server)), ZX_OK);

  PerformBlockingWork([root_client = std::move(root_client)]() mutable {
    fbl::unique_fd root_fd;
    ASSERT_EQ(fdio_fd_create(root_client.TakeChannel().release(), root_fd.reset_and_get_address()),
              ZX_OK);

    fbl::unique_fd subdir_fd(openat(root_fd.get(), "subdir_a", O_DIRECTORY));
    ASSERT_TRUE(subdir_fd) << strerror(errno);
    fbl::unique_fd file_fd(openat(subdir_fd.get(), "unique_file", O_RDONLY));
    ASSERT_TRUE(file_fd) << strerror(errno);

    std::string read_buffer(kFileContents.size(), 0);
    ssize_t bytes_read = read(file_fd.get(), read_buffer.data(), kFileContents.size());
    ASSERT_EQ(bytes_read, static_cast<ssize_t>(kFileContents.size()));
    ASSERT_EQ(kFileContents, read_buffer);
  });
}

TEST_F(PseudoDirTest, ServeFailsWithInvalidArgs) {
  // Serve should fail with an invalid channel.
  ASSERT_EQ(root()->Serve(fuchsia_io::kPermReadable, {}), ZX_ERR_BAD_HANDLE);
  // Serve should fail with an invalid set of flags.
  {
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root()->Serve(static_cast<fuchsia_io::Flags>(~0ull), std::move(root_server)),
              ZX_ERR_INVALID_ARGS);
  }
  // Non-directory protocols are disallowed.
  {
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root()->Serve(fuchsia_io::Flags::kProtocolFile, std::move(root_server)),
              ZX_ERR_INVALID_ARGS);
  }
}

TEST_F(PseudoDirTest, ServeFailsWithDifferentDispatcher) {
  zx::channel root_client, root_server;
  // We should be able to serve the same node multiple times as long as we use the same dispatcher.
  for (size_t i = 0; i < 3; ++i) {
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root()->Serve(fuchsia_io::kPermReadable, std::move(root_server)), ZX_OK);
  }
  // Serve should fail with a different dispatcher. Only one dispatcher may be registered to serve
  // a given node.
  {
    async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root()->Serve(fuchsia_io::kPermReadable, std::move(root_server), loop.dispatcher()),
              ZX_ERR_INVALID_ARGS);
  }
}

}  // namespace
