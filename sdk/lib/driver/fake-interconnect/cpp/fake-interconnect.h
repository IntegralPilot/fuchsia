// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_INTERCONNECT_CPP_FAKE_INTERCONNECT_H_
#define LIB_DRIVER_FAKE_INTERCONNECT_CPP_FAKE_INTERCONNECT_H_

#include <fidl/fuchsia.hardware.interconnect/cpp/fidl.h>
#include <fidl/fuchsia.hardware.interconnect/cpp/test_base.h>
#include <lib/fdf/cpp/dispatcher.h>

#include <optional>
#include <utility>

namespace fdf_fake {

class FakeInterconnect : public fidl::testing::TestBase<fuchsia_hardware_interconnect::Path> {
 public:
  explicit FakeInterconnect(
      async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher());

  fuchsia_hardware_interconnect::PathService::InstanceHandler GetInstanceHandler();

  fidl::ClientEnd<fuchsia_hardware_interconnect::Path> Connect();

  // fidl::Server<fuchsia_hardware_interconnect::Path> overrides
  void SetBandwidth(SetBandwidthRequest& request, SetBandwidthCompleter::Sync& completer) override;

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_interconnect::Path> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // Accessors for test verification
  std::pair<std::optional<uint64_t>, std::optional<uint64_t>> bandwidth_bps();
  std::optional<uint64_t> average_bandwidth_bps() const { return bandwidth_bps_.first; }
  std::optional<uint64_t> peak_bandwidth_bps() const { return bandwidth_bps_.second; }

 private:
  async_dispatcher_t* dispatcher_;
  std::pair<std::optional<uint64_t>, std::optional<uint64_t>> bandwidth_bps_;
  fidl::ServerBindingGroup<fuchsia_hardware_interconnect::Path> bindings_;
};

}  // namespace fdf_fake

#endif  // LIB_DRIVER_FAKE_INTERCONNECT_CPP_FAKE_INTERCONNECT_H_
