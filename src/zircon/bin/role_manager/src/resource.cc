// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "resource.h"

#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>

zx::result<zx::resource> GetSystemProfileResource() {
  zx::result client = component::Connect<fuchsia_kernel::ProfileResource>();
  if (client.is_error()) {
    return client.take_error();
  }
  fidl::WireResult result = fidl::WireCall(*client)->Get();
  if (result.status() != ZX_OK) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}
