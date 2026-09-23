// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma_service/msd.h>

namespace msd {

Driver::~Driver() = default;
Device::~Device() = default;
Connection::~Connection() = default;
Context::~Context() = default;
Buffer::~Buffer() = default;
Semaphore::~Semaphore() = default;
PerfCountPool::~PerfCountPool() = default;

}  // namespace msd
