// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/sys/cpp/component_context.h>

#include "src/camera/bin/virtual_camera/stream_storage.h"
#include "src/camera/bin/virtual_camera/virtual_camera_agent.h"

int main(int argc, char** argv) {
  // Create the VirtualCameraAgent which will serve protocols.
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto component_context = sys::ComponentContext::CreateAndServeOutgoingDirectory();
  camera::StreamStorage stream_storage;
  camera::VirtualCameraAgent virtual_camera_agent(component_context.get(), stream_storage);
  loop.Run();
  return 0;
}
