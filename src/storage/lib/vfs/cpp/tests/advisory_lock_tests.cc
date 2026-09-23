// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/result.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <algorithm>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <future>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/lib/fxl/strings/substitute.h"
#include "src/storage/lib/vfs/cpp/remote_dir.h"
#include "src/storage/memfs/mounted_memfs.h"

namespace {
constexpr char kTmpfsPath[] = "/fshost-flock-tmp";
constexpr char kFlockDir[] = "flock-dir";
constexpr char kTmpfsPathFile[] = "/fshost-flock-tmp/flock_smoke";
const ssize_t FILE_SIZE = 1024;

class FlockTest : public testing::Test {
 public:
  FlockTest() : memfs_loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {}

  void SetUp() override {
    ASSERT_EQ(memfs_loop_.StartThread(), ZX_OK);

    zx::result memfs = MountedMemfs::Create(memfs_loop_.dispatcher(), kTmpfsPath);
    ASSERT_EQ(memfs.status_value(), ZX_OK);
    memfs_.emplace(std::move(memfs.value()));
  }

  void TearDown() override {
    if (std::optional memfs = std::exchange(memfs_, std::nullopt); memfs.has_value()) {
      std::promise<zx_status_t> promise;
      memfs.value()->Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
      ASSERT_EQ(promise.get_future().get(), ZX_OK);
    }
  }

 protected:
  fbl::RefPtr<fs::RemoteDir> GetRemoteDir() {
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    EXPECT_EQ(ZX_OK, fdio_open3(kTmpfsPath,
                                static_cast<uint64_t>(fuchsia_io::wire::kPermReadable |
                                                      fuchsia_io::wire::kPermExecutable),
                                server.TakeChannel().release()));
    return fbl::MakeRefCounted<fs::RemoteDir>(std::move(client));
  }

  void AddFile(size_t content_size) {
    std::string contents(content_size, 'X');
    int fd = open(kTmpfsPathFile, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR);
    use_first_fd_ = true;  // first |GetFd| gets this one
    EXPECT_LT(-1, fd);
    fds_.push_back(fd);
    ASSERT_EQ(write(fd, contents.c_str(), content_size), static_cast<ssize_t>(content_size));
  }

  void CloseFile() {
    std::for_each(fds_.begin(), fds_.end(), &close);
    unlink(kTmpfsPathFile);
  }

  void MakeDir(const std::string& path) {
    ASSERT_EQ(0, mkdir(fxl::Substitute("$0/$1", kTmpfsPath, path).c_str(), 0666));
  }

  int GetFd() {
    if (use_first_fd_) {
      EXPECT_EQ(fds_.size(), 1u);
      use_first_fd_ = false;
      return fds_[0];
    }
    int fd = open(kTmpfsPathFile, O_RDWR);
    EXPECT_LT(-1, fd);
    return fd;
  }

 private:
  // File lock related.
  std::vector<int> fds_;
  bool use_first_fd_;

