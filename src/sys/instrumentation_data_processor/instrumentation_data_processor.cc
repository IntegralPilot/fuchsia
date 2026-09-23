// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/debugdata/datasink.h>
#include <lib/syslog/cpp/macros.h>

#include "src/sys/instrumentation_data_processor/instrumentation_data_publisher.h"

using DataSinkCallback = fit::function<void(std::string)>;

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  FX_LOGS(INFO) << "Started instrumentation data processor";

  async_dispatcher_t* dispatcher = loop.dispatcher();
  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);
  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return -1;
  }

  constexpr char data_sink_dir[] = "/tmp";
  fbl::unique_fd data_sink_dir_fd = fbl::unique_fd(open(data_sink_dir, O_RDWR | O_DIRECTORY));
  if (!data_sink_dir_fd) {
    FX_LOGS(ERROR) << "Could not open output directory: " << data_sink_dir;
    return -1;
  }

  debugdata::DataSink debug_data_sink(data_sink_dir_fd);
  debugdata::DataSinkCallback error_callback = [&](const std::string& error) {
    FX_LOGS(ERROR) << "ProcessInstrumentationData: " << error;
  };
  debugdata::DataSinkCallback warning_callback = [&](const std::string& warning) {
    FX_LOGS(WARNING) << "ProcessInstrumentationData: " << warning;
  };

  instrumentation_data::InstrumentationDataPublisher publisher(
      dispatcher, [&](const std::string& data_sink, zx::vmo vmo) {
        debug_data_sink.ProcessSingleDebugData(data_sink, std::move(vmo), {}, error_callback,
                                               warning_callback);
        debug_data_sink.FlushToDirectory(error_callback, warning_callback);
      });

  result = outgoing.AddUnmanagedProtocol<fuchsia_debugdata::Publisher>(
      [&](fidl::ServerEnd<fuchsia_debugdata::Publisher> server_end) {
        fidl::BindServer(dispatcher, std::move(server_end), &publisher);
      });

  publisher.DrainData();

  loop.Run();

  return 0;
}
