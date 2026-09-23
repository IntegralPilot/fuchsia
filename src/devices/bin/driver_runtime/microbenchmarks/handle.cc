// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_runtime/handle.h"

#include <lib/fdf/cpp/channel.h>

#include <perftest/perftest.h>

#include "src/devices/bin/driver_runtime/channel.h"

namespace {

// These tests measure the times taken to create and close various types of
// fdf handles. Strictly speaking, they test creating fdf objects as
// well as creating handles.
//
// In each test, closing the handles is done implicitly by destructors.

bool ChannelCreateTest(perftest::RepeatState* state) {
  state->DeclareStep("create");
  state->DeclareStep("close");
  while (state->KeepRunning()) {
    auto channels = fdf::ChannelPair::Create(0);
    ZX_ASSERT(channels.status_value() == ZX_OK);
    state->NextStep();
  }
  return true;
}

bool ChannelGetObjectTest(perftest::RepeatState* state) {
  auto channels = fdf::ChannelPair::Create(0);
  ZX_ASSERT(channels.status_value() == ZX_OK);

  while (state->KeepRunning()) {
    fbl::RefPtr<driver_runtime::Channel> channel;
    zx_status_t status =
        driver_runtime::Handle::GetObject<driver_runtime::Channel>(channels->end0.get(), &channel);
    ZX_ASSERT(status == ZX_OK);
  }

  return true;
}

void RegisterTests() {
  perftest::RegisterTest("HandleCreate_Channel", ChannelCreateTest);
  perftest::RegisterTest("HandleGetObject_Channel", ChannelGetObjectTest);
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
