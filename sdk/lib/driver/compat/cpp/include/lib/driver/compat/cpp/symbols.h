// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPAT_CPP_SYMBOLS_H_
#define LIB_DRIVER_COMPAT_CPP_SYMBOLS_H_

namespace compat {

struct device_t {
  const char* name;
  void* context;
};

constexpr device_t kDefaultDevice = {
    .name = "compat-device",
    .context = nullptr,
};

// The symbol for the compat device: device_t.
constexpr char kDeviceSymbol[] = "fuchsia.compat.device/Device";

}  // namespace compat

#endif  // LIB_DRIVER_COMPAT_CPP_SYMBOLS_H_
