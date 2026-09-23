// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_FIDL_DYNSUITE_CHANNEL_UTIL_CHANNEL_H_
#define SRC_TESTS_FIDL_DYNSUITE_CHANNEL_UTIL_CHANNEL_H_

#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/fidl.h>
#include <zircon/types.h>

#include <utility>

#include "src/tests/fidl/dynsuite/channel_util/bytes.h"

namespace channel_util {

// A handle to send on a channel. This is like zx_handle_disposition_t but with
// defaults for type and rights.
struct Handle {
  zx_handle_t handle;
  zx_obj_type_t type = ZX_OBJ_TYPE_NONE;
  zx_rights_t rights = ZX_RIGHT_SAME_RIGHTS;
};

// A handle we expect to receive on a channel. This is like zx_handle_info_t but
// with defaults for type and rights, and no zx_handle_t field. Instead, you can
// set the koid to assert on the koid of the received handle.
struct ExpectedHandle {
  zx_koid_t koid = ZX_KOID_INVALID;
  zx_obj_type_t type = ZX_OBJ_TYPE_NONE;
  zx_rights_t rights = ZX_RIGHT_SAME_RIGHTS;
};

using Handles = std::vector<Handle>;
using ExpectedHandles = std::vector<ExpectedHandle>;

// A message to write to a channel.
struct Message {
  Bytes bytes;
  Handles handles = {};

  // NOLINTNEXTLINE(google-explicit-constructor)
  Message(Bytes bytes) : bytes(std::move(bytes)) {}
  Message(Bytes bytes, Handles handles) : bytes(std::move(bytes)), handles(std::move(handles)) {}
};

// A message that is expected when reading from a channel.
struct ExpectedMessage {
  Bytes bytes;
  ExpectedHandles handles = {};

  // NOLINTNEXTLINE(google-explicit-constructor)
  ExpectedMessage(Bytes bytes) : bytes(std::move(bytes)) {}
  ExpectedMessage(Bytes bytes, ExpectedHandles handles)
      : bytes(std::move(bytes)), handles(std::move(handles)) {}
};

// Timeout to use when waiting for a signal on a channel.
const zx::duration kWaitTimeout = zx::sec(10);

class Channel {
 public:
  Channel() = default;
  explicit Channel(zx::channel channel) : channel_(std::move(channel)) {}

  zx_status_t write(const Message& message);

  zx_status_t wait_for_signal(zx_signals_t signal) {
    ZX_ASSERT_MSG(__builtin_popcount(signal) == 1, "wait_for_signal expects exactly 1 signal");
    return channel_.wait_one(signal, zx::deadline_after(kWaitTimeout), nullptr);
  }

  bool is_signal_present(zx_signals_t signal) {
    ZX_ASSERT_MSG(__builtin_popcount(signal) == 1, "is_signal_present expects exactly 1 signal");
    return ZX_OK == channel_.wait_one(signal, zx::time::infinite_past(), nullptr);
  }

  zx_status_t read_and_check(const ExpectedMessage& expected) {
    return read_and_check_impl("read_and_check", expected, nullptr);
  }

  zx_status_t read_and_check_unknown_txid(const ExpectedMessage& expected, zx_txid_t* out_txid) {
    return read_and_check_impl("read_and_check_unknown_txid", expected, out_txid);
  }

  zx::channel& get() { return channel_; }
  void reset() { channel_.reset(); }

 private:
  zx_status_t read_and_check_impl(const char* log_prefix, const ExpectedMessage& expected,
                                  zx_txid_t* out_unknown_txid);

  zx::channel channel_;
};

}  // namespace channel_util

#endif  // SRC_TESTS_FIDL_DYNSUITE_CHANNEL_UTIL_CHANNEL_H_
