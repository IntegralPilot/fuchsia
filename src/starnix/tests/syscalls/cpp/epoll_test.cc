// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <signal.h>
#include <sys/epoll.h>
#include <sys/poll.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/timerfd.h>
#include <unistd.h>

#include <ctime>
#include <thread>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

// Our Linux sysroot doesn't seem to have tgkill() and gettid().
void DoTgkill(int tgid, int tid, int sig) { syscall(SYS_tgkill, tgid, tid, sig); }
pid_t DoGetTid() { return static_cast<pid_t>(syscall(SYS_gettid)); }

void NoOpSigHandler(int) {}

int EpollAdd(int epfd, int to_watch, uint32_t events, uint64_t data) {
  struct epoll_event event;
  event.events = events;
  event.data.u64 = data;
  return epoll_ctl(epfd, EPOLL_CTL_ADD, to_watch, &event);
}

// Implements the backend thread that responds to 1-byte socket messages for the LotsaSignals test.
// Quits when one of the sockets receives a 'q'.
void LotsaSignals_DoPong(int main_tid, int* socks) {
  fbl::unique_fd epfd(epoll_create(2));
  ASSERT_TRUE(epfd.is_valid());

  ASSERT_EQ(0, EpollAdd(epfd.get(), socks[0], EPOLLIN, 0));
  ASSERT_EQ(0, EpollAdd(epfd.get(), socks[1], EPOLLIN, 1));

  constexpr int kMaxEvents = 4;
  struct epoll_event out_events[kMaxEvents];
  while (true) {
    errno = 0;
    int result = HANDLE_EINTR(epoll_wait(epfd.get(), out_events, kMaxEvents, -1));
    ASSERT_GT(result, 0);

    // Send reply(s).
    for (int i = 0; i < result; i++) {
      int sock_id = static_cast<int>(out_events[i].data.u64);
      ASSERT_TRUE(sock_id == 0 || sock_id == 1);

      char in = 0;
      ASSERT_EQ(1, HANDLE_EINTR(read(socks[sock_id], &in, 1)));
      if (in == 'q')
        return;

      // Spam user signals both before and after the reply to try to trigger races.
      DoTgkill(main_tid, main_tid, SIGUSR1);
      ASSERT_EQ(1, HANDLE_EINTR(write(socks[sock_id], "a", 1)));
      DoTgkill(main_tid, main_tid, SIGUSR1);
    }
  }
}

class ScopedSignalHandler {
 public:
  ScopedSignalHandler(int signum, sighandler_t handler) : signum_(signum) {
    prev_handler_ = signal(signum, handler);
  }
  ~ScopedSignalHandler() { signal(signum_, prev_handler_); }

 private:
  int signum_;
  sighandler_t prev_handler_;
};

}  // namespace

// Ping-pongs a bunch of messages between two threads while spamming the main thread with signals.
// This tests that epoll doesn't issue any spurious wakes.
TEST(EpollTest, LotsaSignals) {
  ScopedSignalHandler handler(SIGUSR1, &NoOpSigHandler);

  int pair1[2];
  int result = socketpair(AF_UNIX, SOCK_STREAM, 0, pair1);
  ASSERT_EQ(result, 0);

  int pair2[2];
  result = socketpair(AF_UNIX, SOCK_STREAM, 0, pair2);
  ASSERT_EQ(result, 0);

  // Ping pong
  pid_t main_tid = DoGetTid();
  std::thread ponger([main_tid, one = fbl::unique_fd(pair1[1]), two = fbl::unique_fd(pair2[1])]() {
    int socks[2];
    socks[0] = one.get();
    socks[1] = two.get();
    LotsaSignals_DoPong(main_tid, socks);
  });

  fbl::unique_fd epfd(epoll_create(2));
  ASSERT_TRUE(epfd.is_valid());

  fbl::unique_fd socks[2] = {fbl::unique_fd(pair1[0]), fbl::unique_fd(pair2[0])};
  ASSERT_EQ(0, EpollAdd(epfd.get(), socks[0].get(), EPOLLIN, 0));
  ASSERT_EQ(0, EpollAdd(epfd.get(), socks[1].get(), EPOLLIN, 1));

  HANDLE_EINTR(write(socks[0].get(), "0", 1));
  HANDLE_EINTR(write(socks[1].get(), "0", 1));

  // Arbitrary but large number of messages to send.
  constexpr int kMessageCount = 10000;

  constexpr int kMaxEvents = 4;
  struct epoll_event out_events[kMaxEvents];
  for (int i = 0; i < kMessageCount; i++) {
    errno = 0;
    int result = HANDLE_EINTR(epoll_wait(epfd.get(), out_events, kMaxEvents, -1));
    ASSERT_GT(result, 0);

    for (int i = 0; i < result; i++) {
      int sock_id = static_cast<int>(out_events[i].data.u64);
      ASSERT_TRUE(sock_id == 0 || sock_id == 1);

      // Read the message and reply to it.
      char in = 0;
      ASSERT_EQ(1, HANDLE_EINTR(read(socks[sock_id].get(), &in, 1)));
      ASSERT_EQ(1, HANDLE_EINTR(write(socks[sock_id].get(), "a", 1)));
    }
  }

  // Send quit to make the pong thread quit.
  ASSERT_EQ(1, HANDLE_EINTR(write(socks[0].get(), "q", 1)));

  ponger.join();
}

