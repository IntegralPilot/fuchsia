// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "process-fixture.h"

namespace object_info_test {

zx::vmar ProcessFixture::vmar_;
zx::process ProcessFixture::process_;
MappingInfo ProcessFixture::info_;

}  // namespace object_info_test
