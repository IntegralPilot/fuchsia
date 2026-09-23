// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl_driver/cpp/natural_messaging.h>
#include <zircon/types.h>

namespace fdf::internal {

const char* const kFailedToCreateDriverArena = "failed to create driver arena";

fidl::OutgoingMessage MoveToArena(fidl::OutgoingMessage& message, const fdf::Arena& arena) {
  if (!message.ok()) {
    return fidl::OutgoingMessage(message.error());
  }

  uint32_t size = message.CountBytes();
  void* bytes_on_arena = arena.Allocate(size);
  {
    uint8_t* dst = static_cast<uint8_t*>(bytes_on_arena);
    for (uint32_t i = 0; i < message.iovec_actual(); ++i) {
      const zx_channel_iovec_t& iovec = message.iovecs()[i];
      dst = std::copy_n(static_cast<const uint8_t*>(iovec.buffer), iovec.capacity, dst);
    }
  }

  zx_channel_iovec_t& iovec_on_arena =
      *static_cast<zx_channel_iovec_t*>(arena.Allocate(sizeof(zx_channel_iovec_t)));
  iovec_on_arena = {
      .buffer = bytes_on_arena,
      .capacity = size,
  };

  fidl_handle_t* handles_on_arena =
      static_cast<fidl_handle_t*>(arena.Allocate(message.handle_actual() * sizeof(fidl_handle_t)));
  std::copy_n(message.handles(), message.handle_actual(), handles_on_arena);

  uint32_t handle_actual = message.handle_actual();
  message.ReleaseHandles();

  return fidl::OutgoingMessage::Create_InternalMayBreak({
      .transport_vtable = &fidl::internal::DriverTransport::VTable,
      .iovecs = &iovec_on_arena,
      .num_iovecs = 1,
      .handles = handles_on_arena,
      .handle_metadata = nullptr,
      .num_handles = handle_actual,
      .is_transactional = message.is_transactional(),
  });
}

}  // namespace fdf::internal
