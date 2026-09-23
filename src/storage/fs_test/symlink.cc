// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <sys/stat.h>
#include <unistd.h>

#include "src/storage/fs_test/fs_test_fixture.h"

namespace fs_test {
namespace {

namespace fio = fuchsia_io;

using SymlinkTest = FilesystemTest;

TEST_P(SymlinkTest, LinkIntoFromUnlinkedSymlinkFails) {
  const std::string symlink_path = GetPath("symlink");
  const std::string target_dir_path = GetPath("target_dir");

  // Create target directory
  ASSERT_EQ(mkdir(target_dir_path.c_str(), 0755), 0);

  // Open root directory to call CreateSymlink
  fbl::unique_fd root_fd(open(GetPath("").c_str(), O_RDONLY | O_DIRECTORY));
  ASSERT_TRUE(root_fd.is_valid());
  fdio_cpp::FdioCaller root_caller(std::move(root_fd));

  // Create symlink via FIDL
  auto symlink_endpoints = fidl::Endpoints<fio::Symlink>::Create();
  uint8_t target[] = {'t', 'a', 'r', 'g', 'e', 't'};
  auto target_view = fidl::VectorView<uint8_t>::FromExternal(target, sizeof(target));

  auto create_result = fidl::WireCall(root_caller.borrow_as<fio::Directory>())
                           ->CreateSymlink(fidl::StringView("symlink"), target_view,
                                           std::move(symlink_endpoints.server));
  ASSERT_TRUE(create_result.ok());
  ASSERT_FALSE(create_result->is_error()) << create_result->error_value();

  // Unlink the symlink (in-memory ref count should remain 1 because we hold the client channel)
  ASSERT_EQ(unlink(symlink_path.c_str()), 0) << strerror(errno);

  // Attempt to LinkInto the target directory. This should fail because the symlink is unlinked.
  // Get token for target directory
  fbl::unique_fd dir_fd(open(target_dir_path.c_str(), O_RDONLY | O_DIRECTORY));
  ASSERT_TRUE(dir_fd.is_valid());
  fdio_cpp::FdioCaller dir_caller(std::move(dir_fd));
  auto token_result = fidl::WireCall(dir_caller.borrow_as<fio::Directory>())->GetToken();
  ASSERT_EQ(token_result.status(), ZX_OK);
  ASSERT_EQ(token_result->s, ZX_OK);

  zx::handle duplicated_token;
  ASSERT_EQ(token_result->token.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicated_token), ZX_OK);
  zx::event token(duplicated_token.release());

  // Call LinkInto using the client end we held
  auto link_result = fidl::WireCall(symlink_endpoints.client)
                         ->LinkInto(std::move(token), fidl::StringView("new_symlink"));

  // It must fail with NOT_FOUND.
  ASSERT_TRUE(link_result.ok());
  ASSERT_TRUE(link_result->is_error());
  EXPECT_EQ(link_result->error_value(), ZX_ERR_NOT_FOUND);
}

INSTANTIATE_TEST_SUITE_P(
    /*no prefix*/, SymlinkTest,
    testing::ValuesIn(MapAndFilterAllTestFilesystems(
        [](const TestFilesystemOptions& options) -> std::optional<TestFilesystemOptions> {
          if (options.filesystem->GetTraits().supports_symlinks) {
            return options;
          } else {
            return std::nullopt;
          }
        })),
    testing::PrintToStringParamName());

GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(SymlinkTest);

}  // namespace
}  // namespace fs_test
