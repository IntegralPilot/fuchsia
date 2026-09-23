// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/binding_driver.h>

#include "driver_ctx.h"

namespace {

zx_driver_ops_t amlogic_video_driver_ops = {
    .version = DRIVER_OPS_VERSION,
    .init = amlogic_decoder::DriverCtx::Init,
    .bind = amlogic_decoder::DriverCtx::Bind,
    .release = amlogic_decoder::DriverCtx::Release,
};

}  // namespace

ZIRCON_DRIVER(amlogic_video, amlogic_video_driver_ops, "zircon", "0.1");
