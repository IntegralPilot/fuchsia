// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.process/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/zx/channel.h>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

namespace {

class DirectoryTest : public zxtest::Test {
  void TearDown() override {
    ZX_ASSERT_MSG(chdir("/") == 0, "Failed to reset working directory to /: %s", strerror(errno));
  }
};

constexpr uint64_t kFlags = static_cast<uint64_t>(fuchsia_io::wire::kRStarDir);

// kPathTooLong is `/a{4095}\0` (leading forward slash then 4,095 'a's then null), which including
// the null at the end, is 4,097 bytes total. This is longer than the maximum allowed length (4096):
// https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.io/io.fidl;l=47;drc=e7cbd843e8ced20ea21f9213989d803ae64fcfaf
constexpr std::string_view kPathTooLong =
    "/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
static_assert(kPathTooLong.length() == 4096);

TEST_F(DirectoryTest, ServiceConnect) {
  ASSERT_STATUS(fdio_service_connect(nullptr, ZX_HANDLE_INVALID), ZX_ERR_INVALID_ARGS);

  zx::channel h1, h2;
  ASSERT_OK(zx::channel::create(0, &h1, &h2));
  ASSERT_STATUS(fdio_service_connect("/x/y/z", h1.release()), ZX_ERR_NOT_FOUND);
  ASSERT_STATUS(fdio_service_connect("/", h2.release()), ZX_ERR_NOT_SUPPORTED);

  ASSERT_OK(zx::channel::create(0, &h1, &h2));
  ASSERT_OK(fdio_service_connect(fidl::DiscoverableProtocolDefaultPath<fuchsia_process::Launcher>,
                                 h1.release()));
}

TEST_F(DirectoryTest, ServiceConnectAt) {
  zx::channel h1, h2;
  ASSERT_OK(zx::channel::create(0, &h1, &h2));
  ASSERT_OK(fdio_open3("/svc", kFlags, h1.release()));

  ASSERT_STATUS(fdio_service_connect_at(h2.get(), nullptr, ZX_HANDLE_INVALID), ZX_ERR_INVALID_ARGS);

  zx::channel h3, h4;
  ASSERT_OK(zx::channel::create(0, &h3, &h4));
  ASSERT_OK(fdio_service_connect_at(
      h2.get(), fidl::DiscoverableProtocolName<fuchsia_process::Launcher>, h3.release()));
}

TEST_F(DirectoryTest, Open3) {
  ASSERT_STATUS(fdio_open3(nullptr, 0, ZX_HANDLE_INVALID), ZX_ERR_INVALID_ARGS);

  zx::channel h1, h2;
  ASSERT_OK(zx::channel::create(0, &h1, &h2));
  ASSERT_STATUS(fdio_open3("/x/y/z", kFlags, h1.release()), ZX_ERR_NOT_FOUND);
  ASSERT_STATUS(fdio_open3("/", kFlags, h2.release()), ZX_ERR_NOT_SUPPORTED);
}

TEST_F(DirectoryTest, Open3Fd) {
  fbl::unique_fd fd;
  ASSERT_STATUS(fdio_open3_fd(nullptr, kFlags, fd.reset_and_get_address()), ZX_ERR_INVALID_ARGS);
  ASSERT_STATUS(fdio_open3_fd("/x/y/z", kFlags, fd.reset_and_get_address()), ZX_ERR_NOT_FOUND);

  // Opening local directories, like the root of the namespace, should be supported.
  ASSERT_OK(fdio_open3_fd("/", kFlags, fd.reset_and_get_address()));

  // Paths should be canonicalized per
  // https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.io/io.fidl;l=41-43;drc=e7cbd843e8ced20ea21f9213989d803ae64fcfaf
  ASSERT_OK(fdio_open3_fd("/pkg/..", kFlags, fd.reset_and_get_address()));

  // Paths of 4,097 bytes (including the null) or more should be rejected.
  ASSERT_STATUS(fdio_open3_fd(kPathTooLong.data(), kFlags, fd.reset_and_get_address()),
                ZX_ERR_BAD_PATH);

  // Path canonicalization of consecutive '/'s should be handled gracefully.
  ASSERT_OK(fdio_open3_fd("//", kFlags, fd.reset_and_get_address()));

  // Relative paths are interpreted to CWD which is '/' at this point of the test.
  ASSERT_OK(fdio_open3_fd("pkg", kFlags, fd.reset_and_get_address()));

  // Should fail to open path ending with a slash if last component is not a directory.
  ASSERT_STATUS(fdio_open3_fd("/pkg/test/fdio-test/", kFlags, fd.reset_and_get_address()),
                ZX_ERR_NOT_DIR);
}

TEST_F(DirectoryTest, Open3FdChangeWorkingDir) {
  // Ensure changes to the working directory are handled.
  ASSERT_EQ(chdir("/pkg"), 0, "errno %d: %s", errno, strerror(errno));

  fbl::unique_fd fd;
  ASSERT_OK(fdio_open3_fd("test", kFlags, fd.reset_and_get_address()));
  ASSERT_TRUE(fd.is_valid());
}

TEST_F(DirectoryTest, Open3FdAt) {
  fbl::unique_fd fd;
  ASSERT_OK(fdio_open3_fd("/pkg/test", kFlags, fd.reset_and_get_address()));
  ASSERT_TRUE(fd.is_valid());

  fbl::unique_fd fd2;
  ASSERT_STATUS(fdio_open3_fd_at(fd.get(), nullptr, kFlags, fd2.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
  ASSERT_FALSE(fd2.is_valid());
  ASSERT_STATUS(
      fdio_open3_fd_at(fd.get(), "some-nonexistent-file", kFlags, fd2.reset_and_get_address()),
      ZX_ERR_NOT_FOUND);
  ASSERT_FALSE(fd2.is_valid());

  // fdio_open3_fd_at() should not resolve absolute paths to the root directory, unlike openat().
  ASSERT_STATUS(fdio_open3_fd_at(fd.get(), "/pkg", kFlags, fd2.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
  ASSERT_FALSE(fd2.is_valid());

  ASSERT_STATUS(fdio_open3_fd_at(fd.get(), "/fdio-test", kFlags, fd2.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
  ASSERT_FALSE(fd2.is_valid());

  // fdio_open3_fd_at rejects paths of 4,097 bytes (including the null) or more.
  ASSERT_STATUS(
      fdio_open3_fd_at(fd.get(), kPathTooLong.data(), kFlags, fd2.reset_and_get_address()),
      ZX_ERR_BAD_PATH);

  // Verify that we can open and read from the binary this test is compiled into.
  ASSERT_OK(fdio_open3_fd_at(fd.get(), "fdio-test", kFlags, fd2.reset_and_get_address()));
  ASSERT_TRUE(fd2.is_valid());
  char buf[256];
  ssize_t bytes_read = read(fd2.get(), buf, 256);
  ASSERT_EQ(bytes_read, 256);

  // Paths should be canonicalized per
  // https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.io/io.fidl;l=41-43;drc=e7cbd843e8ced20ea21f9213989d803ae64fcfaf
  ASSERT_OK(fdio_open3_fd_at(fd.get(), "fdio-test/..", kFlags, fd2.reset_and_get_address()));
  ASSERT_TRUE(fd2.is_valid());

  // Should fail to open path ending with a slash if last component is not a directory.
  ASSERT_STATUS(fdio_open3_fd_at(fd.get(), "fdio-test/", kFlags, fd2.reset_and_get_address()),
                ZX_ERR_NOT_DIR);
  ASSERT_FALSE(fd2.is_valid());
}

TEST_F(DirectoryTest, Open3AtConsumesHandleOnFailure) {
  zx::channel h1, h2;
  ASSERT_OK(zx::channel::create(0, &h1, &h2));
  ASSERT_OK(fdio_open3("/svc", kFlags, h1.release()));

  zx::channel h3, h4;
  ASSERT_OK(zx::channel::create(0, &h3, &h4));
  // Passing nullptr for path should fail with ERR_INVALID_ARGS.
  ASSERT_STATUS(fdio_open3_at(h2.get(), nullptr, kFlags, h3.release()), ZX_ERR_INVALID_ARGS);

  // fdio_open3_at should take ownership of the request handle even on failure.
  zx_signals_t pending;
  ASSERT_OK(h4.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), &pending));
  ASSERT_TRUE((pending & ZX_CHANNEL_PEER_CLOSED) != 0);
}

}  // namespace
