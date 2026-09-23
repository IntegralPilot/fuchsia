// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef VENDOR_HELPER_GENERIC_H
#define VENDOR_HELPER_GENERIC_H

#include <fidl/fuchsia.gpu.magma.test/cpp/wire.h>
#include <lib/syslog/cpp/macros.h>

// This class provides a default VendorHelper implementation.
// Vendor specific server implementations can derive from this generic class to facilitate
// protocol evolution.
class VendorHelperServerGeneric : public fidl::WireServer<fuchsia_gpu_magma_test::VendorHelper> {
 public:
  void GetConfig(GetConfigCompleter::Sync& completer) override {
    fidl::Arena arena;
    auto builder = ::fuchsia_gpu_magma_test::wire::VendorHelperGetConfigResponse::Builder(arena);
    completer.Reply(builder.Build());
  }

  void ValidateCalibratedTimestamps(
      ::fuchsia_gpu_magma_test::wire::VendorHelperValidateCalibratedTimestampsRequest* request,
      ValidateCalibratedTimestampsCompleter::Sync& completer) override {
    FX_CHECK(false);
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_gpu_magma_test::VendorHelper> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGS(WARNING) << "Received an unknown method with ordinal " << metadata.method_ordinal;
  }

  void OnClosed(fidl::UnbindInfo info) {}
};

#endif  // VENDOR_HELPER_GENERIC_H
