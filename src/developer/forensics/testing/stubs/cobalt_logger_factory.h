// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_COBALT_LOGGER_FACTORY_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_COBALT_LOGGER_FACTORY_H_

#include <fidl/fuchsia.metrics/cpp/fidl.h>
#include <fidl/fuchsia.metrics/cpp/test_base.h>
#include <lib/async/default.h>

#include <memory>

#include "src/developer/forensics/testing/stubs/cobalt_logger.h"
#include "src/developer/forensics/testing/stubs/fidl_server.h"
#include "src/developer/forensics/utils/cobalt/event.h"

namespace forensics {
namespace stubs {

// Defines the interface all stub logger factories must implement and provides common functionality.
class CobaltLoggerFactoryBase
    : public SingleBindingFidlServer<fuchsia_metrics::MetricEventLoggerFactory> {
 public:
  CobaltLoggerFactoryBase(async_dispatcher_t* dispatcher, std::unique_ptr<CobaltLoggerBase> logger)
      : dispatcher_(dispatcher), logger_(std::move(logger)) {}
  virtual ~CobaltLoggerFactoryBase() {}

  const cobalt::Event& LastEvent() const { return logger_->LastEvent(); }
  const std::vector<cobalt::Event>& Events() const { return logger_->Events(); }

  bool WasMethodCalled(cobalt::EventType name) const { return logger_->WasMethodCalled(name); }

  void CloseLoggerConnection();

 protected:
  async_dispatcher_t* dispatcher_;
  std::unique_ptr<CobaltLoggerBase> logger_;
  std::optional<fidl::ServerBinding<fuchsia_metrics::MetricEventLogger>> logger_binding_;
};

// Always succeed in setting up the logger.
class CobaltLoggerFactory : public CobaltLoggerFactoryBase {
 public:
  explicit CobaltLoggerFactory(
      async_dispatcher_t* dispatcher,
      std::unique_ptr<CobaltLoggerBase> logger = std::make_unique<CobaltLogger>())
      : CobaltLoggerFactoryBase(dispatcher, std::move(logger)) {}

 private:
  // |fuchsia_metrics::MetricEventLoggerFactory|
  void CreateMetricEventLogger(CreateMetricEventLoggerRequest& request,
                               CreateMetricEventLoggerCompleter::Sync& completer) override;
};

// Always close the connection before setting up the logger.
class CobaltLoggerFactoryClosesConnection : public CobaltLoggerFactoryBase {
 public:
  explicit CobaltLoggerFactoryClosesConnection(async_dispatcher_t* dispatcher)
      : CobaltLoggerFactoryBase(dispatcher, std::make_unique<CobaltLoggerBase>()) {}

 private:
  // |fuchsia_metrics::MetricEventLoggerFactory|
  void CreateMetricEventLogger(CreateMetricEventLoggerRequest& request,
                               CreateMetricEventLoggerCompleter::Sync& completer) override;
};

// Fail to create the logger.
class CobaltLoggerFactoryFailsToCreateLogger : public CobaltLoggerFactoryBase {
 public:
  explicit CobaltLoggerFactoryFailsToCreateLogger(async_dispatcher_t* dispatcher)
      : CobaltLoggerFactoryBase(dispatcher, std::make_unique<CobaltLoggerBase>()) {}

 private:
  // |fuchsia_metrics::MetricEventLoggerFactory|
  void CreateMetricEventLogger(CreateMetricEventLoggerRequest& request,
                               CreateMetricEventLoggerCompleter::Sync& completer) override;
};

// Fail to create the logger until |succeed_after_| attempts have been made.
class CobaltLoggerFactoryCreatesOnRetry : public CobaltLoggerFactoryBase {
 public:
  CobaltLoggerFactoryCreatesOnRetry(async_dispatcher_t* dispatcher, uint64_t succeed_after)
      : CobaltLoggerFactoryBase(dispatcher, std::make_unique<CobaltLogger>()),
        succeed_after_(succeed_after) {}

 private:
  // |fuchsia_metrics::MetricEventLoggerFactory|
  void CreateMetricEventLogger(CreateMetricEventLoggerRequest& request,
                               CreateMetricEventLoggerCompleter::Sync& completer) override;

  const uint64_t succeed_after_;
  uint64_t num_calls_ = 0;
};

// Delay calling the callee provided callback by the specified delay.
class CobaltLoggerFactoryDelaysCallback : public CobaltLoggerFactoryBase {
 public:
  CobaltLoggerFactoryDelaysCallback(std::unique_ptr<CobaltLoggerBase> logger,
                                    async_dispatcher_t* dispatcher, zx::duration delay)
      : CobaltLoggerFactoryBase(dispatcher, std::move(logger)), delay_(delay) {}

 private:
  // |fuchsia_metrics::MetricEventLoggerFactory|
  void CreateMetricEventLogger(CreateMetricEventLoggerRequest& request,
                               CreateMetricEventLoggerCompleter::Sync& completer) override;

  zx::duration delay_;
};

}  // namespace stubs
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_COBALT_LOGGER_FACTORY_H_
