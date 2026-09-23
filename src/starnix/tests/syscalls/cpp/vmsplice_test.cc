// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fit/defer.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <unistd.h>

#include <cstdlib>
#include <string>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "test_helper.h"

namespace {

constexpr size_t kNumPipeEndsCount = 2;

void MakePipe(fbl::unique_fd (&fds)[kNumPipeEndsCount]) {
  int raw_fds[kNumPipeEndsCount] = {};
  ASSERT_EQ(pipe(raw_fds), 0) << strerror(errno);
  for (size_t i = 0; i < kNumPipeEndsCount; ++i) {
    fds[i] = fbl::unique_fd(raw_fds[i]);
    ASSERT_TRUE(fds[i]);
  }
}

TEST(VmspliceTest, VmspliceReadClosedPipe) {
  fbl::unique_fd fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(fds));
  ASSERT_EQ(fds[1].reset(), 0) << strerror(errno);
  char buffer[4096] = {};
  iovec iov = {
      .iov_base = buffer,
      .iov_len = sizeof(buffer),
  };
  ASSERT_EQ(vmsplice(fds[0].get(), &iov, 1, SPLICE_F_NONBLOCK), 0) << strerror(errno);
  ASSERT_EQ(vmsplice(fds[0].get(), &iov, 1, 0), 0) << strerror(errno);
}

TEST(VmspliceTest, VmspliceWriteClosedPipe) {
  constexpr int kRetCode = 42;
  EXPECT_EXIT(([]() {
                signal(SIGPIPE, SIG_IGN);

                fbl::unique_fd fds[kNumPipeEndsCount];
                ASSERT_NO_FATAL_FAILURE(MakePipe(fds));
                ASSERT_EQ(fds[0].reset(), 0) << strerror(errno);
                char buffer[4096] = {};
                iovec iov = {
                    .iov_base = buffer,
                    .iov_len = sizeof(buffer),
                };
                ASSERT_EQ(vmsplice(fds[1].get(), &iov, 1, SPLICE_F_NONBLOCK), -1);
                ASSERT_EQ(errno, EPIPE);
                ASSERT_EQ(vmsplice(fds[1].get(), &iov, 1, 0), -1);
                ASSERT_EQ(errno, EPIPE);
                exit(kRetCode);
              })(),
              testing::ExitedWithCode(kRetCode), "");
}

TEST(VmspliceTest, ModifyBufferInPipe) {
  constexpr char kInitialByte = 'A';
  constexpr char kModifiedByte = 'B';

  fbl::unique_fd fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(fds));
  // Volatile to let the compiler know not to optimize out the final write we
  // perform to `write_buffer` which is never directly read here.
  volatile char write_buffer[10];
  std::fill_n(&write_buffer[0], std::size(write_buffer), kInitialByte);
  {
    iovec iov = {
        .iov_base = const_cast<char*>(write_buffer),
        .iov_len = sizeof(write_buffer),
    };
    ASSERT_EQ(vmsplice(fds[1].get(), &iov, 1, 0), static_cast<ssize_t>(sizeof(write_buffer)))
        << strerror(errno);
  }
  std::fill_n(&write_buffer[0], std::size(write_buffer), kModifiedByte);

  char read_buffer[sizeof(write_buffer)] = {};
  ASSERT_EQ(read(fds[0].get(), read_buffer, sizeof(read_buffer)),
            static_cast<ssize_t>(sizeof(read_buffer)))
      << strerror(errno);
  EXPECT_THAT(read_buffer, testing::Each(kModifiedByte));
}

