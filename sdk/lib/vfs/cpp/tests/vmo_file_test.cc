// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// These tests verify basic functionality of `vfs::VmoFile`. For more comprehensive tests, see
// //src/storage/lib/vfs/cpp and //src/storage/conformance.

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/io.h>
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
constexpr std::string_view kNewContents = "This string should be longer than kFileContents.";
static_assert(kNewContents.size() > kFileContents.size());

// Fixture sets up the following hierarchy, where both files are initialized with and sized to
// the exact length of kFileContents:
//
//    root/
//      writable_file
//      read_only_file
//
class VmoFileTest : public ::gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    root_ = std::make_unique<vfs::PseudoDir>();

    zx::vmo vmo;
    ASSERT_EQ(zx::vmo::create(kFileContents.size(), 0, &vmo), ZX_OK);
    ASSERT_EQ(vmo.write(kFileContents.data(), 0, kFileContents.size()), ZX_OK);
    auto writable_file = std::make_shared<vfs::VmoFile>(std::move(vmo), kFileContents.size(),
                                                        vfs::VmoFile::WriteMode::kWritable);
    ASSERT_EQ(root_->AddSharedEntry("writable_file", writable_file), ZX_OK);
    writable_file_ = std::move(writable_file);

    ASSERT_EQ(zx::vmo::create(kFileContents.size(), 0, &vmo), ZX_OK);
    ASSERT_EQ(vmo.write(kFileContents.data(), 0, kFileContents.size()), ZX_OK);
    auto read_only_file = std::make_shared<vfs::VmoFile>(std::move(vmo), kFileContents.size(),
                                                         vfs::VmoFile::WriteMode::kReadOnly);
    ASSERT_EQ(root_->AddSharedEntry("read_only_file", read_only_file), ZX_OK);
    read_only_file_ = std::move(read_only_file);

    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(
        root_->Serve(fuchsia_io::kPermReadable | fuchsia_io::kPermWritable, std::move(root_server)),
        ZX_OK);
    root_client_ = std::move(root_client);
  }

  // Consumes and opens the root connection as a file descriptor. This must be called from a
  // different thread than the one serving the connection.
  fbl::unique_fd open_root_fd() {
    ZX_ASSERT_MSG(root_client_.is_valid(), "open_root_fd() can only be called once per test!");
    fbl::unique_fd fd;
    zx_status_t status =
        fdio_fd_create(root_client_.TakeHandle().release(), fd.reset_and_get_address());
    ZX_ASSERT_MSG(status == ZX_OK, "Failed to create fd: %s", zx_status_get_string(status));
    return fd;
  }

  vfs::VmoFile* read_only_file() { return read_only_file_.get(); }

  vfs::VmoFile* writable_file() { return writable_file_.get(); }

 private:
  std::unique_ptr<vfs::PseudoDir> root_;
  std::shared_ptr<vfs::VmoFile> read_only_file_;
  std::shared_ptr<vfs::VmoFile> writable_file_;
  fidl::ClientEnd<fuchsia_io::Directory> root_client_;
};

TEST_F(VmoFileTest, ServeReadOnly) {
  // Read-only connections should be allowed.
  {
    auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
    ASSERT_EQ(read_only_file()->Serve(fuchsia_io::kPermReadable, std::move(server)), ZX_OK);
  }
  // Write and execute access should be disallowed.
  {
    auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
    ASSERT_EQ(read_only_file()->Serve(fuchsia_io::kPermWritable, std::move(server)),
              ZX_ERR_ACCESS_DENIED);
  }
  {
    auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
    ASSERT_EQ(read_only_file()->Serve(fuchsia_io::Flags::kPermExecute, std::move(server)),
              ZX_ERR_ACCESS_DENIED);
  }
  // Non-file protocols should be disallowed.
  {
    auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
    ASSERT_EQ(read_only_file()->Serve(fuchsia_io::Flags::kProtocolDirectory, std::move(server)),
              ZX_ERR_INVALID_ARGS);
  }
}

