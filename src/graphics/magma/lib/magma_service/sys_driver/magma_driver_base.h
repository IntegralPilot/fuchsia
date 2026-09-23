// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_DRIVER_BASE_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_DRIVER_BASE_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.gpu.magma/cpp/fidl.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/fit/thread_safety.h>
#include <lib/magma/platform/zircon/zircon_platform_logger_dfv2.h>
#include <lib/magma/platform/zircon/zircon_platform_status.h>
#include <lib/magma/util/macros.h>
#include <lib/scheduler/role.h>
#include <threads.h>
#include <zircon/threads.h>

#include "dependency_injection_server.h"
#include "fidl/fuchsia.gpu.magma/cpp/markers.h"
#include "magma_system_device.h"
#include "performance_counters_server.h"

namespace msd {

class MagmaTestServer;

// The shared objects that the MSD and FIDL server interact with.
struct MagmaObjects {
  std::mutex magma_mutex;
  std::unique_ptr<msd::Driver> magma_driver FIT_GUARDED(magma_mutex);
  std::unique_ptr<MagmaSystemDevice> magma_system_device FIT_GUARDED(magma_mutex);
};

class MagmaCombinedDeviceServer : public fidl::WireServer<fuchsia_gpu_magma::CombinedDevice> {
 public:
  explicit MagmaCombinedDeviceServer(std::shared_ptr<MagmaObjects> magma,
                                     MagmaClientType client_type = MagmaClientType::kUntrusted)
      : magma_(std::move(magma)), client_type_(client_type) {}
  void Query(QueryRequestView request, QueryCompleter::Sync& completer) override;

  void Connect2(Connect2RequestView request, Connect2Completer::Sync& completer) override;
  void DumpState(DumpStateRequestView request, DumpStateCompleter::Sync& completer) override;
  void GetIcdList(GetIcdListCompleter::Sync& completer) override;

 private:
  template <typename T>
  bool CheckSystemDevice(T& completer) FIT_REQUIRES(magma_->magma_mutex) {
    if (!magma_->magma_system_device) {
      MAGMA_LOG(WARNING, "Got message on torn-down device");
      completer.Close(ZX_ERR_BAD_STATE);
      return false;
    }
    return true;
  }

  std::shared_ptr<MagmaObjects> magma_;
  MagmaClientType client_type_;
};

class MagmaDriverBase : public fdf::DriverBase2,
                        public fidl::WireServer<fuchsia_gpu_magma::PowerElementProvider>,
                        public fidl::WireServer<fuchsia_gpu_magma::DebugUtils>,
                        public internal::DependencyInjectionServer::Owner {
 public:
  explicit MagmaDriverBase(std::string_view name, bool serve_untrusted_service = true)
      : DriverBase2(name),
        magma_(std::make_shared<MagmaObjects>()),
        combined_device_server_(magma_, MagmaClientType::kUntrusted),
        trusted_combined_device_server_(magma_, MagmaClientType::kTrusted),
        serve_untrusted_service_(serve_untrusted_service) {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // Initialize MagmaDriver and MagmaSystemDevice.
  virtual zx::result<> MagmaStart(fdf::DriverContext& context) = 0;

  void GetPowerGoals(GetPowerGoalsCompleter::Sync& completer) override { completer.Reply({}); }

  void GetClockSpeedLevel(
      ::fuchsia_gpu_magma::wire::PowerElementProviderGetClockSpeedLevelRequest* request,
      GetClockSpeedLevelCompleter::Sync& completer) override;

  void SetClockLimit(::fuchsia_gpu_magma::wire::PowerElementProviderSetClockLimitRequest* request,
                     SetClockLimitCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_gpu_magma::PowerElementProvider> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  zx::result<zx::resource> GetInfoResource();

  std::mutex& magma_mutex() FIT_RETURN_CAPABILITY(magma_->magma_mutex) {
    return magma_->magma_mutex;
  }

  msd::Driver* magma_driver() FIT_REQUIRES(magma_->magma_mutex) {
    return magma_->magma_driver.get();
  }

  void set_magma_driver(std::unique_ptr<msd::Driver> magma_driver)
      FIT_REQUIRES(magma_->magma_mutex);

  void set_magma_system_device(std::unique_ptr<MagmaSystemDevice> magma_system_device)
      FIT_REQUIRES(magma_->magma_mutex);

  MagmaSystemDevice* magma_system_device() FIT_REQUIRES(magma_->magma_mutex);

  zx::result<> CreateTestService(MagmaTestServer& test_server);

  void SetPowerState(
      fuchsia_gpu_magma::wire::DebugUtilsSetPowerStateRequest* request,
      fidl::WireServer<::fuchsia_gpu_magma::DebugUtils>::SetPowerStateCompleter::Sync& completer)
      override;

 protected:
  std::shared_ptr<fdf::Namespace> incoming() { return incoming_; }

 private:
  zx::result<> ExportServices();

  void InitializeInspector();

  // DependencyInjection::Owner implementation.
  void SetMemoryPressureLevel(MagmaMemoryPressureLevel level) override;

  fit::deferred_callback teardown_logger_callback_;

  std::shared_ptr<MagmaObjects> magma_;
  std::shared_ptr<fdf::Namespace> incoming_;
  std::optional<inspect::ComponentInspector> component_inspector_;
  MagmaCombinedDeviceServer combined_device_server_;
  MagmaCombinedDeviceServer trusted_combined_device_server_;

  internal::PerformanceCountersServer perf_counter_{dispatcher()};
  internal::DependencyInjectionServer dependency_injection_{this, dispatcher()};
  bool serve_untrusted_service_;
};

class MagmaTestServer : public fidl::WireServer<fuchsia_gpu_magma::TestDevice2> {
 public:
  void GetUnitTestStatus(GetUnitTestStatusCompleter::Sync& completer) override {
    MAGMA_DLOG("MagmaTestServer::GetUnitTestStatus");
    completer.Reply(unit_test_status_);
  }
  void set_unit_test_status(zx_status_t status) { unit_test_status_ = status; }

 private:
  zx_status_t unit_test_status_ = ZX_ERR_NOT_FOUND;
};

}  // namespace msd

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_DRIVER_BASE_H_
