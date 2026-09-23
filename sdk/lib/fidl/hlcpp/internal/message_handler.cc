// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/fidl/cpp/internal/message_handler.h"

namespace fidl {
namespace internal {

MessageHandler::~MessageHandler() = default;

void MessageHandler::OnChannelGone() {}

zx_status_t SingleUseMessageHandler::operator()(fidl::HLCPPIncomingMessage message) {
  if (type_ != nullptr) {
    const char* error_msg = nullptr;
    zx_status_t status = message.Decode(type_, &error_msg);
    if (status != ZX_OK) {
      FIDL_REPORT_DECODING_ERROR(message, type_, error_msg);
      return status;
    }
  } else if (unlikely(!message.has_only_header())) {
    return ZX_ERR_INVALID_ARGS;
  }

  zx_status_t status = invoke_(this, std::move(message));
  invoke_ = nullptr;
  destroy_(this);
  return status;
}

SingleUseMessageHandler::~SingleUseMessageHandler() {
  if (invoke_)
    destroy_(this);
}

}  // namespace internal
}  // namespace fidl