TEST_F(VmoFileTest, ServeWritable) {
  // Read-only and write access should be allowed.
  {
    auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
    ASSERT_EQ(writable_file()->Serve(fuchsia_io::kPermReadable, std::move(server)), ZX_OK);
  }
  {
    auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
    ASSERT_EQ(writable_file()->Serve(fuchsia_io::kPermWritable, std::move(server)), ZX_OK);
  }
  // Execute access should be disallowed.
  {
    auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
    ASSERT_EQ(writable_file()->Serve(fuchsia_io::Flags::kPermExecute, std::move(server)),
              ZX_ERR_ACCESS_DENIED);
  }
  // Non-file protocols should be disallowed.
  {
    auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
    ASSERT_EQ(writable_file()->Serve(fuchsia_io::Flags::kProtocolDirectory, std::move(server)),
              ZX_ERR_INVALID_ARGS);
  }
}

TEST_F(VmoFileTest, ReadOnlyIsNotWritable) {
  PerformBlockingWork([this] {
    auto root = open_root_fd();

    fbl::unique_fd file(openat(root.get(), "read_only_file", O_RDWR));
    ASSERT_FALSE(file) << "Should fail to open read_only_file as writable!";
    ASSERT_EQ(errno, EACCES) << strerror(errno);
  });
}

TEST_F(VmoFileTest, ReadContents) {
  PerformBlockingWork([this] {
    auto root = open_root_fd();
    fbl::unique_fd file(openat(root.get(), "read_only_file", O_RDONLY));
    ASSERT_TRUE(file) << "Failed to open file: " << strerror(errno);
    // Attempts to read past EOF should respect file size.
    std::string buffer(kFileContents.size() + 100, 0);
    ssize_t bytes_read = read(file.get(), buffer.data(), kFileContents.size() + 100);
    ASSERT_EQ(bytes_read, static_cast<ssize_t>(kFileContents.size()));
    ASSERT_EQ(kFileContents, buffer.substr(0, kFileContents.size()));
  });
}

TEST_F(VmoFileTest, WriteContents) {
  PerformBlockingWork([this] {
    auto root = open_root_fd();
    fbl::unique_fd file(openat(root.get(), "writable_file", O_RDWR));
    ASSERT_TRUE(file) << "Failed to open file: " << strerror(errno);
    // Attempts to read/write past EOF should respect file size.
    ssize_t bytes_written = write(file.get(), kNewContents.data(), kNewContents.size());
    ASSERT_EQ(bytes_written, static_cast<ssize_t>(kFileContents.size()));
    ASSERT_EQ(lseek(file.get(), 0, SEEK_SET), 0) << strerror(errno);
    std::string buffer(kFileContents.size() + 100, 0);
    ssize_t bytes_read = read(file.get(), buffer.data(), buffer.size());
    ASSERT_EQ(bytes_read, static_cast<ssize_t>(kFileContents.size()));
    ASSERT_EQ(kNewContents.substr(0, kFileContents.size()), buffer.substr(0, kFileContents.size()));
  });
}

TEST_F(VmoFileTest, GetVmo) {
  PerformBlockingWork([this] {
    auto root = open_root_fd();
    fbl::unique_fd file(openat(root.get(), "writable_file", O_RDWR));
    ASSERT_TRUE(file) << "Failed to open file: " << strerror(errno);
    zx::vmo vmo_exact, vmo_clone;
    zx_status_t status = fdio_get_vmo_exact(file.get(), vmo_exact.reset_and_get_address());
    ASSERT_EQ(status, ZX_OK) << "fdio_get_vmo_exact: " << zx_status_get_string(status);
    status = fdio_get_vmo_clone(file.get(), vmo_clone.reset_and_get_address());
    ASSERT_EQ(status, ZX_OK) << "fdio_get_vmo_clone: " << zx_status_get_string(status);

    // Writes to the file should be reflected in vmo_exact but not vmo_clone.
    ASSERT_EQ(write(file.get(), kNewContents.data(), kNewContents.size()),
              static_cast<ssize_t>(kFileContents.size()));
    ASSERT_EQ(lseek(file.get(), 0, SEEK_SET), 0) << strerror(errno);
    std::string buffer(kFileContents.size(), 0);
    ASSERT_EQ(vmo_exact.read(buffer.data(), 0, buffer.size()), ZX_OK);
    ASSERT_EQ(buffer, kNewContents.substr(0, buffer.size()));
    ASSERT_EQ(vmo_clone.read(buffer.data(), 0, buffer.size()), ZX_OK);
    ASSERT_EQ(buffer, kFileContents);
  });
}

}  // namespace