TEST(VmspliceTest, ForkWithBufferInPipe) {
  constexpr char kParentInitialByte = 'A';
  constexpr char kParentModifiedByte = 'B';
  constexpr char kChildInitialByte = 'C';
  constexpr char kChildModifiedByte = 'D';
  constexpr size_t kBufSize = 10;

  fbl::unique_fd fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(fds));

  fbl::unique_fd child_to_parent_pipe_fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(child_to_parent_pipe_fds));
  fbl::unique_fd parent_to_child_pipe_fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(parent_to_child_pipe_fds));

  auto signal_peer = [](const fbl::unique_fd& fd) {
    constexpr char kByte = 1;
    ASSERT_EQ(write(fd.get(), &kByte, sizeof(kByte)), static_cast<ssize_t>(sizeof(kByte)))
        << strerror(errno);
  };

  auto wait_for_peer = [](const fbl::unique_fd& fd) {
    char byte;
    ASSERT_EQ(read(fd.get(), &byte, sizeof(byte)), static_cast<ssize_t>(sizeof(byte)))
        << strerror(errno);
  };

  auto do_write = [&](const char initial_byte, const char modified_byte,
                      const fbl::unique_fd& signal_fd, const fbl::unique_fd& wait_fd) {
    // Volatile to let the compiler know not to optimize out the final write we
    // perform to `write_buffer` which is never directly read here.
    volatile char* write_buffer = reinterpret_cast<volatile char*>(
        mmap(nullptr, kBufSize, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    ASSERT_NE(write_buffer, MAP_FAILED) << strerror(errno);
    auto unmap_write_buffer = fit::defer([&]() {
      EXPECT_EQ(munmap(const_cast<char*>(write_buffer), kBufSize), 0) << strerror(errno);
    });
    std::fill_n(write_buffer, kBufSize, initial_byte);
    iovec iov = {
        .iov_base = const_cast<char*>(write_buffer),
        .iov_len = kBufSize,
    };
    ASSERT_EQ(vmsplice(fds[1].get(), &iov, 1, 0), static_cast<ssize_t>(kBufSize))
        << strerror(errno);
    ASSERT_NO_FATAL_FAILURE(signal_peer(signal_fd));

    // Wait for the peer to wmsplice a payload before we modify our payload.
    ASSERT_NO_FATAL_FAILURE(wait_for_peer(wait_fd));
    std::fill_n(write_buffer, kBufSize, modified_byte);
  };

  auto do_read = [&](const char peer_expected_byte) {
    char read_buffer[kBufSize] = {};
    ASSERT_EQ(read(fds[0].get(), read_buffer, sizeof(read_buffer)),
              static_cast<ssize_t>(sizeof(read_buffer)))
        << strerror(errno);
    EXPECT_THAT(read_buffer, testing::Each(peer_expected_byte));
  };

  // Flow of the test which lets a parent and child vmsplice/modify payloads
  // to the same pipe then make sure that they are able to observe their peer's
  // modified payloads:
  //   - Child starts blocked.
  //   - Parent vmsplice, unblock child vmsplice, blocked to modify.
  //   - Child vmsplice, unblock parent modify, blocked to modify.
  //   - Parent modify, unblock child modify, blocked to read.
  //   - Child modify, do read, unblock parent read.
  //   - Parent do read.

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    // Wait for parent to write first.
    ASSERT_NO_FATAL_FAILURE(wait_for_peer(parent_to_child_pipe_fds[0]));

    ASSERT_NO_FATAL_FAILURE(do_write(kChildInitialByte, kChildModifiedByte,
                                     child_to_parent_pipe_fds[1], parent_to_child_pipe_fds[0]));
    ASSERT_NO_FATAL_FAILURE(do_read(kParentModifiedByte));
    ASSERT_NO_FATAL_FAILURE(signal_peer(child_to_parent_pipe_fds[1]));
  });

  ASSERT_NO_FATAL_FAILURE(do_write(kParentInitialByte, kParentModifiedByte,
                                   parent_to_child_pipe_fds[1], child_to_parent_pipe_fds[0]));
  // Unblock the child from modifying the vmspliced payload.
  ASSERT_NO_FATAL_FAILURE(signal_peer(parent_to_child_pipe_fds[1]));
  // Wait for the child to read first.
  ASSERT_NO_FATAL_FAILURE(wait_for_peer(child_to_parent_pipe_fds[0]));
  ASSERT_NO_FATAL_FAILURE(do_read(kChildModifiedByte));
}

