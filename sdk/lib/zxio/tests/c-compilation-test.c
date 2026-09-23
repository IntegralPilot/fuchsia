// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// To test C compilation, import every single public header from C:

#include <lib/zxio/bsdsocket.h>
#include <lib/zxio/null.h>
#include <lib/zxio/ops.h>
#include <lib/zxio/types.h>
#include <lib/zxio/watcher.h>
#include <lib/zxio/zxio.h>

#include <zxtest/zxtest.h>

// Tests that the zxio headers can be used from C.

void test_destroy(zxio_t* io) {}

TEST(Zxio, UseFromC) {
  zxio_storage_t object;
  zxio_ops_t ops = {};
  memset(&ops, 0, sizeof(ops));
  ops.destroy = test_destroy;
  zxio_init(&object.io, &ops);
  zxio_destroy(&object.io);
}
