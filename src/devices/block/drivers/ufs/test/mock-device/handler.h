// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_HANDLER_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_HANDLER_H_

#include <functional>

template <typename Self>
static void DoResetExtra(Self* self) {
  if constexpr (requires { self->ResetExtra(); }) {
    self->ResetExtra();
  }
}

#define DEF_DEFAULT_HANDLER_BEGIN(opcode_type, handler_func_type) \
  using OpcodeType = opcode_type;                                 \
  using HandlerFuncType = handler_func_type;                      \
  void SetHook(opcode_type opcode, handler_func_type func) {      \
    handlers_[opcode] = std::move(func);                          \
  }                                                               \
  void Reset() {                                                  \
    handlers_ = default_handlers_;                                \
    DoResetExtra(this);                                           \
  }                                                               \
  const std::unordered_map<opcode_type, handler_func_type> default_handlers_ = {
#define DEF_DEFAULT_HANDLER_END() \
  }                               \
  ;                               \
  std::unordered_map<OpcodeType, HandlerFuncType> handlers_ = default_handlers_;
#define DEF_DEFAULT_HANDLER(opcode, default_func_name) {opcode, default_func_name},

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_HANDLER_H_
