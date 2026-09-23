// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include <thread>

void MakeWork() {
  for (unsigned i = 0; i < 100000; i++) {
    FX_LOGS(TRACE) << i;
  }
}

int main() {
  for (;;) {
    std::thread t(MakeWork);
    std::thread u(MakeWork);
    std::thread v(MakeWork);
    t.join();
    u.join();
    v.join();
  }
}
