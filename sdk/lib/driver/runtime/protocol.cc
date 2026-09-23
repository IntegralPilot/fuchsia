// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdf/cpp/protocol.h>
#include <zircon/assert.h>

namespace fdf {

Protocol::Protocol(Handler handler) : fdf_token_t{CallHandler}, handler_(std::move(handler)) {}

Protocol::~Protocol() { ZX_DEBUG_ASSERT(!is_pending()); }

zx_status_t Protocol::Register(zx::channel token, fdf_dispatcher_t* dispatcher) {
  if (dispatcher_) {
    return ZX_ERR_BAD_STATE;
  }
  dispatcher_ = dispatcher;

#pragma clang diagnostic push  // for fdf_token_register
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
  zx_status_t status = fdf_token_register(token.release(), dispatcher_, this);
#pragma clang diagnostic pop  // "-Wdeprecated-declarations"
  if (status != ZX_OK) {
    dispatcher_ = nullptr;
    return status;
  }
  return ZX_OK;
}

// static
void Protocol::CallHandler(fdf_dispatcher_t* dispatcher, fdf_token_t* token, zx_status_t status,
                           fdf_handle_t handle) {
  auto self = static_cast<Protocol*>(token);
  ZX_ASSERT(self->handler_);
  self->dispatcher_ = nullptr;
  self->handler_(dispatcher, self, status, fdf::Channel(handle));
}

zx_status_t ProtocolConnect(zx::channel token, fdf::Channel channel) {
  return fdf_token_transfer(token.release(), channel.release());
}

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
zx::result<fdf::Channel> ProtocolReceive(zx::channel token) {
  fdf_handle_t handle;
  zx_status_t res = fdf_token_receive(token.release(), &handle);
  if (res != ZX_OK) {
    return zx::error(res);
  }
  return zx::success(fdf::Channel(handle));
}
#endif

}  // namespace fdf
