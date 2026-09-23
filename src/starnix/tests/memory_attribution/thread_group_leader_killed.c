// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <linux/unistd.h>

pid_t main_tid;
void* thread_function(void* arg);

// This program spawns a thread that allocates memory after the main thread
// exits. This is meant to test that the allocated memory is still reported even
// when the thread group leader is killed.
int main(void) {
  main_tid = (pid_t)syscall(SYS_gettid);

  // Sleep needed to address a flake in
  // //src/lib/fuchsia-component-test/src/lib.rs:ScopedInstanceFactory::new_named_instance.
  sleep(10);

  pthread_t thread;
  if (pthread_create(&thread, NULL, thread_function, NULL)) {
    fprintf(stderr, "Error creating second thread\n");
    return 1;
  }

  // Terminate the thread group leader (main thread).
  syscall(SYS_exit, 0);
  return 0;
}

// Each thread maps an anonymous memory region, writes some log, and then sleeps forever.
void* thread_function(void* arg) {
  // Wait for main thread to go away.
  for (;;) {
    int ret = tgkill(getpid(), main_tid, 0);
    if (ret == -1 && errno == ESRCH) {
      fprintf(stdout, "[thread_group_leader_killed] main thread is no longer running.\n");
      fflush(stdout);
      break;
    }
  }

  const size_t num_pages_to_allocate = 4200;
  const size_t page_size = sysconf(_SC_PAGESIZE);

  void* addr = NULL;
  const size_t size = num_pages_to_allocate * page_size;
  const int prot = PROT_READ | PROT_WRITE;
  const int flags = MAP_SHARED | MAP_ANONYMOUS;
  const int fd = -1;
  const off_t offset = 0;

  addr = mmap(addr, size, prot, flags, fd, offset);
  if (addr == MAP_FAILED) {
    exit(EXIT_FAILURE);
  }

  // Populate those pages.
  for (size_t i = 0; i < size; i++) {
    ((volatile char*)addr)[i] = (char)(1);
  }

  fprintf(stdout, "[thread_group_leader_killed] second thread did mmap\n");
  fflush(stdout);

  // Sleep forever.
  for (;;) {
    sleep(10);
  }
  return NULL;
}
