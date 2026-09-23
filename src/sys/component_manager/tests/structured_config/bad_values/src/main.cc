// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

int main(int argc, const char** argv) {
  FX_LOG_KV(ERROR, "This component was started when it wasn't supposed to.");
  return 1;
}
