// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START common_imports]
#include <stdio.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/clock.h>
// [END common_imports]
// [START utc_imports]
#include <zircon/utc.h>
// [END utc_imports]

// [START monotonic]
void monotonic_examples(void) {
  // Read monotonic time.
  zx_time_t mono_nsec = zx_clock_get_monotonic();
  printf("The monotonic time is %ld ns.\n", mono_nsec);
}
// [END monotonic]

// [START utc]
void utc_examples(void) {
  // This is a borrowed handle. Do not close it, and do not replace it using
  // zx_utc_reference_swap while using it.
  zx_handle_t utc_clock = zx_utc_reference_get();

  if (utc_clock != ZX_HANDLE_INVALID) {
    // Wait for the UTC clock to start.  The clock may never start on a device
    // that does not have a RTC or a network connection.
    // A started clock may, but also may not have a valid UTC actual.
    zx_status_t status = zx_object_wait_one(utc_clock, ZX_CLOCK_STARTED, ZX_TIME_INFINITE, NULL);
    if (status == ZX_OK) {
      printf("UTC clock is started.\n");
    } else {
      printf("zx_object_wait_one syscall failed (status = %d).\n", status);
    }

    // Wait for the UTC clock to be externally synchronized.  Once that happens,
    // the clock is known to correspond to UTC actual (with error bounds available through
    // zx_clock_get_details).
    status = zx_object_wait_one(utc_clock, ZX_USER_SIGNAL_0, ZX_TIME_INFINITE, NULL);
    if (status == ZX_OK) {
      printf("UTC clock is externally synchronized.\n");
    } else {
      printf("zx_object_wait_one syscall failed (status = %d).\n", status);
    }

    // Wait for the UTC clock to be of "logging quality".  Logging quality UTC
    // clock is started, but not necessarily corresponding to UTC actual. This
    // clock is to be used only for logging timestamps.
    status = zx_object_wait_one(utc_clock, ZX_USER_SIGNAL_1, ZX_TIME_INFINITE, NULL);
    if (status == ZX_OK) {
      printf("UTC clock is of logging quality.\n");
    } else {
      printf("zx_object_wait_one syscall failed (status = %d).\n", status);
    }

    // Read the UTC clock.
    zx_time_t nsec;
    status = zx_clock_read(utc_clock, &nsec);
    if (status == ZX_OK) {
      printf("It has been %ld ns since the epoch.\n", nsec);
    } else {
      printf("zx_clock_read syscall failed (status = %d).\n", status);
    }

    // Read UTC clock details.
    zx_clock_details_v1_t details;
    status = zx_clock_get_details(utc_clock, ZX_CLOCK_ARGS_VERSION(1), &details);
    if (status == ZX_OK) {
      printf("The UTC clock's backstop time is %ld ns since the epoch.\n", details.backstop_time);
    } else {
      printf("zx_clock_get_details failed (status = %d).\n", status);
    }

  } else {
    printf("Error, our runtime has no clock assigned to it!\n");
  }
}
// [END utc]

int main(int argc, const char** argv) {
  monotonic_examples();
  utc_examples();
}