TEST(EpollTest, CloseAfterAdd) {
  int sockets[2];
  int result = socketpair(AF_UNIX, SOCK_STREAM, 0, sockets);
  ASSERT_EQ(0, result);

  fbl::unique_fd epfd(epoll_create(2));
  ASSERT_TRUE(epfd.is_valid());

  // Wait on socket[1] readable.
  struct epoll_event event;
  event.events = EPOLLIN;
  event.data.u64 = 1;
  result = epoll_ctl(epfd.get(), EPOLL_CTL_ADD, sockets[1], &event);
  ASSERT_EQ(result, 0);

  // Write data in the "0" end so the "1" will be marked ready to read. Writing just one byte
  // ensures there can't be a short write.
  ASSERT_EQ(1, HANDLE_EINTR(write(sockets[0], "a", 1)));

  // Close the read socket out from under epoll.
  close(sockets[1]);

  // Waiting on the (now empty) epoll object should timeout rather than report it's ready to read
  // or that there's a bad file descriptor.
  result = epoll_wait(epfd.get(), &event, 1, 1);
  EXPECT_EQ(0, result) << errno;
}

TEST(EpollTest, CtlModUpdatesData) {
  int sockets[2];
  int result = socketpair(AF_UNIX, SOCK_STREAM, 0, sockets);
  ASSERT_EQ(0, result);

  fbl::unique_fd epfd(epoll_create(2));
  ASSERT_TRUE(epfd.is_valid());

  struct epoll_event event;
  event.events = EPOLLIN;
  event.data.u32 = 1;
  result = epoll_ctl(epfd.get(), EPOLL_CTL_ADD, sockets[1], &event);
  ASSERT_EQ(result, 0);
  ASSERT_EQ(1, HANDLE_EINTR(write(sockets[0], "a", 1)));

  event.data.u32 = 0;
  result = epoll_wait(epfd.get(), &event, 1, 1);
  uint32_t first_read_data = event.data.u32;
  EXPECT_EQ(first_read_data, 1u);
  EXPECT_EQ(1, result) << errno;

  event.data.u32 = 2;
  result = epoll_ctl(epfd.get(), EPOLL_CTL_MOD, sockets[1], &event);

  ASSERT_EQ(1, HANDLE_EINTR(write(sockets[0], "a", 1)));

  event.data.u32 = 0;
  result = epoll_wait(epfd.get(), &event, 1, 1);
  uint32_t second_read_data = event.data.u32;
  EXPECT_EQ(second_read_data, 2u);
  EXPECT_EQ(1, result) << errno;
}

TEST(EpollTest, InvalidCreateSize) {
  errno = 0;
  EXPECT_EQ(-1, epoll_create(0));
  EXPECT_EQ(EINVAL, errno);

  errno = 0;
  EXPECT_EQ(-1, epoll_create(-1));
  EXPECT_EQ(EINVAL, errno);
}

TEST(EpollTest, WaitInvalidParams) {
  fbl::unique_fd epfd(epoll_create(2));
  ASSERT_TRUE(epfd);

  struct epoll_event event;

  errno = 0;
  EXPECT_EQ(-1, epoll_wait(epfd.get(), &event, 0, 0));
  EXPECT_EQ(EINVAL, errno);

  errno = 0;
  EXPECT_EQ(-1, epoll_wait(epfd.get(), &event, -1, 0));
  EXPECT_EQ(EINVAL, errno);

  // Pass invalid event pointer but valid count.  Linux seems to believe that
  // valid means 0 <= ptr < process memory - size of maxevents.
  errno = 0;
  EXPECT_EQ(
      -1, epoll_wait(epfd.get(), reinterpret_cast<struct epoll_event*>(0xFFFFFFFFFFFFFFFF), 1, 0));
  EXPECT_EQ(EFAULT, errno);

  // Linux believes nullptr is okay, so testing that
  EXPECT_EQ(0, epoll_wait(epfd.get(), nullptr, 1, 0));

  // When both the pointer and the count are invalid, Linux returns EINVAL (it checks the count
  // first).
  errno = 0;
  EXPECT_EQ(-1, epoll_wait(epfd.get(), nullptr, 0, 0));
  EXPECT_EQ(EINVAL, errno);
}