  async::Loop memfs_loop_;
  std::optional<MountedMemfs> memfs_;
};

TEST_F(FlockTest, FlockOnDir) {
  // Initialize test directory
  MakeDir(kFlockDir);

  int fd = open(fxl::Substitute("$0/$1", kTmpfsPath, kFlockDir).c_str(), O_RDONLY | O_DIRECTORY);
  ASSERT_NE(-1, fd);

  ASSERT_EQ(0, flock(fd, LOCK_EX));

  int fd2 = open(fxl::Substitute("$0/$1", kTmpfsPath, kFlockDir).c_str(), O_RDONLY | O_DIRECTORY);
  ASSERT_LT(-1, fd2);
  ASSERT_EQ(-1, flock(fd2, LOCK_EX | LOCK_NB));

  ASSERT_EQ(0, flock(fd, LOCK_UN));
  close(fd);
  close(fd2);
}

TEST_F(FlockTest, FlockExclusiveNoBlock) {
  AddFile(FILE_SIZE);
  int fd_a = GetFd();
  int fd_b = GetFd();

  EXPECT_EQ(0, flock(fd_a, LOCK_EX));
  EXPECT_EQ(-1, flock(fd_b, LOCK_EX | LOCK_NB));
  EXPECT_EQ(errno, EWOULDBLOCK);
  EXPECT_EQ(0, flock(fd_a, LOCK_UN));

  EXPECT_EQ(0, flock(fd_b, LOCK_EX));
  EXPECT_EQ(0, flock(fd_b, LOCK_UN));
  CloseFile();
}

TEST_F(FlockTest, FlockVsShare) {
  AddFile(FILE_SIZE);
  int fd_a = GetFd();
  int fd_b = GetFd();

  ASSERT_EQ(0, flock(fd_a, LOCK_SH));
  ASSERT_EQ(0, flock(fd_b, LOCK_SH));
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  ASSERT_EQ(0, flock(fd_b, LOCK_UN));
  CloseFile();
}

TEST_F(FlockTest, FlockLockUnlock) {
  AddFile(FILE_SIZE);
  int fd_a = GetFd();
  ASSERT_EQ(0, flock(fd_a, LOCK_SH));
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  ASSERT_EQ(0, flock(fd_a, LOCK_SH));
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  ASSERT_EQ(0, flock(fd_a, LOCK_EX));
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  ASSERT_EQ(0, flock(fd_a, LOCK_EX));
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  ASSERT_EQ(0, flock(fd_a, LOCK_SH));
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  CloseFile();
}

TEST_F(FlockTest, FlockTwoShared) {
  AddFile(FILE_SIZE);
  int fd_a = GetFd();
  int fd_b = GetFd();
  ASSERT_EQ(0, flock(fd_a, LOCK_SH));
  ASSERT_EQ(0, flock(fd_b, LOCK_SH));
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  ASSERT_EQ(0, flock(fd_b, LOCK_UN));
  CloseFile();
}

TEST_F(FlockTest, FlockSharedNoBlockExclusive) {
  AddFile(FILE_SIZE);
  int fd_a = GetFd();
  int fd_b = GetFd();

  ASSERT_EQ(0, flock(fd_a, LOCK_SH));
  ASSERT_EQ(-1, flock(fd_b, LOCK_EX | LOCK_NB));
  ASSERT_EQ(EWOULDBLOCK, errno);
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  CloseFile();
}

TEST_F(FlockTest, FlockExclusiveNoBlockShared) {
  AddFile(FILE_SIZE);
  int fd_a = GetFd();
  int fd_b = GetFd();

  ASSERT_EQ(0, flock(fd_a, LOCK_EX));
  ASSERT_EQ(-1, flock(fd_b, LOCK_SH | LOCK_NB));
  ASSERT_EQ(EWOULDBLOCK, errno);
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  CloseFile();
}

TEST_F(FlockTest, FlockExclusiveNoBlockExclusive) {
  AddFile(FILE_SIZE);
  int fd_a = GetFd();
  int fd_b = GetFd();

  ASSERT_EQ(0, flock(fd_a, LOCK_EX));
  ASSERT_EQ(-1, flock(fd_b, LOCK_EX | LOCK_NB));
  ASSERT_EQ(EWOULDBLOCK, errno);
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  CloseFile();
}

TEST_F(FlockTest, FlockExclusiveToShared) {
  AddFile(FILE_SIZE);
  int fd_a = GetFd();
  int fd_b = GetFd();

  ASSERT_EQ(0, flock(fd_a, LOCK_EX));
  ASSERT_EQ(-1, flock(fd_b, LOCK_SH | LOCK_NB));
  ASSERT_EQ(EWOULDBLOCK, errno);
  ASSERT_EQ(0, flock(fd_a, LOCK_SH));
  ASSERT_EQ(0, flock(fd_b, LOCK_SH));
  ASSERT_EQ(0, flock(fd_a, LOCK_UN));
  ASSERT_EQ(0, flock(fd_b, LOCK_UN));
  CloseFile();
}

TEST_F(FlockTest, FlockSharedToExclusive) {
  AddFile(FILE_SIZE);
  int fd_a = GetFd();
  int fd_b = GetFd();

  ASSERT_EQ(0, flock(fd_a, LOCK_SH));
  ASSERT_EQ(0, flock(fd_b, LOCK_SH));
  ASSERT_EQ(0, flock(fd_b, LOCK_UN));
  ASSERT_EQ(0, flock(fd_a, LOCK_EX));
  ASSERT_EQ(-1, flock(fd_b, LOCK_SH | LOCK_NB));
  ASSERT_EQ(EWOULDBLOCK, errno);
  CloseFile();
}
}  // namespace
