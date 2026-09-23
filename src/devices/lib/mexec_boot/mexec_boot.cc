// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "include/lib/mexec_boot/mexec_boot.h"

#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.system.state/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/error-string.h>
#include <lib/zbitl/image.h>
#include <lib/zbitl/item.h>
#include <lib/zbitl/vmo.h>
#include <lib/zx/resource.h>
#include <lib/zx/vmo.h>
#include <zircon/status.h>

#include <src/bringup/lib/mexec/mexec.h>
#include <src/lib/fsl/vmo/sized_vmo.h>
#include <src/lib/fsl/vmo/vector.h>

namespace {

struct MexecVmos {
  zx::vmo kernel_zbi;
  zx::vmo data_zbi;
};

zx::result<MexecVmos> GetMexecZbis(zx::unowned_resource mexec_resource) {
  zx::result client_end = component::Connect<fuchsia_system_state::SystemStateTransition>();
  if (client_end.is_error()) {
    return client_end.take_error();
  }
  fidl::WireSyncClient client(std::move(*client_end));

  fidl::WireResult result = client->GetMexecZbis();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result.value().is_error()) {
    return result.value().take_error();
  }

  zx::vmo kernel_zbi = std::move(result.value().value()->kernel_zbi);
  zx::vmo data_zbi = std::move(result.value().value()->data_zbi);

  if (zx_status_t status = mexec::PrepareDataZbi(mexec_resource->borrow(), data_zbi.borrow());
      status != ZX_OK) {
    return zx::error(status);
  }

  zx::result connect_result = component::Connect<fuchsia_boot::Items>();
  if (connect_result.is_error()) {
    return connect_result.take_error();
  }
  fidl::WireSyncClient<fuchsia_boot::Items> items(std::move(connect_result).value());

  // Driver metadata that the driver framework generally expects to be present.
  constexpr std::array kItemsToAppend{ZBI_TYPE_DRV_MAC_ADDRESS, ZBI_TYPE_DRV_PARTITION_MAP,
                                      ZBI_TYPE_DRV_BOARD_PRIVATE, ZBI_TYPE_DRV_BOARD_INFO};
  zbitl::Image data_image{data_zbi.borrow()};
  for (uint32_t type : kItemsToAppend) {
    // TODO(https://fxbug.dev/42053781): Use a method that returns all matching items of
    // a given type instead of guessing possible `extra` values.
    for (uint32_t extra : std::array{0, 1, 2}) {
      fidl::WireResult result = items->Get(type, extra);
      if (!result.ok()) {
        return zx::error(result.status());
      }
      if (!result.value().payload.is_valid()) {
        // Absence is signified with an empty result value.
        continue;
      }
      fsl::SizedVmo payload(std::move(result.value().payload), result.value().length);

      std::vector<char> contents;
      if (!fsl::VectorFromVmo(payload, &contents)) {
        return zx::error(ZX_ERR_INTERNAL);
      }

      if (fit::result result = data_image.Append(zbi_header_t{.type = type, .extra = extra},
                                                 zbitl::AsBytes(contents));
          result.is_error()) {
        return zx::error(ZX_ERR_INTERNAL);
      }
    }
  }

  return zx::ok(MexecVmos{
      .kernel_zbi = std::move(kernel_zbi),
      .data_zbi = std::move(data_zbi),
  });
}

}  // namespace

extern "C" zx_status_t mexec_boot(zx_handle_t mexec_resource_handle) {
  zx::unowned_resource mexec_resource(mexec_resource_handle);
  zx::result<MexecVmos> mexec_vmos = GetMexecZbis(mexec_resource->borrow());
  if (mexec_vmos.is_error()) {
    return mexec_vmos.status_value();
  }

  zx_status_t status = mexec::BootZbi(std::move(mexec_resource), std::move(mexec_vmos->kernel_zbi),
                                      std::move(mexec_vmos->data_zbi));
  return status;
}
