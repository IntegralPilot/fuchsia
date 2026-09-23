// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.memory.stacktrack.process/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/zx/channel.h>
#include <zircon/assert.h>

#include "stacktrack/bind.h"

static constexpr const char *kServicePath =
    fidl::DiscoverableProtocolDefaultPath<fuchsia_memory_stacktrack_process::Registry>;

__EXPORT void stacktrack_bind_with_fdio(void) {
  zx::channel local, remote;
  auto status = zx::make_result(zx::channel::create(0, &local, &remote));
  if (status.is_ok()) {
    fdio_service_connect(kServicePath, remote.release());
  }

  stacktrack_bind_with_channel(local.release());
}
