// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>

#include "test-start.h"
#include "zygote.h"

// The zygote-dep in the secondary domain will call this function, while the
// zygote-dep in the first domain calls the one in the main zygote executable.
// However, initialized_data here resolves to the executable's symbol, because
// it's also in the secondary domain's resolution list, but after this module.
__EXPORT int called_by_zygote_dep() {
  static int** data = &initialized_data[2];
  return *(*data--)++ + zygote_test_main();
}

extern "C" int64_t TestStart() { return zygote_dep() + 1; }
