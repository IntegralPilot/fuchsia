// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_ZIRCON_TESTS_RESTRICTED_MODE_SHARED_STRESS_LIB_H_
#define SRC_ZIRCON_TESTS_RESTRICTED_MODE_SHARED_STRESS_LIB_H_

#include <lib/zx/time.h>

enum class TestMode : uint8_t {
  Restricted,
  Shared,
};

// This test aims to stress test unified aspaces.
//
// Failures in this test indicate concurrency issues in the unified aspaces implementation. More
// often than not, the issue will be in the TLB invalidation logic.
//
// When testing the shared region (test_mode is TestMode::Shared), this test performs the
// following sequence of events on loop:
// 1. The main thread will map a shared address and write a random value to it.
// 2. The main thread will spin up a bunch of reader threads.
// 3. The readers will attempt to read the shared address and pause.
// 4. The main thread will verify that the readers all read the right value.
// 5. The main thread will unmap the shared address and resume all of the readers.
// 6. The readers will then attempt to read the address again and fault.
// 7. The main thread will verify that all the readers faulted.
// 8. The main thread will remap the shared address with a new random value.
// 9. The main thread will resume all of the readers and wait for them to exit.
// 10. The main thread will then verify that the readers all read the new random value.
//
// When testing the restricted region (test_mode is TestMode::Restricted), this test performs the
// following sequence of events on loop:
// 1. The main thread creates a shared process and maps an executable VMO to be run in restricted
//    mode.
// 2. The main thread maps a shared address into the restricted VMAR.
// 3. The main thread spins up a bunch of readers that start up and wait for a signal that states
//    the shared address has been mapped and written to.
// 4. The main thread spins up a writer thread that runs in the shared process and writes a random
//    value to the shared address. This thread then signals to the readers that they are free to
//    proceed.
// 5. The readers all resume, enter restricted mode, and read the value from the shared address.
// 6. The readers all exit restricted mode, at which point they validate that they exited with an
//    undefined instruction exception, and that the correct value was read from the shared address.
// 7. The readers then pause and wait for the main thread to unmap the shared address.
// 8. The main thread unmaps the shared address and signals to the readers that it has done so.
// 9. The readers all enter restricted mode again, but this time they enter a page fault trying to
//    read the shared address and exit restricted mode. They then verify that they all got page
//    faults.
//
// This loop will continue for `runtime`, and will print out an update with the number of
// iterations completed every `poll_interval`.
void Orchestrator(zx::duration runtime, zx::duration poll_interval, TestMode test_mode);

#endif  // SRC_ZIRCON_TESTS_RESTRICTED_MODE_SHARED_STRESS_LIB_H_
