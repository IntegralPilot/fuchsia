// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/structured_logger.h>

#include <optional>

namespace {

class PackagedDriver final : public fdf::DriverBase2 {
 public:
  PackagedDriver() : fdf::DriverBase2("packaged") {}

  zx::result<> Start(fdf::DriverContext context) override {
    inspector_.emplace(context.CreateInspector(this));
    inspector_->root().RecordString("hello", "world");

    FDF_SLOG(DEBUG, "Debug world");
    FDF_SLOG(INFO, "Hello world", KV("The answer is", 42));
    return zx::ok();
  }

 private:
  std::optional<inspect::ComponentInspector> inspector_;
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(PackagedDriver);
