// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.dictionaryoffers.test/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>

namespace ft = fuchsia_dictionaryoffers_test;

namespace {

class ChildDriver : public fdf::DriverBase2 {
 public:
  ChildDriver() : fdf::DriverBase2("child") {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto control = context.incoming().Connect<ft::ControlService::Control>();
    if (control.is_ok()) {
      fidl::Result result = fidl::Call(control.value())->Check();
      if (result.is_error()) {
        fdf::error("Failed to call Check: {}", result.error_value().FormatDescription());
        return zx::error(ZX_ERR_INTERNAL);
      }

      fdf::info("child successfully connected and called the control plane provided.");
    } else {
      fdf::info("child has no control provided");
    }

    auto data = context.incoming().Connect<ft::DataService::Data>();
    if (data.is_error()) {
      if (data.error_value() != ZX_ERR_NOT_FOUND) {
        fdf::warn("Failed to connect to default DataService: {}", data);
      } else {
        fdf::info("switching to left");
      }
      data = context.incoming().Connect<ft::DataService::Data>("left");
      if (data.is_error()) {
        fdf::error("Failed to connect to DataService: {}", data);
        return zx::error(ZX_ERR_INTERNAL);
      }

      data = context.incoming().Connect<ft::DataService::Data>("opt");
      if (data.is_error()) {
        fdf::error("Failed to connect to DataService: {}", data);
        return zx::error(ZX_ERR_INTERNAL);
      }
    }

    fidl::Result result = fidl::Call(data.value())->DataDo();
    if (result.is_error()) {
      fdf::error("Failed to call DataDo: {}", result.error_value().FormatDescription());
      return zx::error(ZX_ERR_INTERNAL);
    }

    fdf::info("child successfully connected and called the data plane provided.");

    return zx::ok();
  }
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(ChildDriver);
