// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdio>

#include "pw_log_fuchsia/log_fuchsia.h"

extern "C" {
void pw_log_fuchsia_impl(int level, const char* module_name, const char* file_name, int line_number,
                         const char* message) {
  printf("%s: [%s:%s:%d] %s\n", pw_log_fuchsia::LogLevelToString(level), module_name,
         pw_log_fuchsia::BaseName(file_name), line_number, message);
}
}
