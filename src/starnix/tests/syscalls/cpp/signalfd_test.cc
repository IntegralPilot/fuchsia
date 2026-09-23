// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <poll.h>
#include <signal.h>
#include <sys/signalfd.h>
#include <sys/wait.h>
#include <unistd.h>

#include <gtest/gtest.h>

namespace {

TEST(Signalfd, SignalDoesNotTriggerSpuriousWakeup) {
  // Block SIGCHLD to ensure it is caught by the signalfd.
  sigset_t block_mask;
  sigemptyset(&block_mask);
  sigaddset(&block_mask, SIGCHLD);
  sigaddset(&block_mask, SIGTERM);
  sigaddset(&block_mask, SIGINT);
  ASSERT_NE(sigprocmask(SIG_BLOCK, &block_mask, nullptr), -1);

  // Create a signalfd for termination signals (SIGTERM, SIGINT).
  sigset_t term_mask;
  sigemptyset(&term_mask);
  sigaddset(&term_mask, SIGTERM);
  sigaddset(&term_mask, SIGINT);
  int sfd_term = signalfd(-1, &term_mask, 0);
  ASSERT_NE(sfd_term, -1);

  // Create a signalfd for the child signal (SIGCHLD).
  sigset_t chld_mask;
  sigemptyset(&chld_mask);
  sigaddset(&chld_mask, SIGCHLD);
  int sfd_chld = signalfd(-1, &chld_mask, 0);
  ASSERT_NE(sfd_chld, -1);

  // Create a simple child process that exits immediately and therefore generates a SIGCHLD.
  pid_t pid = fork();
  ASSERT_NE(pid, -1);

  if (pid == 0) {
    // Child process.
    _exit(0);
  }

  // Parent process can poll both fds for events using an arbitrary timeout.
  struct pollfd fds[2];
  fds[0].fd = sfd_term;
  fds[0].events = POLLIN;
  fds[1].fd = sfd_chld;
  fds[1].events = POLLIN;
  int num_events = poll(fds, 2, 1000);

  // Expect only one wakeup as only one signalfd cares about SIGCHLD.
  ASSERT_EQ(num_events, 1);
  // The SIGTERM/SIGINT signalfd should have no `revents`.
  EXPECT_EQ(fds[0].revents, 0);
  // The SIGCHLD should have `revents` for POLLIN as the signal was received.
  EXPECT_EQ(fds[1].revents, POLLIN);

  // Read from the signalfd to confirm it was SIGCHLD.
  struct signalfd_siginfo ssi;
  ssize_t bytes_read = read(sfd_chld, &ssi, sizeof(ssi));
  ASSERT_EQ(static_cast<size_t>(bytes_read), sizeof(ssi));
  EXPECT_EQ(ssi.ssi_signo, static_cast<uint32_t>(SIGCHLD));

  // Clean up resources and child.
  close(sfd_term);
  close(sfd_chld);
  waitpid(pid, nullptr, 0);
  ASSERT_NE(sigprocmask(SIG_UNBLOCK, &block_mask, nullptr), -1);
}

}  // namespace
