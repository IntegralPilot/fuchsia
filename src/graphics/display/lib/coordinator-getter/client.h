// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_COORDINATOR_GETTER_CLIENT_H_
#define SRC_GRAPHICS_DISPLAY_LIB_COORDINATOR_GETTER_CLIENT_H_

#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <lib/fpromise/promise.h>

namespace display {

struct CoordinatorClientChannels {
  fidl::ClientEnd<fuchsia_hardware_display::Coordinator> coordinator_client_end;
  fidl::ServerEnd<fuchsia_hardware_display::CoordinatorListener> coordinator_listener_server_end;
};

// Connects to the [`fuchsia.hardware.display/Coordinator`] protocol using the
// `provider` client. The FIDL connection is asynchronously performed on
// `dispatcher`.
//
// Returns a promise which will be resolved when the display coordinator is
// obtained on success, if the display provider service is available and can
// be connected; otherwise returns a fpromise::error.
//
// TODO(https://fxbug.dev/500852294): Add ClientPriority argument.
fpromise::promise<CoordinatorClientChannels, zx_status_t> GetCoordinator(
    fidl::ClientEnd<fuchsia_hardware_display::Provider> provider, async_dispatcher_t* dispatcher);

// Same as `GetCoordinator(provider, dispatcher)`,
// but the FIDL connection is asynchronously performed on the default async
// dispatcher.
//
// TODO(https://fxbug.dev/500852294): Add ClientPriority argument.
fpromise::promise<CoordinatorClientChannels, zx_status_t> GetCoordinator(
    fidl::ClientEnd<fuchsia_hardware_display::Provider> provider);

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_COORDINATOR_GETTER_CLIENT_H_
