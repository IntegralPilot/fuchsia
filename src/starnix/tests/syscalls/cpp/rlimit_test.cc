// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/types.h>
#include <unistd.h>

#include <atomic>
#include <thread>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

std::atomic<size_t> g_signal_count = 0;

void handle_sigxfsz(int signum) { g_signal_count += 1; }

TEST(SetRLimitTest, ZeroFSizeOnRegularFiles) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    signal(SIGXFSZ, handle_sigxfsz);

    struct rlimit limit = {
        .rlim_cur = 0,
        .rlim_max = 0,
    };

    char path_template[] = "/tmp/XXXXXX";
    char* tmp_path = mkdtemp(path_template);
    ASSERT_NE(tmp_path, nullptr) << "mkdtemp failed" << std::strerror(errno) << '\n';
    std::string file_path = std::string(tmp_path) + "/regular_file";

    ASSERT_EQ(setrlimit(RLIMIT_FSIZE, &limit), 0)
        << "setrlimit failed" << std::strerror(errno) << '\n';

    fbl::unique_fd fd(creat(file_path.c_str(), 0666));
    ASSERT_TRUE(fd.is_valid()) << "failed to create file" << std::strerror(errno) << '\n';

    uint8_t buf[0x20] = {0};
    EXPECT_EQ(write(fd.get(), buf, sizeof(buf)), -1);
    EXPECT_EQ(errno, EFBIG);
    EXPECT_EQ(g_signal_count, 1u);
    signal(SIGXFSZ, SIG_DFL);
    ASSERT_EQ(unlink(file_path.c_str()), 0) << "unlink failed" << std::strerror(errno) << '\n';
    ASSERT_EQ(rmdir(tmp_path), 0) << "rmdir failed" << std::strerror(errno) << '\n';
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(SetRLimitTest, ZeroFSizeOnMemFd) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    signal(SIGXFSZ, handle_sigxfsz);

    struct rlimit limit = {
        .rlim_cur = 0,
        .rlim_max = 0,
    };

    ASSERT_EQ(setrlimit(RLIMIT_FSIZE, &limit), 0)
        << "setrlimit failed" << std::strerror(errno) << '\n';

    fbl::unique_fd fd(test_helper::MemFdCreate("memfd", 0));
    ASSERT_TRUE(fd.is_valid()) << "failed to create file" << std::strerror(errno) << '\n';

    uint8_t buf[0x20] = {0};
    EXPECT_EQ(write(fd.get(), buf, sizeof(buf)), -1);
    EXPECT_EQ(errno, EFBIG);
    EXPECT_EQ(g_signal_count, 1u);
    signal(SIGXFSZ, SIG_DFL);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

