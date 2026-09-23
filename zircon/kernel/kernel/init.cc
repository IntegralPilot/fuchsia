// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2013 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "kernel/init.h"

#include <kernel/mp.h>
#include <kernel/percpu.h>

// Called at LK_INIT_LEVEL_KERNEL
void kernel_init() {
  mp_init();

  // Allocate secondary percpu instances before booting other processors, after
  // vm and system topology are initialized.
  percpu::InitializeSecondariesBegin();
}
