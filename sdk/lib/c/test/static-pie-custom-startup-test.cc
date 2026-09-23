// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/channel.h>
#include <zircon/assert.h>
#include <zircon/sanitizer.h>
#include <zircon/startup.h>
#include <zircon/types.h>

#include <array>

// This implements a static PIE that overrides the <zircon/startup.h> API
// functions to make the static libc.a startup code work with a custom
// bootstrap protocol.  That protocol is defined in the header shared with the
// test component that launches this static PIE.
//
// This is meant to exercise the libc startup code and ensure that full
// initialization happens in the proper order and the API contract of the
// <zircon/startup.h> functions is held up by libc.  This program will just
// crash with assertion failures (that probably won't get printed anywhere,
// just crashes) if the various kinds of initializer / constructor hook aren't
// all called in the right way in the right order, finishing with calling main.
// Finally main sends a "pong" reply to the test harness and exits with code 0.

#include "custom-startup-test.h"

namespace {

using InitFn = void();

enum State {
  kInitial,
  kGetHandles,
  kGetArguments,
  kPreinitHook,
  kPreinitArray,
  kCtor,
  kMain,
};

void Check(State new_state) {
  static State gState = kInitial;
  ZX_ASSERT(gState == new_state - 1);
  gState = new_state;
}

[[gnu::constructor]] void Ctor() { Check(kCtor); }

void PreinitArrayFn() { Check(kPreinitArray); }

[[gnu::section(".preinit_array"), gnu::used,
  gnu::retain]] alignas(InitFn*) InitFn* const kCallPreinitArrayFn = PreinitArrayFn;

struct Hook {
  zx::channel channel;
};

Hook gHook;

}  // namespace

zx_startup_handles_t _zx_startup_get_handles(zx_handle_t process_start_handle) {
  Check(kGetHandles);

  gHook = {.channel{process_start_handle}};

  zx_signals_t pending;
  zx_status_t status = gHook.channel.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &pending);
  ZX_ASSERT(status == ZX_OK);
  ZX_ASSERT(pending & ZX_CHANNEL_READABLE);

  std::array<char, kPing.size()> message_bytes;
  std::array<zx_handle_t, kMessageHandles> message_handles;
  uint32_t actual_bytes, actual_handles;
  status = gHook.channel.read(0, message_bytes.data(), message_handles.data(), message_bytes.size(),
                              message_handles.size(), &actual_bytes, &actual_handles);
  ZX_ASSERT(status == ZX_OK);

  ZX_ASSERT(actual_bytes == kPing.size());
  ZX_ASSERT(std::string_view(message_bytes.data(), actual_bytes) == kPing);

  return {
      .process_self = message_handles[kProcessSelfHandle],
      .thread_self = message_handles[kThreadSelfHandle],
      .allocation_vmar = message_handles[kAllocationVmarHandle],
      .executable_image_vmar = message_handles[kImageVarHandle],
      .log = message_handles[kLogHandle],
      .hook = &gHook,
  };
}

zx_startup_arguments_t _zx_startup_get_arguments(void* hook) {
  Check(kGetArguments);
  ZX_ASSERT(hook == &gHook);
  return {};
}

void _zx_startup_preinit(void* hook) {
  Check(kPreinitHook);
  ZX_ASSERT(hook == &gHook);
}

int main(int argc, char** argv, char** envp) {
  Check(kMain);

  ZX_ASSERT(argc == 0);
  ZX_ASSERT(argv != nullptr);
  ZX_ASSERT(argv[0] == nullptr);
  ZX_ASSERT(envp != nullptr);
  ZX_ASSERT(envp[0] == nullptr);

  __sanitizer_log_write(kLog.data(), kLog.size());

  zx_status_t status = gHook.channel.write(0, kPong.data(), kPong.size(), nullptr, 0);
  ZX_ASSERT(status == ZX_OK);

  return 0;
}
