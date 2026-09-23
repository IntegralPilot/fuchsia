// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_RESUME_COMPLETER_H_
#define LIB_DRIVER_COMPONENT_CPP_RESUME_COMPLETER_H_

#include <lib/driver/component/cpp/start_completer.h>

namespace fdf {

// This is the completer for the Resume operation in |DriverBase|.
class ResumeCompleter final : public Completer {
 public:
  using Completer::Completer;
  using Completer::operator();
};

}  // namespace fdf

#endif  // LIB_DRIVER_COMPONENT_CPP_RESUME_COMPLETER_H_