TEST(VmspliceTest, FileInPipe) {
  constexpr char kInitialByte = 'A';
  constexpr char kFirstModifiedByte = 'B';
  constexpr char kLastModifiedByte = 'C';
  constexpr size_t kFileSize = 10;

  char path[] = "/tmp/tmpfile.XXXXXX";
  fbl::unique_fd tmp_file = fbl::unique_fd(mkstemp(path));
  ASSERT_TRUE(tmp_file) << strerror(errno);

  auto write_to_file = [&](const fbl::unique_fd& fd, char c) {
    char write_buf[kFileSize];
    std::fill_n(&write_buf[0], std::size(write_buf), c);
    ASSERT_EQ(pwrite(fd.get(), write_buf, sizeof(write_buf), 0),
              static_cast<ssize_t>(sizeof(write_buf)))
        << strerror(errno);
  };
  ASSERT_NO_FATAL_FAILURE(write_to_file(tmp_file, kInitialByte));

  fbl::unique_fd fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(fds));
  {
    void* write_buffer = mmap(nullptr, kFileSize, PROT_READ, MAP_PRIVATE, tmp_file.get(), 0);
    ASSERT_NE(write_buffer, MAP_FAILED) << strerror(errno);
    auto unmap_write_buffer =
        fit::defer([&]() { EXPECT_EQ(munmap(write_buffer, kFileSize), 0) << strerror(errno); });

    iovec iov = {
        .iov_base = write_buffer,
        .iov_len = kFileSize,
    };
    for (int i = 0; i < 6; ++i) {
      ASSERT_EQ(vmsplice(fds[1].get(), &iov, 1, 0), static_cast<ssize_t>(kFileSize))
          << strerror(errno);
    }
  }

  auto read_from_file = [&](const fbl::unique_fd& fd, char c) {
    char read_buffer[kFileSize] = {};
    ASSERT_EQ(read(fd.get(), read_buffer, sizeof(read_buffer)),
              static_cast<ssize_t>(sizeof(read_buffer)))
        << strerror(errno);
    EXPECT_THAT(read_buffer, testing::Each(c));
  };
  ASSERT_NO_FATAL_FAILURE(read_from_file(fds[0], kInitialByte));

  // Update the file contents and expect that the payload sitting in the pipe
  // reflects the write, even after the FD is closed.
  ASSERT_NO_FATAL_FAILURE(write_to_file(tmp_file, kFirstModifiedByte));
  ASSERT_NO_FATAL_FAILURE(read_from_file(fds[0], kFirstModifiedByte));
  tmp_file.reset();
  ASSERT_NO_FATAL_FAILURE(read_from_file(fds[0], kFirstModifiedByte));

  // Update the file contents through a new FD, and expect that the payload
  // sitting in the pipe reflects the write, even after the FD is closed.
  tmp_file = fbl::unique_fd(open(path, O_WRONLY));
  ASSERT_TRUE(tmp_file) << strerror(errno);
  ASSERT_NO_FATAL_FAILURE(write_to_file(tmp_file, kLastModifiedByte));
  ASSERT_NO_FATAL_FAILURE(read_from_file(fds[0], kLastModifiedByte));
  tmp_file.reset();
  ASSERT_NO_FATAL_FAILURE(read_from_file(fds[0], kLastModifiedByte));

  // Remove the file and expect the payload sitting in the pipe to still be
  // valid for reading. This implies that pages allocated for a file will
  // be kept in memory, even after the file is removed, while it sits in the
  // pipe.
  ASSERT_EQ(remove(path), 0) << strerror(errno);
  ASSERT_EQ(access(path, 0), -1);
  ASSERT_EQ(errno, ENOENT);
  ASSERT_NO_FATAL_FAILURE(read_from_file(fds[0], kLastModifiedByte));
}

