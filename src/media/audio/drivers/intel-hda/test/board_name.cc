// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "board_name.h"

#include <fidl/fuchsia.sysinfo/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <fbl/string.h>

namespace audio::intel_hda {

// Get the name of the board we are running on.
zx::result<fbl::String> GetBoardName() {
  zx::result sysinfo = component::Connect<fuchsia_sysinfo::SysInfo>();
  if (sysinfo.is_error()) {
    return sysinfo.take_error();
  }
  const fidl::WireResult result = fidl::WireCall(sysinfo.value())->GetBoardName();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(response.name.get());
}

}  // namespace audio::intel_hda
