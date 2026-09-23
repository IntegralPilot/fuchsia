// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_TEST_FAKE_CLOCK_H_
#define SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_TEST_FAKE_CLOCK_H_

#include <lib/zx/clock.h>
#include <lib/zx/time.h>

namespace mdns::test {

// Fakes |zx_clock_get_monotonic| (and therefore |zx::clock::get_monotonic()|).
//
// Creating an instance of this class fakes the clock during the lifetime of the instance:
//
// {
//   // Real clock in use here.
//   ...
//   FakeClock kayfabe;
//   // Fake clock in use here.
//   ...
//   FakeClock::Advance(zx::sec(10));
//   ...
// }
// // Real clock in use here.
//
// Alternatively, the static methods |Start| and |Stop| may be used.
//
class FakeClock {
 public:
  // Begins faking |zx_clock_get_monotonic|, initially setting the fake clock time to |at|.
  // To resume faking the clock at its current value, do this:
  //
  //     FakeClock::Start(FakeClock::Get());
  //
  // Starting the fake clock at zero is generally not recommended, because some code may reasonably
  // assume that |zx_clock_get_monotonic| will never return zero.
  static void Start(zx::time at = zx::clock::get_monotonic());

  // Makes |zx_clock_get_monotonic| function normally.
  static void Stop();

  // Advances the fake clock value by the specified duration.
  static void Advance(zx::duration by);

  // Sets the current fake clock value.
  static void Set(zx::time to);

  // Gets the current fake clock value.
  static zx::time Get();

  // Constructs a |FakeClock|, starting the fake clock at |at| and stopping it when the destructor
  // runs.
  explicit FakeClock(zx::time at = zx::clock::get_monotonic()) { Start(at); }

  // Destructs the current |FakeClock|, returning |zx_clock_get_monotonic| to normal function.
  ~FakeClock() { Stop(); }
};

}  // namespace mdns::test

#endif  // SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_TEST_FAKE_CLOCK_H_
