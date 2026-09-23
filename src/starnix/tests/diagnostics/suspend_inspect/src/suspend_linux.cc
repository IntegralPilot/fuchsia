// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <signal.h>
#include <time.h>
#include <unistd.h>

#include "src/lib/files/file.h"

timer_t start_interval_timer() {
  timer_t timer_id;
  struct sigevent sev = {};
  sev.sigev_notify = SIGEV_NONE;
  timer_create(CLOCK_REALTIME_ALARM, &sev, &timer_id);

  // Set a test timer that will trigger every 1 second to wake
  // up all the subsequent suspends.
  struct itimerspec its = {};
  its.it_value.tv_sec = 2;
  its.it_interval.tv_sec = 2;
  timer_settime(timer_id, 0, &its, nullptr);

  // TODO(https://fxbug.dev/373676361): This sleep is here to guarantee that
  // the hrtimer request has been sent before tests try to suspend.
  sleep(5);

  return timer_id;
}

void stop_interval_timer(timer_t timer_id) {
  struct itimerspec its = {};
  its.it_value.tv_sec = 0;
  its.it_interval.tv_sec = 0;
  timer_settime(timer_id, 0, &its, nullptr);
}

int main(int argc, char** argv) {
  printf("suspend_linux, suspending ...\n");
  timer_t timer = start_interval_timer();
  bool ok = files::WriteFile("/sys/power/state", "mem");

  if (!ok) {
    printf("suspend_linux, error. Could not open /sys/power/state.\n");
    return 1;
  }
  printf("suspend_linux, resumed, and exiting.\n");

  stop_interval_timer(timer);
  return 0;
}