TEST(EpollTest, EPOLLWAKEUPEvent) {
  int fd = timerfd_create(CLOCK_REALTIME, 0);
  ASSERT_NE(-1, fd) << errno;

  fbl::unique_fd epoll_fd(epoll_create(1));
  EXPECT_TRUE(epoll_fd.is_valid());

  struct epoll_event ev = epoll_event();
  ev.events = EPOLLIN | EPOLLWAKEUP;
  EXPECT_EQ(0, epoll_ctl(epoll_fd.get(), EPOLL_CTL_ADD, fd, &ev));

  timespec begin = {};
  ASSERT_EQ(0, clock_gettime(CLOCK_REALTIME, &begin));
  // Timer 1 second in the future.
  struct itimerspec its = {};
  its.it_value = begin;
  its.it_value.tv_sec += 1;
  EXPECT_EQ(0, timerfd_settime(fd, TFD_TIMER_ABSTIME, &its, nullptr));

  pollfd pfd = {.fd = fd, .events = POLLIN};
  EXPECT_EQ(1, poll(&pfd, 1, -1));

  int ret = 0;
  struct epoll_event out_ev;
  ret = epoll_wait(epoll_fd.get(), &out_ev, 1, -1);
  EXPECT_EQ(1, ret);

  EXPECT_EQ(0, epoll_ctl(epoll_fd.get(), EPOLL_CTL_DEL, fd, &ev));
  close(fd);
  ret = epoll_wait(epoll_fd.get(), &out_ev, 1, 0);
  EXPECT_EQ(0, ret);
}

TEST(EpollTest, AliasArgumentWithDup) {
  fbl::unique_fd epoll_fd(epoll_create(1));
  ASSERT_TRUE(epoll_fd.is_valid());
  fbl::unique_fd another_fd(test_helper::MemFdCreate("memfd", 0));
  SAFE_SYSCALL(dup2(epoll_fd.get(), another_fd.get()));
  struct epoll_event event = {};
  EXPECT_EQ(-1, epoll_ctl(epoll_fd.get(), 1, another_fd.get(), &event));
  EXPECT_EQ(EINVAL, errno);
}

TEST(EpollTest, EpollIsPollable) {
  fbl::unique_fd epoll_fd(epoll_create(1));
  ASSERT_TRUE(epoll_fd.is_valid());

  fbl::unique_fd side_a, side_b;
  {
    int sockets[2];
    int result = socketpair(AF_UNIX, SOCK_STREAM, 0, sockets);
    ASSERT_EQ(0, result);
    side_a.reset(sockets[0]);
    side_b.reset(sockets[1]);
  }

  // Register side_a for writability. It will be immediately asserted.
  ASSERT_EQ(0, EpollAdd(epoll_fd.get(), side_a.get(), EPOLLOUT, 0));

  // Verify that epoll_fd reports that it's readable.
  pollfd pfd = {.fd = epoll_fd.get(), .events = POLLIN};
  ASSERT_EQ(1, poll(&pfd, 1, 0));
  EXPECT_EQ(POLLIN, pfd.revents);
}

TEST(EpollTest, PipeWritesNonemptyTriggerEpollInWithEpollET) {
  int pipe_ends[2];
  SAFE_SYSCALL(pipe2(pipe_ends, 0));
  fbl::unique_fd pipe_rd(pipe_ends[0]);
  fbl::unique_fd pipe_wr(pipe_ends[1]);

  struct epoll_event ev{};
  fbl::unique_fd epfd(epoll_create1(0));

  ev.events = EPOLLIN | EPOLLET;
  ev.data.fd = pipe_rd.get();

  SAFE_SYSCALL(epoll_ctl(epfd.get(), EPOLL_CTL_ADD, pipe_rd.get(), &ev));

  struct epoll_event events[1];
  ASSERT_EQ(0, epoll_wait(epfd.get(), events, 1, 0));

  // Write a byte into the pipe so that it is nonempty.
  char buf = 'a';
  EXPECT_EQ(1, write(pipe_wr.get(), &buf, 1));

  // Read a EPOLLIN event.
  ASSERT_EQ(1, epoll_wait(epfd.get(), events, 1, 0));
  EXPECT_EQ(events[0].data.fd, pipe_rd.get());

  // Now the epoll entry is not ready, so epoll_wait returns no ready items.
  ASSERT_EQ(0, epoll_wait(epfd.get(), events, 1, 0));

  // Write another byte to trigger EPOLLIN again.
  EXPECT_EQ(1, write(pipe_wr.get(), &buf, 1));

  // This marks the epoll entry as ready (even though the level didn't change).
  ASSERT_EQ(1, epoll_wait(epfd.get(), events, 1, 0));
  EXPECT_EQ(events[0].data.fd, pipe_rd.get());
}

