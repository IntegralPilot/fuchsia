// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/compiler.h>

#include "src/lib/dso/cpp/sync.h"

int dso_main(int argc, const char** argv, const char** envp) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"simple_dso"}).BuildAndInitialize();
  FX_LOGS(INFO) << "Hello world!";
  return 0;
}
