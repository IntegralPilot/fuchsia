// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/test/fake_clock.h"

extern "C" {

thread_local bool g_kayfabe = false;
thread_local zx::time g_fake_time = zx::time(0);

__EXPORT zx_time_t zx_clock_get_monotonic() {
  return g_kayfabe ? g_fake_time.get() : _zx_clock_get_monotonic();
}
}

namespace mdns::test {

// static
void FakeClock::Start(zx::time at) {
  g_fake_time = at;
  g_kayfabe = true;
}

// static
void FakeClock::Stop() { g_kayfabe = false; }

// static
void FakeClock::Advance(zx::duration by) { g_fake_time += by; }

// static
void FakeClock::Set(zx::time to) { g_fake_time = to; }

// static
zx::time FakeClock::Get() { return zx::time(g_fake_time); }

}  // namespace mdns::test