TEST(VmspliceTest, UnmapBufferInPipe) {
  constexpr size_t kMmapSize = 4096;
  constexpr char kInitialByte = 'A';
  constexpr char kModifiedByte = 'B';

  fbl::unique_fd fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(fds));
  {
    // Volatile to let the compiler know not to optimize out the final write we
    // perform to `write_buffer` which is never directly read here.
    volatile char* write_buffer = reinterpret_cast<volatile char*>(
        mmap(nullptr, kMmapSize, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    ASSERT_NE(write_buffer, MAP_FAILED) << strerror(errno);
    auto unmap_write_buffer = fit::defer([&]() {
      EXPECT_EQ(munmap(const_cast<char*>(write_buffer), kMmapSize), 0) << strerror(errno);
    });
    std::fill_n(write_buffer, kMmapSize, kInitialByte);
    {
      iovec iov = {
          .iov_base = const_cast<char*>(write_buffer),
          .iov_len = kMmapSize,
      };
      ASSERT_EQ(vmsplice(fds[1].get(), &iov, 1, 0), static_cast<ssize_t>(kMmapSize))
          << strerror(errno);
    }
    std::fill_n(write_buffer, kMmapSize, kModifiedByte);
  }

  char read_buffer[kMmapSize] = {};
  ASSERT_EQ(read(fds[0].get(), read_buffer, sizeof(read_buffer)),
            static_cast<ssize_t>(sizeof(read_buffer)))
      << strerror(errno);
  EXPECT_THAT(read_buffer, testing::Each(kModifiedByte));
}

TEST(VmspliceTest, UnmapBufferInPipeThenMapInPlace) {
  constexpr size_t kMmapSize = 4096;
  constexpr char kInitialByte = 'A';
  constexpr char kModifiedByte = 'B';

  fbl::unique_fd fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(fds));
  // Volatile to let the compiler know not to optimize out the final write we
  // perform to `write_buffer` which is never directly read here.
  volatile char* write_buffer = reinterpret_cast<volatile char*>(
      mmap(nullptr, kMmapSize, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  {
    ASSERT_NE(write_buffer, MAP_FAILED) << strerror(errno);
    auto unmap_write_buffer = fit::defer([&]() {
      EXPECT_EQ(munmap(const_cast<char*>(write_buffer), kMmapSize), 0) << strerror(errno);
    });
    std::fill_n(write_buffer, kMmapSize, kInitialByte);
    {
      iovec iov = {
          .iov_base = const_cast<char*>(write_buffer),
          .iov_len = kMmapSize,
      };
      ASSERT_EQ(vmsplice(fds[1].get(), &iov, 1, 0), static_cast<ssize_t>(kMmapSize))
          << strerror(errno);
    }
    std::fill_n(write_buffer, kMmapSize, kModifiedByte);
  }

  {
    // Volatile to let the compiler know not to optimize out the final write we
    // perform to `write_buffer_new` which is never read here.
    volatile char* write_buffer_new = reinterpret_cast<volatile char*>(
        mmap(const_cast<char*>(write_buffer), kMmapSize, PROT_READ | PROT_WRITE,
             MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_NE(write_buffer_new, MAP_FAILED) << strerror(errno);
    ASSERT_EQ(write_buffer, write_buffer_new);
    auto unmap_write_buffer_new = fit::defer([&]() {
      EXPECT_EQ(munmap(const_cast<char*>(write_buffer_new), kMmapSize), 0) << strerror(errno);
    });
    std::fill_n(write_buffer_new, kMmapSize, kInitialByte);
  }

  char read_buffer[kMmapSize] = {};
  ASSERT_EQ(read(fds[0].get(), read_buffer, sizeof(read_buffer)),
            static_cast<ssize_t>(sizeof(read_buffer)))
      << strerror(errno);
  EXPECT_THAT(read_buffer, testing::Each(kModifiedByte));
}

class VmspliceRemapNewMemoryOverBufferInPipeTest : public testing::TestWithParam<bool> {};

TEST_P(VmspliceRemapNewMemoryOverBufferInPipeTest, RemapNewMemoryOverBufferInPipe) {
  constexpr char kInitialByte = 'A';
  constexpr char kModifiedByte = 'B';
  constexpr size_t kMmapSize = 4096;

  fbl::unique_fd fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(fds));
  // Volatile to let the compiler know not to optimize out the final write we
  // perform to `write_buffer` which is never read here.
  volatile char* write_buffer = reinterpret_cast<volatile char*>(
      mmap(nullptr, kMmapSize, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  ASSERT_NE(write_buffer, MAP_FAILED) << strerror(errno);
  auto unmap_write_buffer = fit::defer([&]() {
    EXPECT_EQ(munmap(const_cast<char*>(write_buffer), kMmapSize), 0) << strerror(errno);
  });
  std::fill_n(write_buffer, kMmapSize, kInitialByte);
  {
    iovec iov = {
        .iov_base = const_cast<char*>(write_buffer),
        .iov_len = kMmapSize,
    };
    ASSERT_EQ(vmsplice(fds[1].get(), &iov, 1, 0), static_cast<ssize_t>(kMmapSize))
        << strerror(errno);
  }

  {
    // Volatile to let the compiler know not to optimize out the final write we
    // perform to `write_buffer_new` which is never read here.
    volatile char* write_buffer_new = reinterpret_cast<volatile char*>(
        mmap(nullptr, kMmapSize, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    ASSERT_NE(write_buffer_new, MAP_FAILED) << strerror(errno);
    ASSERT_NE(write_buffer_new, write_buffer);
    auto unmap_write_buffer_new = fit::defer([&]() {
      EXPECT_EQ(munmap(const_cast<char*>(write_buffer_new), kMmapSize), 0) << strerror(errno);
    });
    std::fill_n(write_buffer_new, kMmapSize, kModifiedByte);

    void* remapped_addr = mremap(const_cast<char*>(write_buffer_new), kMmapSize, kMmapSize,
                                 MREMAP_MAYMOVE | MREMAP_FIXED, const_cast<char*>(write_buffer));
    ASSERT_NE(remapped_addr, MAP_FAILED) << strerror(errno);
    ASSERT_EQ(remapped_addr, write_buffer);
    // The unmapping will be handled by `unmap_write_buffer`.
    unmap_write_buffer_new.cancel();
  }

  if (GetParam()) {
    // This write is performed on the _new_ mapping, not the old one which is
    // "shared" with the paylaod sitting in the pipe buffer. Even though we no
    // longer hold a reference to the private anon mapping, the kernel continues
    // to keep it alive until it is read from the pipe.
    std::fill_n(write_buffer, kMmapSize, kModifiedByte);
  }

  char read_buffer[kMmapSize] = {};
  ASSERT_EQ(read(fds[0].get(), read_buffer, sizeof(read_buffer)),
            static_cast<ssize_t>(sizeof(read_buffer)))
      << strerror(errno);
  EXPECT_THAT(read_buffer, testing::Each(kInitialByte));
}

void AliasSharedMapping(volatile char* addr, size_t size, volatile char** alias) {
  // First allocate a mapping that will accommodate our alias mapping. This is
  // where the original mapping will alias from.
  void* remap_target = mmap(nullptr, size, 0, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(remap_target, MAP_FAILED) << strerror(errno);
  // The aliased mapping obviously can't be the same as our original mapping.
  ASSERT_NE(remap_target, addr);
  auto unmap_remap_target =
      fit::defer([&]() { EXPECT_EQ(munmap(remap_target, size), 0) << strerror(errno); });

  void* remapped_addr =
      mremap(const_cast<char*>(addr), 0, size, MREMAP_MAYMOVE | MREMAP_FIXED, remap_target);
  ASSERT_NE(remapped_addr, MAP_FAILED) << strerror(errno);
  ASSERT_EQ(remapped_addr, remap_target);

  unmap_remap_target.cancel();
  *alias = reinterpret_cast<volatile char*>(remapped_addr);
}

INSTANTIATE_TEST_SUITE_P(VmspliceTest, VmspliceRemapNewMemoryOverBufferInPipeTest,
                         testing::Values(true, false));

TEST(VmspliceTest, RemapSharedBufferInPipeThenModify) {
  constexpr char kInitialByte = 'A';
  constexpr char kModifiedByte = 'B';
  constexpr size_t kMmapSize = 4096;

  fbl::unique_fd fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(fds));
  // Volatile to let the compiler know not to optimize out the final write we
  // perform to `write_buffer` which is never read here.
  volatile char* write_buffer = reinterpret_cast<volatile char*>(
      mmap(nullptr, kMmapSize, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0));
  ASSERT_NE(write_buffer, MAP_FAILED) << strerror(errno);
  auto unmap_write_buffer = fit::defer([&]() {
    EXPECT_EQ(munmap(const_cast<char*>(write_buffer), kMmapSize), 0) << strerror(errno);
  });
  std::fill_n(write_buffer, kMmapSize, kInitialByte);
  {
    iovec iov = {
        .iov_base = const_cast<char*>(write_buffer),
        .iov_len = kMmapSize,
    };
    ASSERT_EQ(vmsplice(fds[1].get(), &iov, 1, 0), static_cast<ssize_t>(kMmapSize))
        << strerror(errno);
  }

  {
    // Volatile to let the compiler know not to optimize out the final write we
    // perform to `write_buffer_new` which is never read here.
    volatile char* write_buffer_new;
    ASSERT_NO_FATAL_FAILURE(AliasSharedMapping(write_buffer, kMmapSize, &write_buffer_new));
    auto unmap_write_buffer_new = fit::defer([&]() {
      EXPECT_EQ(munmap(const_cast<char*>(write_buffer_new), kMmapSize), 0) << strerror(errno);
    });

    // `write_buffer_new` and `write_buffer` should point to the same memory even
    // though they are different virtual addresses.
    ASSERT_NE(write_buffer_new, write_buffer);
    std::fill_n(write_buffer_new, kMmapSize, kModifiedByte);
    std::for_each_n(write_buffer, kMmapSize, [&](char b) { EXPECT_EQ(b, kModifiedByte); });
  }

  char read_buffer[kMmapSize] = {};
  ASSERT_EQ(read(fds[0].get(), read_buffer, sizeof(read_buffer)),
            static_cast<ssize_t>(sizeof(read_buffer)))
      << strerror(errno);
  EXPECT_THAT(read_buffer, testing::Each(kModifiedByte));
}

template <typename U>
void VmspliceUnmapThenModify(fbl::unique_fd& fd, volatile char* addr, volatile char* other_addr,
                             size_t size, char new_byte, U& unmap) {
  iovec iov = {
      .iov_base = const_cast<char*>(addr),
      .iov_len = size,
  };
  ASSERT_EQ(vmsplice(fd.get(), &iov, 1, 0), static_cast<ssize_t>(size)) << strerror(errno);
  unmap.call();
  std::fill_n(other_addr, size, new_byte);
}

class VmspliceUnmapBufferAfterVmsplicingRemappedBufferTest : public testing::TestWithParam<bool> {};

TEST_P(VmspliceUnmapBufferAfterVmsplicingRemappedBufferTest,
       UnmapBufferAfterVmsplicingRemappedBuffer) {
  constexpr char kInitialByte = 'A';
  constexpr char kModifiedByte = 'B';
  constexpr size_t kMmapSize = 4096;

  fbl::unique_fd fds[kNumPipeEndsCount];
  ASSERT_NO_FATAL_FAILURE(MakePipe(fds));
  // Volatile to let the compiler know not to optimize out the final write we
  // perform to `write_buffer` which is never read here.
  volatile char* write_buffer = reinterpret_cast<volatile char*>(
      mmap(nullptr, kMmapSize, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0));
  ASSERT_NE(write_buffer, MAP_FAILED) << strerror(errno);
  auto unmap_write_buffer = fit::defer([&]() {
    EXPECT_EQ(munmap(const_cast<char*>(write_buffer), kMmapSize), 0) << strerror(errno);
  });
  std::fill_n(write_buffer, kMmapSize, kInitialByte);

  // Volatile to let the compiler know not to optimize out the final write we
  // perform to `write_buffer_new` which is never read here.
  volatile char* write_buffer_new;
  ASSERT_NO_FATAL_FAILURE(AliasSharedMapping(write_buffer, kMmapSize, &write_buffer_new));
  auto unmap_write_buffer_new = fit::defer([&]() {
    EXPECT_EQ(munmap(const_cast<char*>(write_buffer_new), kMmapSize), 0) << strerror(errno);
  });

  // `write_buffer_new` and `write_buffer` should point to the same memory even
  // though they are different virtual addresses.
  ASSERT_NE(write_buffer_new, write_buffer);
  std::for_each_n(write_buffer_new, kMmapSize, [&](char b) { EXPECT_EQ(b, kInitialByte); });

  // Vmsplice one of the mappings, unmap the vmspliced region then modify the memory
  // through the other region.
  if (GetParam()) {
    ASSERT_NO_FATAL_FAILURE(VmspliceUnmapThenModify(fds[1], write_buffer, write_buffer_new,
                                                    kMmapSize, kModifiedByte, unmap_write_buffer));
  } else {
    ASSERT_NO_FATAL_FAILURE(VmspliceUnmapThenModify(
        fds[1], write_buffer_new, write_buffer, kMmapSize, kModifiedByte, unmap_write_buffer_new));
  }

  char read_buffer[kMmapSize] = {};
  ASSERT_EQ(read(fds[0].get(), read_buffer, sizeof(read_buffer)),
            static_cast<ssize_t>(sizeof(read_buffer)))
      << strerror(errno);
  EXPECT_THAT(read_buffer, testing::Each(kModifiedByte));
}

INSTANTIATE_TEST_SUITE_P(VmspliceTest, VmspliceUnmapBufferAfterVmsplicingRemappedBufferTest,
                         testing::Values(true, false));

}  // namespace
