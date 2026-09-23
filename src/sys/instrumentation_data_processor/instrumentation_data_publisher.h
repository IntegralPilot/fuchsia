// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_INSTRUMENTATION_DATA_PROCESSOR_INSTRUMENTATION_DATA_PUBLISHER_H_
#define SRC_SYS_INSTRUMENTATION_DATA_PROCESSOR_INSTRUMENTATION_DATA_PUBLISHER_H_

#include <fidl/fuchsia.debugdata/cpp/wire.h>
#include <lib/async/cpp/wait.h>

#include <list>
#include <memory>
#include <string>
#include <tuple>

namespace instrumentation_data {

// InstrumentationDataPublisher implements the |fuchsia.debugdata.Publisher| protocol.
class InstrumentationDataPublisher : public fidl::WireServer<fuchsia_debugdata::Publisher> {
 public:
  using VmoHandler = fit::function<void(std::string, zx::vmo)>;

  explicit InstrumentationDataPublisher(async_dispatcher_t* dispatcher, VmoHandler vmo_callback);

  ~InstrumentationDataPublisher() override = default;

  void Publish(PublishRequestView request, PublishCompleter::Sync& completer) override;

  void DrainData();

 private:
  async_dispatcher_t* const dispatcher_;
  VmoHandler vmo_callback_;
  std::list<std::tuple<std::shared_ptr<async::WaitOnce>, std::string, zx::vmo>> pending_handlers_;
};
}  // namespace instrumentation_data

#endif  // SRC_SYS_INSTRUMENTATION_DATA_PROCESSOR_INSTRUMENTATION_DATA_PUBLISHER_H_
