// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_MACROS_H_
#define SRC_SYSMEM_SERVER_MACROS_H_

#include "src/sysmem/server/logging.h"

#define LOG(severity, fmt, ...)                                                         \
  ::sysmem_service::Log(__FX_LOG_SEVERITY_##severity, __FILE__, __LINE__, nullptr, fmt, \
                        ##__VA_ARGS__)

#endif  // SRC_SYSMEM_SERVER_MACROS_H_
