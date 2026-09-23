// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <signal.h>
#include <sys/poll.h>
#include <unistd.h>

#include <thread>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

TEST(PipeTest, NonBlockingPartialWrite) {
  // Allocate 1M that should be bigger than the pipe buffer.
  constexpr ssize_t kBufferSize = 1024 * 1024;

  int pipefd[2];
  SAFE_SYSCALL(pipe2(pipefd, O_NONBLOCK));

  char* buffer = static_cast<char*>(malloc(kBufferSize));
  ASSERT_NE(buffer, nullptr);
  ssize_t write_result = write(pipefd[1], buffer, kBufferSize);
  free(buffer);
  ASSERT_GT(write_result, 0);
  ASSERT_LT(write_result, kBufferSize);
}

TEST(PipeTest, BlockingSmallWrites) {
  // Create a pipe with size 4096, and fill all but 128 bytes of it.
  int pipefd[2];
  SAFE_SYSCALL(pipe2(pipefd, O_NONBLOCK));
  SAFE_SYSCALL(fcntl(pipefd[1], F_SETPIPE_SZ, getpagesize()));
  const int kWriteSize = getpagesize() - 128;
  char buf[kWriteSize];
  ASSERT_EQ(write(pipefd[1], buf, kWriteSize), kWriteSize);
  // Trying to write 256 bytes must returns EAGAIN
  ASSERT_EQ(write(pipefd[1], buf, 256), -1);
  ASSERT_EQ(errno, EAGAIN);
}

TEST(PipeTest, SpliceShortRead) {
  char* tmp = getenv("TEST_TMPDIR");
  std::string path = tmp == nullptr ? "/tmp/test_file" : std::string(tmp) + "/test_file";
  fbl::unique_fd fd(open(path.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0777));
  ASSERT_TRUE(fd.is_valid());
  ASSERT_EQ(write(fd.get(), "hello", 5), 5);
  int pipefd[2];
  SAFE_SYSCALL(pipe2(pipefd, 0));
  off64_t offset = 0;
  ASSERT_EQ(splice(fd.get(), &offset, pipefd[1], nullptr, 100, 0), 5);
  char buffer[100];
  ASSERT_EQ(read(pipefd[0], buffer, 10), 5);
  ASSERT_EQ(strncmp(buffer, "hello", 5), 0);
}

TEST(PipeTest, TeeFromEmptyPipe) {
  int pipe_a[2];
  SAFE_SYSCALL(pipe2(pipe_a, 0));
  // Closing the write end of pipe_a makes the read end readable even though it's empty.
  close(pipe_a[1]);
  int pipe_b[2];
  SAFE_SYSCALL(pipe2(pipe_b, 0));
  ASSERT_EQ(tee(pipe_a[0], pipe_b[1], 100, 0), 0);
  close(pipe_a[0]);
  close(pipe_b[0]);
  close(pipe_b[1]);
}

std::string CreateNewFifo() {
  char* tmp = getenv("TEST_TMPDIR");
  std::string dir_path = tmp == nullptr ? "/tmp/dirXXXXXX" : std::string(tmp) + "/dirXXXXXX";
  mkdtemp(&dir_path[0]);
  std::string fifo_path = dir_path + "/fifo";
  EXPECT_EQ(mkfifo(fifo_path.c_str(), 0600), 0);
  return fifo_path;
}

TEST(PipeTest, OpenFifoRW) {
  std::string path = CreateNewFifo();
  // Open and close the the fifo once.
  fbl::unique_fd fifo(open(path.c_str(), O_RDWR));
  ASSERT_TRUE(fifo.is_valid());
}

TEST(PipeTest, OpenFifoRo_NotBlocking) {
  std::string path = CreateNewFifo();
  // Reopen the fifo and check it is not disconnected.
  fbl::unique_fd fifo(open(path.c_str(), O_RDONLY | O_NONBLOCK));
  ASSERT_TRUE(fifo.is_valid());
}

void on_alarm(int) {}

TEST(PipeTest, OpenFifoBlock) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    std::string path = CreateNewFifo();
    struct sigaction act;
    act.sa_handler = on_alarm;
    act.sa_flags = 0;
    sigaction(SIGALRM, &act, nullptr);
    alarm(1);
    int fd = open(path.c_str(), O_RDONLY);
    ASSERT_EQ(fd, -1);
    ASSERT_EQ(errno, EINTR);
  });
}

