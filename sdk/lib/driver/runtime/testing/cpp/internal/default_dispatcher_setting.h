// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_DEFAULT_DISPATCHER_SETTING_H_
#define LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_DEFAULT_DISPATCHER_SETTING_H_

#include <lib/fdf/cpp/dispatcher.h>

namespace fdf_internal {

class DefaultDispatcherSetting {
 public:
  explicit DefaultDispatcherSetting(fdf_dispatcher_t* dispatcher);
  ~DefaultDispatcherSetting();
};

}  // namespace fdf_internal

#endif  // LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_DEFAULT_DISPATCHER_SETTING_H_
