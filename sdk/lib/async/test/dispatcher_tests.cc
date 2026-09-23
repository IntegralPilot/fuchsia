// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-testing/dispatcher_stub.h>
#include <lib/async/cpp/irq.h>

#include <gtest/gtest.h>

namespace {

namespace change_detector_test {
static_assert(sizeof(async_ops_t) == 128);
static_assert(offsetof(async_ops_t, version) == 0);
static_assert(offsetof(async_ops_t, reserved) == 4);
static_assert(offsetof(async_ops_t, v1) == 8);
static_assert(offsetof(async_ops_t, v2) == 64);
static_assert(offsetof(async_ops_t, v3) == 96);
static_assert(offsetof(async_ops_t, v4) == 112);
}  // namespace change_detector_test

}  // namespace
