// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/cpp/caller.h>
#include <unistd.h>

#include <utility>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

#include "src/lib/files/scoped_temp_dir.h"

namespace fio = fuchsia_io;

namespace {

void TryFilesystemOperations(fidl::UnownedClientEnd<fio::File> client_end) {
  constexpr std::string_view payload("foobar");
  const fidl::WireResult write_result =
      fidl::WireCall(client_end)
          ->WriteAt(
              fidl::VectorView<uint8_t>::FromExternal(
                  reinterpret_cast<uint8_t*>(const_cast<char*>(payload.data())), payload.size()),
              0);
  ASSERT_OK(write_result);
  const fit::result write_response = write_result.value();
  ASSERT_TRUE(write_response.is_ok(), "%s", zx_status_get_string(write_response.error_value()));
  ASSERT_EQ(write_response.value()->actual_count, payload.size());

  const fidl::WireResult read_result = fidl::WireCall(client_end)->ReadAt(256, 0);
  ASSERT_OK(read_result);
  const fit::result read_response = read_result.value();
  ASSERT_TRUE(read_response.is_ok(), "%s", zx_status_get_string(read_response.error_value()));
  const fidl::VectorView data = read_result->value()->data;
  ASSERT_EQ(std::string_view(reinterpret_cast<const char*>(data.data()), data.size()), payload);
}

void TryFilesystemOperations(const zx::unowned_channel& channel) {
  TryFilesystemOperations(fidl::UnownedClientEnd<fio::File>(channel));
}

void TryFilesystemOperations(const zx::channel& channel) {
  TryFilesystemOperations(channel.borrow());
}

void TryFilesystemOperations(const fdio_cpp::FdioCaller& caller) {
  TryFilesystemOperations(caller.channel());
}

void TryFilesystemOperations(const fdio_cpp::UnownedFdioCaller& caller) {
  TryFilesystemOperations(caller.channel());
}

class FdioCallerTest : public zxtest::Test {
 protected:
  void SetUp() override {
    std::string path;
    ASSERT_TRUE(temp_dir_.NewTempFile(&path));
    fd_.reset(open(path.c_str(), O_CREAT | O_RDWR));
    ASSERT_TRUE(fd_.is_valid());
  }

  fbl::unique_fd& fd() { return fd_; }

 private:
  files::ScopedTempDir temp_dir_;
  fbl::unique_fd fd_;
};

TEST_F(FdioCallerTest, FdioCallerFile) {
  // Try some filesystem operations.
  fdio_cpp::FdioCaller caller(std::move(fd()));
  ASSERT_TRUE(caller);
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller));

  // Re-acquire the underlying fd.
  fbl::unique_fd fd = caller.release();
  ASSERT_EQ(close(fd.release()), 0);
}

TEST_F(FdioCallerTest, FdioCallerMoveAssignment) {
  fdio_cpp::FdioCaller caller(std::move(fd()));
  fdio_cpp::FdioCaller move_assignment_caller = std::move(caller);
  ASSERT_TRUE(move_assignment_caller);
  ASSERT_FALSE(caller);
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(move_assignment_caller));
}

TEST_F(FdioCallerTest, FdioCallerMoveConstructor) {
  fdio_cpp::FdioCaller caller(std::move(fd()));
  fdio_cpp::FdioCaller move_ctor_caller(std::move(caller));
  ASSERT_TRUE(move_ctor_caller);
  ASSERT_FALSE(caller);
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(move_ctor_caller));
}

TEST_F(FdioCallerTest, FdioCallerBorrow) {
  fdio_cpp::FdioCaller caller(std::move(fd()));
  zx::unowned_channel channel = caller.channel();
  ASSERT_TRUE(channel->is_valid());
  ASSERT_TRUE(caller);
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(channel));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller.node().channel()));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller.file().channel()));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller.directory().channel()));
}

TEST_F(FdioCallerTest, FdioCallerClone) {
  fdio_cpp::FdioCaller caller(std::move(fd()));
  auto channel = caller.clone_channel();
  ASSERT_OK(channel.status_value());
  ASSERT_TRUE(channel->is_valid());
  ASSERT_TRUE(caller);
  ASSERT_NE(caller.channel()->get(), channel->get());
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(channel->borrow()));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller.clone_node()->channel()));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller.clone_file()->channel()));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller.clone_directory()->channel()));
}

TEST_F(FdioCallerTest, FdioCallerTake) {
  fdio_cpp::FdioCaller caller(std::move(fd()));
  auto channel = caller.take_channel();
  ASSERT_OK(channel.status_value());
  ASSERT_TRUE(channel->is_valid());
  ASSERT_FALSE(caller);
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(channel->borrow()));
}

TEST_F(FdioCallerTest, FdioCallerTakeAs) {
  fdio_cpp::FdioCaller caller(std::move(fd()));
  auto channel = caller.take_as<fio::File>();
  ASSERT_OK(channel.status_value());
  ASSERT_TRUE(channel->is_valid());
  ASSERT_FALSE(caller);
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(channel->borrow()));
}

TEST_F(FdioCallerTest, UnownedFdioCaller) {
  fdio_cpp::UnownedFdioCaller caller(fd());
  ASSERT_TRUE(caller);
  ASSERT_TRUE(fd());
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller));
}

TEST_F(FdioCallerTest, UnownedFdioCallerBorrow) {
  fdio_cpp::UnownedFdioCaller caller(fd());
  zx::unowned_channel channel = caller.channel();
  ASSERT_TRUE(channel->is_valid());
  ASSERT_TRUE(caller);
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(channel));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller.node().channel()));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller.file().channel()));
  ASSERT_NO_FATAL_FAILURE(TryFilesystemOperations(caller.directory().channel()));
}

}  // namespace
