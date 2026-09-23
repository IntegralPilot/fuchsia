// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <lib/driver/component/cpp/driver_export2.h>

#include "src/connectivity/ethernet/drivers/virtio/virtio_net_driver.h"

// The tests export their own driver to mock out certain behaviors. They still need access to the
// actual driver though, so this export has to be in a separate file to avoid a duplicate export.
FUCHSIA_DRIVER_EXPORT2(virtio::VirtioNetDriver);
