// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_TEST_CUSTOM_STARTUP_TEST_H_
#define LIB_C_TEST_CUSTOM_STARTUP_TEST_H_

#include <cstdint>
#include <string_view>

// This defines a trivial private protocol shared between the static PIE
// (static-pie-custom-startup-test.cc) implementing <zircon/startup.h> hooks,
// and the thing launching that program (the test component implemented in
// custom-startup-test.cc).  There is one bootstrap message sent by the
// launcher: the 4 "ping" bytes, with kMessageHandles handles.  Then there is
// one message sent back by the test PIE if all is well: the "pong" bytes with
// no handles.  It also writes the "log" bytes via kLogHandle before it exits.

inline constexpr std::string_view kPing = "ping", kPong = "pong", kLog = "log";

enum MessageHandles : uint32_t {
  kProcessSelfHandle,
  kThreadSelfHandle,
  kAllocationVmarHandle,
  kImageVarHandle,
  kLogHandle,
  kMessageHandles,
};

#endif  // LIB_C_TEST_CUSTOM_STARTUP_TEST_H_