TEST(EpollTest, SocketpairWritesNonemptyTriggerEpollInWithEpollET) {
  int socketpair_fds[2];
  SAFE_SYSCALL(socketpair(AF_UNIX, SOCK_STREAM, 0, socketpair_fds));
  fbl::unique_fd socket_a(socketpair_fds[0]);
  fbl::unique_fd socket_b(socketpair_fds[1]);

  struct epoll_event ev{};
  fbl::unique_fd epfd(epoll_create1(0));

  ev.events = EPOLLIN | EPOLLET;
  ev.data.fd = socket_a.get();

  SAFE_SYSCALL(epoll_ctl(epfd.get(), EPOLL_CTL_ADD, socket_a.get(), &ev));

  struct epoll_event events[1];
  ASSERT_EQ(0, epoll_wait(epfd.get(), events, 1, 0));

  // Write a byte into the socket so that it is nonempty.
  char buf = 'a';
  EXPECT_EQ(1, write(socket_b.get(), &buf, 1));

  // Read a EPOLLIN event.
  ASSERT_EQ(1, epoll_wait(epfd.get(), events, 1, 0));
  EXPECT_EQ(events[0].data.fd, socket_a.get());

  // Now the epoll entry is not ready, so epoll_wait returns no ready items.
  ASSERT_EQ(0, epoll_wait(epfd.get(), events, 1, 0));

  // Write another byte to trigger EPOLLIN again.
  EXPECT_EQ(1, write(socket_b.get(), &buf, 1));

  // This marks the epoll entry as ready (even though the level didn't change).
  ASSERT_EQ(1, epoll_wait(epfd.get(), events, 1, 0));
  EXPECT_EQ(events[0].data.fd, socket_a.get());
}

TEST(EpollTest, PipeReadFullTriggerEpollOutWithEpollET) {
  int pipe_fds[2];
  SAFE_SYSCALL(pipe2(pipe_fds, O_NONBLOCK));
  fbl::unique_fd pipe_rd(pipe_fds[0]);
  fbl::unique_fd pipe_wr(pipe_fds[1]);

  struct epoll_event ev{};
  fbl::unique_fd epfd(epoll_create1(0));

  ev.events = EPOLLOUT | EPOLLET;
  ev.data.fd = pipe_wr.get();

  // Fill the pipe with page-sized buffers.
  const size_t page_size = sysconf(_SC_PAGE_SIZE);
  std::vector<std::byte> page_buffer(page_size, std::byte{0xAB});
  SAFE_SYSCALL(fcntl(pipe_wr.get(), F_SETPIPE_SZ, 2 * page_size));
  ASSERT_EQ(static_cast<ssize_t>(page_size),
            write(pipe_wr.get(), page_buffer.data(), page_buffer.size()));
  ASSERT_EQ(static_cast<ssize_t>(page_size),
            write(pipe_wr.get(), page_buffer.data(), page_buffer.size()));

  SAFE_SYSCALL(epoll_ctl(epfd.get(), EPOLL_CTL_ADD, pipe_wr.get(), &ev));

  // Since the pipe is full, epoll_wait returns no available events.
  struct epoll_event events[1];
  ASSERT_EQ(0, epoll_wait(epfd.get(), events, 1, 0));

  // Read a page out of the pipe to trigger EPOLLOUT.
  EXPECT_EQ(static_cast<ssize_t>(page_buffer.size()),
            read(pipe_rd.get(), page_buffer.data(), page_buffer.size()));

  // This marks the epoll entry as ready since the pipe is no longer full.
  ASSERT_EQ(1, epoll_wait(epfd.get(), events, 1, 0));
  EXPECT_EQ(events[0].data.fd, pipe_wr.get());

  EXPECT_EQ(0, epoll_wait(epfd.get(), events, 1, 0));

  // Read another byte.
  char rd_buf{};
  EXPECT_EQ(1, read(pipe_rd.get(), &rd_buf, 1));

  // Since the pipe was not full before reading, this read does not trigger EPOLLOUT.
  ASSERT_EQ(0, epoll_wait(epfd.get(), events, 1, 0));
}
