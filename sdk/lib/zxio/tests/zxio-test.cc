// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zxio/ops.h>
#include <lib/zxio/types.h>
#include <string.h>

#include <zxtest/zxtest.h>

TEST(OpsTest, Close) {
  zxio_ops_t ops = {
      .destroy = [](zxio_t*) {},
      .close = [](zxio_t*) { return ZX_OK; },
  };
  zxio_t io = {};
  ASSERT_EQ(nullptr, zxio_get_ops(&io));

  zxio_init(&io, &ops);

  ASSERT_EQ(&ops, zxio_get_ops(&io));
  ASSERT_OK(zxio_close(&io));
  zxio_destroy(&io);
}

TEST(OpsTest, DestroyWillInvalidateTheObject) {
  zxio_ops_t ops = {
      .destroy = [](zxio_t*) {},
      .close = [](zxio_t*) { return ZX_OK; },
  };

  zxio_t io = {};
  zxio_init(&io, &ops);
  ASSERT_OK(zxio_close(&io));
  zxio_destroy(&io);
  ASSERT_STATUS(zxio_close(&io), ZX_ERR_BAD_HANDLE);
  ASSERT_STATUS(zxio_release(&io, nullptr), ZX_ERR_BAD_HANDLE);
}

TEST(OpsTest, CallingDestroyTwicePanics) {
  zxio_ops_t ops = {
      .destroy = [](zxio_t*) {},
  };

  zxio_t io = {};
  zxio_init(&io, &ops);
  zxio_destroy(&io);
  ASSERT_DEATH([&io] { zxio_destroy(&io); }, "Calling zxio_destroy twice should panic");
}
