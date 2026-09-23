// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.runner/cpp/natural_types.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>

namespace fdf_testing {

ScopedGlobalLogger::ScopedGlobalLogger(FuchsiaLogSeverity min_severity)
    : loop_(&kAsyncLoopConfigNeverAttachToThread) {
  ZX_ASSERT_MSG(!fdf::Logger::HasGlobalInstance(), "There is already an active logger.");

  zx::result open_result = component::OpenServiceRoot();
  ZX_ASSERT(open_result.is_ok());

  std::vector<fuchsia_component_runner::ComponentNamespaceEntry> entries;
  fidl::ClientEnd<fuchsia_io::Directory> svc = std::move(open_result).value();
  entries.emplace_back(fuchsia_component_runner::ComponentNamespaceEntry{{
      .path = "/svc",
      .directory = std::move(svc),
  }});

  // Create Namespace object from the entries.
  zx::result<fdf::Namespace> ns_result = fdf::Namespace::Create(entries);
  ZX_ASSERT(ns_result.is_ok());

  // Create Logger with dispatcher and namespace.
#if FUCHSIA_API_LEVEL_AT_LEAST(24)
  std::unique_ptr<fdf::Logger> logger =
      fdf::Logger::Create2(std::move(ns_result).value(), loop_.dispatcher(),
                           "fdf-testing-scoped-global-logger", min_severity);
  logger_ = std::move(logger);
#else
  zx::result<std::unique_ptr<fdf::Logger>> logger =
      fdf::Logger::Create(std::move(ns_result).value(), loop_.dispatcher(),
                          "fdf-testing-scoped-global-logger", min_severity);
  ZX_ASSERT(logger.is_ok());
  logger_ = std::move(logger).value();
#endif

  fdf::Logger::SetGlobalInstance(logger_.get());
}

ScopedGlobalLogger::~ScopedGlobalLogger() { fdf::Logger::SetGlobalInstance(nullptr); }

}  // namespace fdf_testing
