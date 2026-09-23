// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "trace_controller.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/start.h>
#include <zircon/errors.h>

#include <fstream>

#include "src/lib/fsl/socket/blocking_drain.h"

fit::result<fit::failed, Tracer> StartTracing(fuchsia_tracing_controller::TraceConfig trace_config,
                                              const char* output_file) {
  trace_provider_start();

  zx::socket trace_socket, outgoing_socket;
  if (zx_status_t status = zx::socket::create(ZX_SOCKET_STREAM, &trace_socket, &outgoing_socket);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "failed to create zircon socket";
    return fit::failed();
  }

  fidl::SyncClient<fuchsia_tracing_controller::Provisioner> launcher;
  {
    zx::result launcher_client = component::Connect<fuchsia_tracing_controller::Provisioner>();
    if (launcher_client.is_error()) {
      FX_PLOGS(ERROR, launcher_client.status_value())
          << "failed to connect to fuchsia.tracing.controller/Provisioner";
      return fit::failed();
    }
    launcher.Bind(std::move(*launcher_client));
  }

  auto endpoints = fidl::Endpoints<fuchsia_tracing_controller::Session>::Create();
  if (fit::result<fidl::OneWayError> response = launcher->InitializeTracing(
          {std::move(endpoints.server), std::move(trace_config), std::move(outgoing_socket)});
      response.is_error()) {
    FX_LOGS(ERROR) << "failed to initialize tracing: " << response.error_value();
    return fit::failed();
  }
  fidl::SyncClient controller{std::move(endpoints.client)};

  if (fidl::Result<fuchsia_tracing_controller::Session::StartTracing> response =
          controller->StartTracing({});
      response.is_error()) {
    FX_LOGS(ERROR) << "failed to start tracing: " << response.error_value();
    return fit::failed();
  }

  std::future<zx_status_t> fut = std::async(
      std::launch::async,
      [](zx::socket trace_socket, std::string output_file) {
        std::ofstream ofs(output_file);
        if (!fsl::BlockingDrainFrom(std::move(trace_socket), [&](const void* data, uint32_t len) {
              const char* begin = static_cast<const char*>(data);
              ofs.write(begin, len);
              return len;
            })) {
          FX_LOGS(ERROR) << "failed to write trace bytes to output file";
          return ZX_ERR_PEER_CLOSED;
        };

        return ZX_OK;
      },
      std::move(trace_socket), output_file);

  return fit::ok(Tracer{.controller = std::move(controller), .future = std::move(fut)});
}

fit::result<fit::failed> StopTracing(Tracer tracer) {
  fuchsia_tracing_controller::StopOptions stop_options;
  stop_options.write_results(true);

  if (fidl::Result<fuchsia_tracing_controller::Session::StopTracing> response =
          tracer.controller->StopTracing({stop_options});
      response.is_error()) {
    FX_LOGS(ERROR) << "failed to stop tracing: " << response.error_value();
    return fit::failed();
  }
  tracer.controller = fidl::SyncClient<fuchsia_tracing_controller::Session>();

  tracer.future.wait();
  if (zx_status_t status = tracer.future.get(); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "error reading trace";
    return fit::failed();
  }
  return fit::ok();
}
