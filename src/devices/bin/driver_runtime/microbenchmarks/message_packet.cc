// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_runtime/message_packet.h"

#include <lib/fdf/cpp/arena.h>
#include <lib/syslog/cpp/macros.h>

#include <format>

#include <perftest/perftest.h>

#include "src/devices/bin/driver_runtime/microbenchmarks/assert.h"
#include "src/devices/bin/driver_runtime/thread_context.h"

namespace {

// Measure the time taken to allocate and free a message packet
// which holds a buffer of |buffer_size| and an array of |num_handles|.
bool MessagePacketCreateDestroyTest(perftest::RepeatState* state, uint32_t buffer_size,
                                    uint32_t num_handles) {
  state->DeclareStep("setup");
  state->DeclareStep("create");
  state->DeclareStep("destroy");
  state->DeclareStep("teardown");

  while (state->KeepRunning()) {
    // Setup the arena, data, and handle buffers to pass to the message packet.
    constexpr uint32_t kTag = 'BNCH';
    fdf::Arena arena(kTag);

    void* data = arena.Allocate(buffer_size);
    auto handles_buf = static_cast<zx_handle_t*>(arena.Allocate(num_handles * sizeof(zx_handle_t)));

    for (uint32_t i = 0; i < num_handles; i++) {
      // Ownership of the handle is passed to the message packet.
      zx_handle_t event;
      ASSERT_OK(zx_event_create(0, &event));
      handles_buf[i] = event;
    }

    // The internal |MessagePacket| API expects a ref ptr.
    fbl::RefPtr<fdf_arena> arena_ref(arena.get());
    {
      state->NextStep();
      driver_runtime::MessagePacketOwner msg_packet = driver_runtime::MessagePacket::Create(
          arena_ref, data, buffer_size, handles_buf, num_handles);
      state->NextStep();
      // The msg_packet and included handles will be automatically destroyed.
    }
    state->NextStep();
  }

  return true;
}

void RegisterTests() {
  static const unsigned kBufferSize[] = {
      0,
      64,
  };
  static const unsigned kNumHandles[] = {
      0,
      1,
  };
  for (auto buffer_size : kBufferSize) {
    for (auto num_handles : kNumHandles) {
      auto name =
          std::format("MessagePacket/CreateDestroy/{}bytes/{}handles", buffer_size, num_handles);
      perftest::RegisterTest(name.c_str(), MessagePacketCreateDestroyTest, buffer_size,
                             num_handles);
    }
  }
}

PERFTEST_CTOR(RegisterTests)

}  // namespace
