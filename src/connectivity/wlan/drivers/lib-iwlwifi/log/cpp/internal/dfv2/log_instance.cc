// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zircon/assert.h>

#include <wlan/drivers/log_instance.h>

namespace wlan::drivers::log {

// static
void Instance::Init(uint32_t filter, fdf::Logger* logger) {
  Instance& inst = get();

  ZX_ASSERT(logger != nullptr);
  ZX_ASSERT(inst.logger_ == nullptr);
  inst.filter_ = filter;
  inst.logger_ = logger;
}

// static
bool Instance::IsFilterOn(uint32_t filter) {
  Instance& inst = get();
  ZX_ASSERT(inst.logger_ != nullptr);
  return (inst.filter_ & filter) != 0;
}

// static
fdf::Logger* Instance::GetLogger() {
  Instance& inst = get();
  ZX_ASSERT(inst.logger_ != nullptr);
  return inst.logger_;
}

// static
Instance& Instance::get() {
  static Instance inst{};
  return inst;
}

// static
void Instance::Reset() {
  Instance& inst = get();
  inst.filter_ = 0;
  inst.logger_ = nullptr;
}

}  // namespace wlan::drivers::log
