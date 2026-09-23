// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/log_settings.h>

#include "adb-file-sync.h"

int main(int argc, char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"adb"}).BuildAndInitialize();

  return adb_file_sync::AdbFileSync::StartService(
      adb_file_sync_config::Config::TakeFromStartupHandle());
}
