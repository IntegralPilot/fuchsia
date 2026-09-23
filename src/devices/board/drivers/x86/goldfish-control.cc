// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/platform-defs.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/google/platform/cpp/bind.h>

#include "src/devices/board/drivers/x86/x86.h"

namespace x86 {

const ddk::BindRule kGoldfishPipeRules[] = {
    ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.goldfish.pipe.Service"),
    ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                            bind_fuchsia_google_platform::BIND_PLATFORM_DEV_VID_GOOGLE),
    ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_PID,
                            bind_fuchsia_google_platform::BIND_PLATFORM_DEV_PID_GOLDFISH),
    ddk::MakeAcceptBindRule(
        bind_fuchsia::PLATFORM_DEV_DID,
        bind_fuchsia_google_platform::BIND_PLATFORM_DEV_DID_GOLDFISH_PIPE_CONTROL),

};

const device_bind_prop_t kGoldfishPipeProperties[] = {
    ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.goldfish.pipe.Service"),
    ddk::MakeProperty(bind_fuchsia::PLATFORM_DEV_VID,
                      bind_fuchsia_google_platform::BIND_PLATFORM_DEV_VID_GOOGLE),
    ddk::MakeProperty(bind_fuchsia::PLATFORM_DEV_PID,
                      bind_fuchsia_google_platform::BIND_PLATFORM_DEV_PID_GOLDFISH),
    ddk::MakeProperty(bind_fuchsia::PLATFORM_DEV_DID,
                      bind_fuchsia_google_platform::BIND_PLATFORM_DEV_DID_GOLDFISH_PIPE_CONTROL),
};

const ddk::BindRule kGoldfishAddressSpaceRules[] = {
    ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.goldfish.AddressSpaceService"),
};

const device_bind_prop_t kGoldfishAddressSpaceProperties[] = {
    ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.goldfish.AddressSpaceService"),
};

const ddk::BindRule kGoldfishSyncRules[] = {
    ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.goldfish.SyncService"),
};

const device_bind_prop_t kGoldfishSyncProperties[] = {
    ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.goldfish.SyncService"),
};

zx_status_t X86::GoldfishControlInit() {
  zx_status_t status = DdkAddCompositeNodeSpec(
      "goldfish-control-2",
      ddk::CompositeNodeSpec(kGoldfishPipeRules, kGoldfishPipeProperties)
          .AddParentSpec(kGoldfishAddressSpaceRules, kGoldfishAddressSpaceProperties)
          .AddParentSpec(kGoldfishSyncRules, kGoldfishSyncProperties));

  if (status != ZX_OK) {
    zxlogf(ERROR, "goldfish-control-2: DdkAddCompositeNodeSpec failed: %s",
           zx_status_get_string(status));
    return status;
  }
  return status;
}

}  // namespace x86
