// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <zircon/syscalls.h>

#include <thread>

#include <stacktrack/bind.h>

// A recursive function to create a deep stack trace.
[[gnu::noinline, clang::no_sanitize("safe-stack")]]
static void deep_recursion(int n) {
  uint8_t large_local_buffer[1024];

  // Ensure the buffer is not optimized away before the recursive call.
  asm volatile("" : : "r"(large_local_buffer) : "memory");

  if (n == 0) {
    // Stacktrack measures stack usage at syscall call sites, so let's call one.
    zx_nanosleep(ZX_TIME_INFINITE_PAST);
  } else {
    deep_recursion(n - 1);
  }

  // Avoid tail call optimization and ensure the buffer is not optimized away after the recursive
  // call.
  asm volatile("" : : "r"(large_local_buffer) : "memory");
}

static void sleep_10s() {
  fprintf(stderr, "Sleep start\n");
  sleep(10);
  fprintf(stderr, "Sleep stop\n");
}

int main(int argc, char **argv) {
  fprintf(stderr, "Stacktrack example starting...\n");
  stacktrack_bind_with_fdio();

  for (int i = 20; i < 30; i++) {
    fprintf(stderr, "Iteration %d\n", i);
    deep_recursion(i);
    sleep(1);
  }

  fprintf(stderr, "First part finished, entering second part...\n");

  // This creates an alternating pattern of having four threads active (the main
  // thread plus the three auxiliary threads) and then just the main thread.
  while (true) {
    std::thread t1(sleep_10s);
    std::thread t2(sleep_10s);
    std::thread t3(sleep_10s);
    t1.join();
    t2.join();
    t3.join();

    sleep_10s();
  }
}
