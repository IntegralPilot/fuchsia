// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdlib.h>

#include <kernel/thread.h>

#include "lib/console.h"
#include "tests.h"

static int spinner_thread(void* arg) {
  for (;;) {
    __asm__ volatile("");
  }

  return 0;
}

// This is the source of the `k spinner` command.
int spinner(int argc, const cmd_args* argv, uint32_t) {
  if (argc < 2) {
    printf("not enough args\n");
    printf("usage: %s <priority>\n", argv[0].str);
    return -1;
  }

  Thread* t = Thread::Create("spinner", spinner_thread, NULL, (int)argv[1].u);
  if (!t)
    return ZX_ERR_NO_MEMORY;

  t->DetachAndResume();

  return 0;
}
