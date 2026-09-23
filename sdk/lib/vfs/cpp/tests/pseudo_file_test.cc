// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// These tests verify basic functionality of `vfs::PseudoFile`. For more comprehensive tests, see
// //src/storage/lib/vfs/cpp and //src/storage/conformance.

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/io.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/pseudo_file.h>
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

constexpr size_t kMaxFileSize = kNewContents.size();

// Populates the file with the contents of kFileContents.

// Fixture sets up a file with max size equal to kNewContents, but initialized with kFileContents.
class PseudoFileTest : public ::gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    root_ = std::make_unique<vfs::PseudoDir>();

    auto buffer = std::make_shared<std::vector<uint8_t>>();
    buffer->resize(kFileContents.size());
    std::memcpy(buffer->data(), kFileContents.data(), kFileContents.size());

    auto read_handler = [buffer](std::vector<uint8_t>* output, size_t max_bytes) {
      output->resize(buffer->size());
      std::memcpy(output->data(), buffer->data(), buffer->size());
      return ZX_OK;
    };

    auto write_handler = [buffer](std::vector<uint8_t> contents) {
      *buffer = std::move(contents);
      return ZX_OK;
    };

    auto writable_file = std::make_shared<vfs::PseudoFile>(kMaxFileSize, std::move(read_handler),
                                                           std::move(write_handler));
    root_->AddSharedEntry("file", writable_file);
    writable_file_ = std::move(writable_file);

    auto read_only_handler = [](std::vector<uint8_t>* output, size_t max_bytes) {
      output->resize(kFileContents.size());
      std::memcpy(output->data(), kFileContents.data(), kFileContents.size());
      return ZX_OK;
    };
    auto read_only_file =
        std::make_shared<vfs::PseudoFile>(kMaxFileSize, std::move(read_only_handler), nullptr);
    root_->AddSharedEntry("read_only_file", read_only_file);
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
        fdio_fd_create(root_client_.TakeChannel().release(), fd.reset_and_get_address());
    ZX_ASSERT_MSG(status == ZX_OK, "Failed to create fd: %s", zx_status_get_string(status));
    return fd;
  }

  vfs::PseudoFile* read_only_file() const { return read_only_file_.get(); }
  vfs::PseudoFile* writable_file() const { return writable_file_.get(); }

 private:
  std::unique_ptr<vfs::PseudoDir> root_;
  std::shared_ptr<vfs::PseudoFile> read_only_file_;
  std::shared_ptr<vfs::PseudoFile> writable_file_;
  fidl::ClientEnd<fuchsia_io::Directory> root_client_;
};

TEST_F(PseudoFileTest, ServeReadOnly) {
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

TEST_F(PseudoFileTest, ServeWritable) {
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

TEST_F(PseudoFileTest, ReadWrite) {
  PerformBlockingWork([this] {
    auto root = open_root_fd();
    fbl::unique_fd file(openat(root.get(), "file", O_RDWR));
    ASSERT_TRUE(file) << "Failed to open file: " << strerror(errno);

    // Verify initial contents. We should only be able to read as much as the initial contents.
    std::string contents(kFileContents);
    contents.resize(kMaxFileSize, 0);
    std::string buffer(kMaxFileSize, 0);
    ASSERT_EQ(read(file.get(), buffer.data(), buffer.size()),
              static_cast<ssize_t>(kFileContents.size()));
    ASSERT_EQ(lseek(file.get(), 0, SEEK_SET), 0) << strerror(errno);
    ASSERT_EQ(contents, buffer);

    // Write new contents. Writes past EOF should be handled gracefully.
    contents = kNewContents;
    contents.resize(kMaxFileSize + 100);
    ASSERT_EQ(write(file.get(), contents.data(), contents.size()),
              static_cast<ssize_t>(kMaxFileSize));
    ASSERT_EQ(lseek(file.get(), 0, SEEK_SET), 0) << strerror(errno);

    // Read back data and verify. This is a buffered pseudo-file, so we won't observe any side
    // effects to the state of the file until it is re-opened.
    ASSERT_EQ(read(file.get(), buffer.data(), buffer.size()),
              static_cast<ssize_t>(kFileContents.size()));
    ASSERT_EQ(lseek(file.get(), 0, SEEK_SET), 0) << strerror(errno);
    ASSERT_EQ(buffer.substr(0, kFileContents.size()), kFileContents);

    // Once we close and re-open the file, we should observe the changes we expect.
    file.reset();
    file = fbl::unique_fd(openat(root.get(), "file", O_RDWR));
    ASSERT_EQ(read(file.get(), buffer.data(), buffer.size()), static_cast<ssize_t>(buffer.size()));
    ASSERT_EQ(lseek(file.get(), 0, SEEK_SET), 0) << strerror(errno);
    ASSERT_EQ(contents.substr(0, kMaxFileSize), buffer);
  });
}

TEST_F(PseudoFileTest, ReadOnly) {
  PerformBlockingWork([this] {
    auto root = open_root_fd();
    fbl::unique_fd file(openat(root.get(), "read_only_file", O_RDWR));
    ASSERT_FALSE(file) << "Should fail to open read-only PseudoFile as writable!";
    ASSERT_EQ(errno, EACCES);

    file = fbl::unique_fd(openat(root.get(), "read_only_file", O_RDONLY));
    ASSERT_TRUE(file) << "Failed to open read-only PseudoFile: " << strerror(errno);

    std::string contents(kFileContents);
    contents.resize(kMaxFileSize, 0);
    std::string buffer(kMaxFileSize, 0);
    ASSERT_EQ(read(file.get(), buffer.data(), buffer.size()),
              static_cast<ssize_t>(kFileContents.size()));
    ASSERT_EQ(lseek(file.get(), 0, SEEK_SET), 0) << strerror(errno);
    ASSERT_EQ(contents, buffer);

    // Writes should fail.
    contents = kNewContents;
    contents.resize(kMaxFileSize + 100);
    ASSERT_LT(write(file.get(), contents.data(), contents.size()), 0);
    ASSERT_EQ(errno, EBADF) << strerror(errno);
  });
}

}  // namespace