TEST(PipeTest, PolloutFragmentation) {
  const size_t page_size = sysconf(_SC_PAGE_SIZE);
  std::vector<std::byte> page_sized_buffer(page_size, std::byte{0xAB});
  int pipe_fds[2];
  SAFE_SYSCALL(pipe2(pipe_fds, O_NONBLOCK));
  fbl::unique_fd pipe_rd(pipe_fds[0]);
  fbl::unique_fd pipe_wr(pipe_fds[1]);

  // Set the pipe buffer capacity to 2 pages.
  SAFE_SYSCALL(fcntl(pipe_wr.get(), F_SETPIPE_SZ, 2 * page_size));

  // Fill the first page of the pipe completely.
  ASSERT_EQ(static_cast<ssize_t>(page_size),
            write(pipe_wr.get(), page_sized_buffer.data(), page_sized_buffer.size()));

  // Write one more byte to occupy the start of the second page.
  ASSERT_EQ(1, write(pipe_wr.get(), page_sized_buffer.data(), 1));

  // Since there is not a full page size buffer available, POLLOUT is not asserted.
  pollfd p = {
      .fd = pipe_wr.get(),
      .events = POLLOUT,
  };
  EXPECT_EQ(poll(&p, 1, 0), 0);

  // Despite POLLOUT not being ready, writes that fit into the remaining portion of the second page
  // do not block.
  ASSERT_EQ(1, write(pipe_wr.get(), page_sized_buffer.data(), 1));

  // Attempts to write more data than will fit into the remaining portion would block.
  ASSERT_EQ(-1, write(pipe_wr.get(), page_sized_buffer.data(), page_size - 1));
  EXPECT_EQ(errno, EAGAIN);

  // Read out all but 1 byte from the first page of the pipe
  ASSERT_EQ(static_cast<ssize_t>(page_size - 1),
            read(pipe_rd.get(), page_sized_buffer.data(), page_size - 1));

  // Even though only 3 bytes are currently in the pipe, POLLOUT is still not ready.
  EXPECT_EQ(poll(&p, 1, 0), 0);

  // Read one more byte so that the first page is no longer occupied.
  ASSERT_EQ(1, read(pipe_rd.get(), page_sized_buffer.data(), 1));

  // Now POLLOUT will finally be asserted.
  EXPECT_EQ(poll(&p, 1, 0), 1);
  EXPECT_EQ(p.revents, POLLOUT);
}

// Verify that when the only writer (a child process) exits, the pipe is immediately closed even if
// the child is not yet reaped.
TEST(PipeTest, ZombieWriter) {
  int pipe_fds[2];
  // Create a non-blocking pipe so read won't block if the writer is still alive.
  SAFE_SYSCALL(pipe2(pipe_fds, O_NONBLOCK));
  fbl::unique_fd pipe_rd(pipe_fds[0]);
  fbl::unique_fd pipe_wr(pipe_fds[1]);

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([read_fd = pipe_fds[0]] {
    // Close read end so it is only a writer, then exit immediately.
    // This should also close the write end.
    close(read_fd);
  });

  // Close the parent's write end so the child is the only writer.
  // Then wait for the child to exit and enter the zombie state.
  pipe_wr.reset();
  ASSERT_TRUE(helper.WaitForChildren());

  // Read from the pipe.
  //
  // If the uniquely-owned file descriptor table was cleared correctly on child exit, the write-end
  // is closed, and read() should return 0 immediately.
  char buf[1];
  EXPECT_THAT(read(pipe_rd.get(), buf, sizeof(buf)), SyscallSucceedsWithValue(0));
}

// Verify that when a multi-threaded child process exits, the pipe is immediately closed when the
// last thread (the main thread, in this case) exits, even if spawned threads exited earlier.
TEST(PipeTest, ZombieMultiThreadedWriter) {
  int pipe_fds[2];
  SAFE_SYSCALL(pipe2(pipe_fds, O_NONBLOCK));
  fbl::unique_fd pipe_rd(pipe_fds[0]);
  fbl::unique_fd pipe_wr(pipe_fds[1]);

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([read_fd = pipe_fds[0]] {
    close(read_fd);

    // Spawn a thread that also shares the FD table.
    std::thread t([] {
      // Just exit immediately.
    });

    // Wait for the spawned thread to exit.
    // This decrements file descriptor table's share count to 1.
    t.join();

    // Now the main thread (the last thread) exits, decrementing the share count to 0. This should
    // immediately clear FDs.
  });

  pipe_wr.reset();
  ASSERT_TRUE(helper.WaitForChildren());

  char buf[1];
  EXPECT_THAT(read(pipe_rd.get(), buf, sizeof(buf)), SyscallSucceedsWithValue(0));
}

}  // namespace
