// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdf/cpp/arena.h>
#include <lib/fidl/cpp/wire/message.h>
#include <lib/fidl_driver/cpp/transport.h>
#include <lib/fidl_driver/cpp/unknown_interactions.h>

namespace fidl::internal {

void SendDriverUnknownMethodReply(UnknownMethodReply reply, ::fidl::Transaction* txn) {
  fdf::Arena arena('FIDL');

  decltype(reply)& arena_reply = *static_cast<decltype(reply)*>(arena.Allocate(sizeof(reply)));
  arena_reply = reply;

  zx_channel_iovec_t& iovec_on_arena =
      *static_cast<zx_channel_iovec_t*>(arena.Allocate(sizeof(zx_channel_iovec_t)));
  iovec_on_arena = {
      .buffer = &arena_reply,
      .capacity = sizeof(reply),
  };

  ::fidl::OutgoingMessage msg = ::fidl::OutgoingMessage::Create_InternalMayBreak({
      .transport_vtable = &DriverTransport::VTable,
      .iovecs = &iovec_on_arena,
      .num_iovecs = 1,
      .handles = nullptr,
      .handle_metadata = nullptr,
      .num_handles = 0,
      .is_transactional = true,
  });

  ::fidl::internal::OutgoingTransportContext context =
      ::fidl::internal::OutgoingTransportContext::Create<::fidl::internal::DriverTransport>(
          arena.get());

  txn->Reply(&msg, {.outgoing_transport_context = std::move(context)});
}
}  // namespace fidl::internal
