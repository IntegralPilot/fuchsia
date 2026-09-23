// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_RESOURCE_H_
#define SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_RESOURCE_H_

#include <lib/zx/resource.h>
#include <lib/zx/result.h>

zx::result<zx::resource> GetSystemProfileResource();

#endif  // SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_RESOURCE_H_
