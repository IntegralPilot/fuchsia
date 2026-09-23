// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Tests which require the UTC timeline to be mutable.

#include <sys/epoll.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <sys/timerfd.h>
#include <time.h>
#include <unistd.h>

#include <ctime>
#include <thread>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

TEST(TimerFD, AlarmCancelOnSet) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "AlarmCancelOnSet test requires CAP_SYS_ADMIN";
  }

  int epoll_fd = epoll_create(/*ignored*/ 1);
  ASSERT_GT(epoll_fd, 0);

  int timer_fd = timerfd_create(CLOCK_REALTIME, TFD_NONBLOCK);
  ASSERT_GT(timer_fd, 0);

  epoll_event ev;
  ev.events = EPOLLIN | EPOLLWAKEUP;
  ev.data.u32 = 0x42;  // ignored
  ASSERT_EQ(0, epoll_ctl(epoll_fd, EPOLL_CTL_ADD, timer_fd, &ev));

  struct itimerspec wakeup_spec = {};
  ASSERT_EQ(0, timerfd_settime(timer_fd, TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET, &wakeup_spec,
                               nullptr));

  auto rendezvous = test_helper::MakeRendezvous();
  pid_t test_thread_tid = 0;
  std::thread test_thread([&rendezvous, &test_thread_tid, epoll_fd, timer_fd] {
    test_thread_tid = gettid();
    rendezvous.poker.poke();

    struct epoll_event events[1];

    // When the UTC timeline changes, we should get an event, and reading
    // the timer must give us ECANCELED. This is the TFD_TIMER_CANCEL_ON_SET way.
    printf("test_thread: entering epoll_wait\n");
    int ev_count = epoll_wait(epoll_fd, events, sizeof(events), -1);
    printf("test_thread: epoll_wait returned ev_count=%d (errno %s)\n", ev_count, strerror(errno));
    ASSERT_GT(ev_count, 0) << strerror(errno);

    uint64_t unused;
    ASSERT_EQ(-1, read(timer_fd, &unused, sizeof(unused)));
    // This errno read should not race with the one below, since the previous
    // line will unblock only once that syscall is complete.
    ASSERT_EQ(errno, ECANCELED);
  });

  // We must ensure that there is an active epoll_wait monitoring for timeline change before
  // we attempt to change the UTC timeline. Otherwise the change would have happened
  // "before" the poll, and the poll will not see it.
  rendezvous.holder.hold();
  printf("main_thread: wait here to ensure test_thread entered epoll_wait\n");
  // A sleep alone removes flakiness 100%, but a wait alone does not. Let's
  // see if this makes a difference.
  sleep(1);
  ASSERT_TRUE(test_helper::WaitUntilBlocked(test_thread_tid, true));
  printf("main_thread: ending wait, on to changing the timeline\n");

  // Now, rejigger the UTC timeline. This should cause epoll_wait above to unblock.
  struct timeval tv = {};
  ASSERT_EQ(0, gettimeofday(&tv, nullptr));
  // Using settimeofday, as it is the only syscall allowed to change UTC. Working
  // around the libc `settimeofday` which offloads to a different function.
  //
  // Force a UTC timeline update by winding time into the future. The ability to
  // do this in syscall tests requires a special test fixture.
  tv.tv_sec += 100000;
  printf("main_thread: entering settimeofday syscall to force timeline change\n");
  ASSERT_EQ(0, syscall(__NR_settimeofday, &tv, nullptr)) << "strerror: " << strerror(errno);
  printf("main_thread: settimeofday syscall returned\n");

  test_thread.join();

  close(timer_fd);
  close(epoll_fd);
}