// Test that we can write up to one byte to files.
TEST(SetRLimitTest, OneFSize) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    signal(SIGXFSZ, handle_sigxfsz);

    struct rlimit limit = {
        .rlim_cur = 1,
        .rlim_max = 1,
    };

    ASSERT_EQ(setrlimit(RLIMIT_FSIZE, &limit), 0)
        << "setrlimit failed" << std::strerror(errno) << '\n';

    fbl::unique_fd fd(test_helper::MemFdCreate("memfd", 0));
    ASSERT_TRUE(fd.is_valid()) << "failed to create file" << std::strerror(errno) << '\n';

    uint8_t buf[0x1] = {0};
    EXPECT_EQ(write(fd.get(), buf, sizeof(buf)), 1);
    EXPECT_EQ(g_signal_count, 0u);

    // Next write should fail.
    EXPECT_EQ(write(fd.get(), buf, sizeof(buf)), -1);
    EXPECT_EQ(errno, EFBIG);
    EXPECT_EQ(g_signal_count, 1u);
    signal(SIGXFSZ, SIG_DFL);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(SetRLimitTest, ZeroFSizeOnPipe) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    signal(SIGXFSZ, handle_sigxfsz);

    struct rlimit limit = {
        .rlim_cur = 0,
        .rlim_max = 0,
    };

    ASSERT_EQ(setrlimit(RLIMIT_FSIZE, &limit), 0)
        << "setrlimit failed" << std::strerror(errno) << '\n';

    int pipefd[2];
    EXPECT_EQ(0, pipe2(pipefd, 0)) << "failed to create pipe" << std::strerror(errno) << '\n';

    uint8_t buf[0x1] = {0};
    ASSERT_EQ(write(pipefd[1], buf, sizeof(buf)), 1);
    EXPECT_EQ(g_signal_count, 0u);

    EXPECT_EQ(read(pipefd[0], buf, sizeof(buf)), 1);
    signal(SIGXFSZ, SIG_DFL);

    close(pipefd[0]);
    close(pipefd[1]);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(SetRLimitTest, ZeroFSizeOnFIFO) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    signal(SIGXFSZ, handle_sigxfsz);

    struct rlimit limit = {
        .rlim_cur = 0,
        .rlim_max = 0,
    };

    char path_template[] = "/tmp/XXXXXX";
    char* tmp_path = mkdtemp(path_template);
    ASSERT_NE(tmp_path, nullptr) << "mkdtemp failed" << std::strerror(errno) << '\n';
    std::string file_path = std::string(tmp_path) + "/regular_file";

    ASSERT_EQ(setrlimit(RLIMIT_FSIZE, &limit), 0)
        << "setrlimit failed" << std::strerror(errno) << '\n';

    ASSERT_EQ(0, mkfifo(file_path.c_str(), 0666))
        << "failed to create fifo" << std::strerror(errno) << '\n';

    std::thread reader([file_path]() {
      fbl::unique_fd fd(open(file_path.c_str(), O_RDONLY));
      ASSERT_TRUE(fd.is_valid()) << "failed to open file for reading" << std::strerror(errno)
                                 << '\n';

      uint8_t buf[0x1] = {0};
      EXPECT_EQ(read(fd.get(), buf, sizeof(buf)), 1)
          << "read failed" << std::strerror(errno) << '\n';
    });

    fbl::unique_fd fd(open(file_path.c_str(), O_WRONLY));
    ASSERT_TRUE(fd.is_valid()) << "failed to file for writing" << std::strerror(errno) << '\n';

    uint8_t buf[0x1] = {0};
    ASSERT_EQ(write(fd.get(), buf, sizeof(buf)), 1);

    reader.join();

    EXPECT_EQ(g_signal_count, 0u);
    signal(SIGXFSZ, SIG_DFL);

    ASSERT_EQ(unlink(file_path.c_str()), 0) << "unlink failed" << std::strerror(errno) << '\n';
    ASSERT_EQ(rmdir(tmp_path), 0) << "rmdir failed" << std::strerror(errno) << '\n';
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(SetRLimitTest, SpliceToRegularFileWithZeroFSize) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    signal(SIGXFSZ, handle_sigxfsz);

    struct rlimit limit = {
        .rlim_cur = 0,
        .rlim_max = 0,
    };

    ASSERT_EQ(setrlimit(RLIMIT_FSIZE, &limit), 0)
        << "setrlimit failed" << std::strerror(errno) << '\n';

    int pipefd[2];
    ASSERT_EQ(pipe2(pipefd, 0), 0) << "pipe2 failed: " << std::strerror(errno) << '\n';
    fbl::unique_fd pipe_in(pipefd[0]);
    fbl::unique_fd pipe_out(pipefd[1]);

    uint8_t buf[0x20] = {0};
    ASSERT_EQ(write(pipe_out.get(), buf, sizeof(buf)), static_cast<ssize_t>(sizeof(buf)));

    fbl::unique_fd fd(test_helper::MemFdCreate("memfd", 0));
    ASSERT_TRUE(fd.is_valid()) << "failed to create file: " << std::strerror(errno) << '\n';

    EXPECT_EQ(splice(pipe_in.get(), nullptr, fd.get(), nullptr, sizeof(buf), 0), -1);
    EXPECT_EQ(errno, EFBIG);
    EXPECT_EQ(g_signal_count, 1u);

    signal(SIGXFSZ, SIG_DFL);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(SetRLimitTest, SpliceToRegularFileTruncatedByFSize) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    signal(SIGXFSZ, handle_sigxfsz);

    struct rlimit limit = {
        .rlim_cur = 10,
        .rlim_max = 10,
    };

    ASSERT_EQ(setrlimit(RLIMIT_FSIZE, &limit), 0)
        << "setrlimit failed" << std::strerror(errno) << '\n';

    int pipefd[2];
    ASSERT_EQ(pipe2(pipefd, 0), 0) << "pipe2 failed: " << std::strerror(errno) << '\n';
    fbl::unique_fd pipe_in(pipefd[0]);
    fbl::unique_fd pipe_out(pipefd[1]);

    uint8_t buf[20] = {0};
    ASSERT_EQ(write(pipe_out.get(), buf, sizeof(buf)), 20);

    fbl::unique_fd fd(test_helper::MemFdCreate("memfd", 0));
    ASSERT_TRUE(fd.is_valid()) << "failed to create file: " << std::strerror(errno) << '\n';

    // Splice requested 20 bytes, but RLIMIT_FSIZE is 10. Should splice 10 bytes and deliver
    // SIGXFSZ.
    EXPECT_EQ(splice(pipe_in.get(), nullptr, fd.get(), nullptr, 20, 0), 10);
    EXPECT_EQ(g_signal_count, 1u);

    // Write more data into the pipe so the pipe is not empty.
    ASSERT_EQ(write(pipe_out.get(), buf, 10), 10);

    // Further splice starts at offset 10, which is equal to RLIMIT_FSIZE 10. Should fail with EFBIG
    // and SIGXFSZ.
    EXPECT_EQ(splice(pipe_in.get(), nullptr, fd.get(), nullptr, 10, 0), -1);
    EXPECT_EQ(errno, EFBIG);
    EXPECT_EQ(g_signal_count, 2u);

    signal(SIGXFSZ, SIG_DFL);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(SetRLimitTest, SpliceToRegularFileWithExplicitOffset) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    signal(SIGXFSZ, handle_sigxfsz);

    struct rlimit limit = {
        .rlim_cur = 10,
        .rlim_max = 10,
    };

    ASSERT_EQ(setrlimit(RLIMIT_FSIZE, &limit), 0)
        << "setrlimit failed" << std::strerror(errno) << '\n';

    int pipefd[2];
    ASSERT_EQ(pipe2(pipefd, 0), 0) << "pipe2 failed: " << std::strerror(errno) << '\n';
    fbl::unique_fd pipe_in(pipefd[0]);
    fbl::unique_fd pipe_out(pipefd[1]);

    uint8_t buf[20] = {0};
    ASSERT_EQ(write(pipe_out.get(), buf, sizeof(buf)), 20);

    fbl::unique_fd fd(test_helper::MemFdCreate("memfd", 0));
    ASSERT_TRUE(fd.is_valid()) << "failed to create file: " << std::strerror(errno) << '\n';

    loff_t off_out = 5;
    // Splice at offset 5 with limit 10: should splice 10 - 5 = 5 bytes, updating off_out to 10 and
    // delivering SIGXFSZ.
    EXPECT_EQ(splice(pipe_in.get(), nullptr, fd.get(), &off_out, 15, 0), 5);
    EXPECT_EQ(off_out, 10);
    EXPECT_EQ(g_signal_count, 1u);

    // Write more data into the pipe so the pipe is not empty.
    ASSERT_EQ(write(pipe_out.get(), buf, 10), 10);

    // Further splice at offset 10 (off_out == 10) >= limit 10. Should fail with EFBIG.
    EXPECT_EQ(splice(pipe_in.get(), nullptr, fd.get(), &off_out, 10, 0), -1);
    EXPECT_EQ(errno, EFBIG);
    EXPECT_EQ(g_signal_count, 2u);

    signal(SIGXFSZ, SIG_DFL);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(SetRLimitTest, SpliceFromRegularFileToPipeWithZeroFSize) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    signal(SIGXFSZ, handle_sigxfsz);

    fbl::unique_fd fd(test_helper::MemFdCreate("memfd", 0));
    ASSERT_TRUE(fd.is_valid()) << "failed to create file: " << std::strerror(errno) << '\n';

    uint8_t buf[10] = {0};
    ASSERT_EQ(write(fd.get(), buf, sizeof(buf)), 10);
    ASSERT_EQ(lseek(fd.get(), 0, SEEK_SET), 0);

    struct rlimit limit = {
        .rlim_cur = 0,
        .rlim_max = 0,
    };

    ASSERT_EQ(setrlimit(RLIMIT_FSIZE, &limit), 0)
        << "setrlimit failed" << std::strerror(errno) << '\n';

    int pipefd[2];
    ASSERT_EQ(pipe2(pipefd, 0), 0) << "pipe2 failed: " << std::strerror(errno) << '\n';
    fbl::unique_fd pipe_in(pipefd[0]);
    fbl::unique_fd pipe_out(pipefd[1]);

    // Splice from regular file to pipe when RLIMIT_FSIZE is 0 should succeed.
    EXPECT_EQ(splice(fd.get(), nullptr, pipe_out.get(), nullptr, 10, 0), 10);
    EXPECT_EQ(g_signal_count, 0u);

    signal(SIGXFSZ, SIG_DFL);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

}  //  namespace
