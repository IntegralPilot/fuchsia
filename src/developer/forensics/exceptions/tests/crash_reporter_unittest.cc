// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/developer/forensics/exceptions/handler/crash_reporter.h"

#include <fidl/fuchsia.driver.crash/cpp/fidl.h>
#include <fuchsia/feedback/cpp/fidl.h>
#include <fuchsia/sys2/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <zircon/status.h>
#include <zircon/syscalls/exception.h>
#include <zircon/types.h>

#include <memory>
#include <queue>
#include <utility>

#include <gtest/gtest.h>

#include "src/developer/forensics/exceptions/constants.h"
#include "src/developer/forensics/exceptions/tests/crasher_wrapper.h"
#include "src/developer/forensics/testing/gmatchers.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/stubs/wake_lease.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/lib/fostr/fidl/fuchsia/exception/formatting.h"
#include "src/lib/fsl/handles/object_info.h"

namespace forensics {
namespace exceptions {
namespace handler {

inline void ToString(const fuchsia::exception::ExceptionType& value, std::ostream* os) {
  *os << value;
}

namespace {

using ::testing::UnorderedElementsAreArray;

constexpr zx::duration kDefaultTimeout{zx::duration::infinite()};

constexpr std::string kDriverUrl = "test-driver-url";
constexpr std::string kDriverMoniker = "test-driver-moniker";

class StubCrashReporter : public fuchsia::feedback::CrashReporter {
 public:
  void FileReport(fuchsia::feedback::CrashReport report, FileReportCallback callback) override {
    reports_.push_back(std::move(report));

    fuchsia::feedback::CrashReporter_FileReport_Result result;
    result.set_response({});
    callback(std::move(result));
  }

  fidl::InterfaceRequestHandler<fuchsia::feedback::CrashReporter> GetHandler() {
    return [this](fidl::InterfaceRequest<fuchsia::feedback::CrashReporter> request) {
      bindings_.AddBinding(this, std::move(request));
    };
  }

  const std::vector<fuchsia::feedback::CrashReport>& reports() const { return reports_; }

 private:
  std::vector<fuchsia::feedback::CrashReport> reports_;

  fidl::BindingSet<fuchsia::feedback::CrashReporter> bindings_;
};

class StubCrashReporterDelaysResponse : public fuchsia::feedback::CrashReporter {
 public:
  void FileReport(fuchsia::feedback::CrashReport report, FileReportCallback callback) override {
    pending_callbacks_.push(std::move(callback));
  }

  void PopResponse() {
    FX_CHECK(!pending_callbacks_.empty());

    fuchsia::feedback::CrashReporter_FileReport_Result result;
    result.set_response({});
    pending_callbacks_.front()(std::move(result));
    pending_callbacks_.pop();
  }

  fidl::InterfaceRequestHandler<fuchsia::feedback::CrashReporter> GetHandler() {
    return [this](fidl::InterfaceRequest<fuchsia::feedback::CrashReporter> request) {
      bindings_.AddBinding(this, std::move(request));
    };
  }

 private:
  std::queue<FileReportCallback> pending_callbacks_;
  fidl::BindingSet<fuchsia::feedback::CrashReporter> bindings_;
};

class StubCrashIntrospect : public fuchsia::sys2::CrashIntrospect {
 public:
  struct ComponentInfo {
    std::string url;
    std::string moniker;
  };
  void FindComponentByThreadKoid(uint64_t thread_koid, FindComponentByThreadKoidCallback callback) {
    using namespace fuchsia::sys2;
    if (!tids_to_component_infos_.contains(thread_koid)) {
      fuchsia::component::Error err{fuchsia::component::Error::RESOURCE_NOT_FOUND};
      callback(CrashIntrospect_FindComponentByThreadKoid_Result::WithErr(std::move(err)));
    } else {
      const ComponentInfo& info = tids_to_component_infos_[thread_koid];

      ComponentCrashInfo crash_info;
      crash_info.set_url(info.url).set_moniker(info.moniker);

      callback(CrashIntrospect_FindComponentByThreadKoid_Result::WithResponse(
          CrashIntrospect_FindComponentByThreadKoid_Response(std::move(crash_info))));
    }
  }

  fidl::InterfaceRequestHandler<fuchsia::sys2::CrashIntrospect> GetHandler() {
    return [this](fidl::InterfaceRequest<fuchsia::sys2::CrashIntrospect> request) {
      bindings_.AddBinding(this, std::move(request));
    };
  }

  void AddThreadKoidToComponentInfo(uint64_t thread_koid, ComponentInfo component_info) {
    tids_to_component_infos_[thread_koid] = std::move(component_info);
  }

 private:
  std::map<uint64_t, ComponentInfo> tids_to_component_infos_;

  fidl::BindingSet<fuchsia::sys2::CrashIntrospect> bindings_;
};

class DriverCrashIntrospect final : public fidl::Server<fuchsia_driver_crash::CrashIntrospect> {
 public:
  void FindDriverCrash(FindDriverCrashRequest& request,
                       FindDriverCrashCompleter::Sync& completer) override {
    if (failing_) {
      completer.Reply(fit::error(ZX_ERR_INTERNAL));
      return;
    }

    completer.Reply(fit::ok(fuchsia_driver_crash::CrashIntrospectFindDriverCrashResponse{
        fuchsia_driver_crash::DriverCrashInfo{{
            .url = kDriverUrl,
            .node_moniker = kDriverMoniker,
        }}}));
  }

  fidl::ClientEnd<fuchsia_driver_crash::CrashIntrospect> GetClient(async_dispatcher_t* dispatcher) {
    auto [client, server] = fidl::Endpoints<fuchsia_driver_crash::CrashIntrospect>::Create();
    bindings_.AddBinding(dispatcher, std::move(server), this, fidl::kIgnoreBindingClosure);
    return std::move(client);
  }

  void SetFailing() { failing_ = true; }

 private:
  fidl::ServerBindingGroup<fuchsia_driver_crash::CrashIntrospect> bindings_;
  bool failing_ = false;
};

class HandlerTest : public UnitTestFixture {
 public:
  void HandleException(
      zx::exception exception, zx::duration component_lookup_timeout,
      CrashReporter::SendCallback callback = [](const ::fidl::StringPtr& moniker) {},
      bool driver_introspect = true) {
    handler_ = std::make_unique<CrashReporter>(
        dispatcher(), services(), component_lookup_timeout,
        /*wake_lease=*/nullptr,
        driver_introspect ? driver_introspect_.GetClient(dispatcher())
                          : fidl::ClientEnd<fuchsia_driver_crash::CrashIntrospect>{});

    zx::process process;
    exception.get_process(&process);

    zx::thread thread;
    exception.get_thread(&thread);

    handler_->Send(std::move(exception), std::move(process), std::move(thread),
                   std::move(callback));
    RunLoopUntilIdle();
  }

  void HandleException(
      zx::process process, zx::thread thread, zx::duration component_lookup_timeout,
      CrashReporter::SendCallback callback = [](const ::fidl::StringPtr& moniker) {},
      bool driver_introspect = true) {
    handler_ = std::make_unique<CrashReporter>(
        dispatcher(), services(), component_lookup_timeout,
        /*wake_lease=*/nullptr,
        driver_introspect ? driver_introspect_.GetClient(dispatcher())
                          : fidl::ClientEnd<fuchsia_driver_crash::CrashIntrospect>{});

    handler_->Send(zx::exception{}, std::move(process), std::move(thread), std::move(callback));
    RunLoopUntilIdle();
  }

  void SetUpCrashReporter() { InjectServiceProvider(&crash_reporter_); }
  void SetUpCrashIntrospect() { InjectServiceProvider(&introspect_); }

  void SetupFailingDriverIntrospect() { driver_introspect_.SetFailing(); }

  const StubCrashReporter& crash_reporter() const { return crash_reporter_; }

  StubCrashIntrospect& introspect() { return introspect_; }
  const StubCrashIntrospect& introspect() const { return introspect_; }

 private:
  std::unique_ptr<CrashReporter> handler_{nullptr};

  StubCrashReporter crash_reporter_;
  StubCrashIntrospect introspect_;
  DriverCrashIntrospect driver_introspect_;
};

bool RetrieveExceptionContext(ExceptionContext* pe, const std::string& process_name = "crasher") {
  // Create a process that crashes and obtain the relevant handles and exception.
  // By the time |SpawnCrasher| has returned, the thread has already thrown an exception.
  if (!SpawnCrasher(pe, process_name))
    return false;

  // We mark the exception to be handled. We need this because we pass on the exception to the
  // handler, which will resume it before we get the control back. If we don't mark it as handled,
  // the exception will bubble out of our environment.
  return MarkExceptionAsHandled(pe);
}

// Utilities ---------------------------------------------------------------------------------------

inline void ValidateCrashSignature(const fuchsia::feedback::CrashReport& report,
                                   const std::string& crash_signature) {
  ASSERT_TRUE(report.has_crash_signature());
  EXPECT_EQ(report.crash_signature(), crash_signature);
}

inline void ValidateCrashReport(const fuchsia::feedback::CrashReport& report,
                                const std::string& expected_program_name,
                                const std::string& expected_process_name,
                                const zx_koid_t expected_process_koid,
                                const std::string& expected_thread_name,
                                const zx_koid_t expected_thread_koid,
                                const std::map<std::string, std::string>& expected_annotations) {
  ASSERT_TRUE(report.has_program_name());
  EXPECT_EQ(report.program_name(), expected_program_name);

  ASSERT_TRUE(report.has_specific_report());
  ASSERT_TRUE(report.specific_report().is_native());
  EXPECT_EQ(report.specific_report().native().process_name(), expected_process_name);
  EXPECT_EQ(report.specific_report().native().process_koid(), expected_process_koid);
  EXPECT_EQ(report.specific_report().native().thread_name(), expected_thread_name);
  EXPECT_EQ(report.specific_report().native().thread_koid(), expected_thread_koid);

  if (!expected_annotations.empty()) {
    ASSERT_TRUE(report.has_annotations());

    // Infer the type of |matchers|.
    auto matchers = std::vector({MatchesAnnotation("", "")});
    matchers.clear();

    for (const auto& [k, v] : expected_annotations) {
      matchers.push_back(MatchesAnnotation(k.c_str(), v.c_str()));
    }

    EXPECT_THAT(report.annotations(), UnorderedElementsAreArray(matchers));
  }
}

TEST_F(HandlerTest, NoIntrospectConnection) {
  SetUpCrashReporter();

  // Create the exception.
  ExceptionContext exception;
  ASSERT_TRUE(RetrieveExceptionContext(&exception));

  bool called = false;
  std::optional<std::string> out_moniker{std::nullopt};
  HandleException(std::move(exception.exception), kDefaultTimeout,
                  [&called, &out_moniker](const ::fidl::StringPtr& moniker) {
                    called = true;
                    if (moniker.has_value()) {
                      out_moniker = moniker.value();
                    }
                  });

  ASSERT_TRUE(called);
  ASSERT_FALSE(out_moniker.has_value());
  EXPECT_EQ(crash_reporter().reports().size(), 1u);

  // We kill the jobs. This kills the underlying process. We do this so that the crashed process
  // doesn't get rescheduled. Otherwise the exception on the crash program would bubble out of our
  // environment and create noise on the overall system.
  exception.job.kill();
}

TEST_F(HandlerTest, NoCrashReporterConnection) {
  SetUpCrashIntrospect();

  // Create the exception.
  ExceptionContext exception;
  ASSERT_TRUE(RetrieveExceptionContext(&exception));

  zx::thread thread;
  ASSERT_EQ(exception.exception.get_thread(&thread), ZX_OK);
  const zx_koid_t thread_koid = fsl::GetKoid(thread.get());

  const std::string kComponentUrl = "component_url";
  const std::string kComponentMoniker = "/realm/path/component_name";
  introspect().AddThreadKoidToComponentInfo(thread_koid, StubCrashIntrospect::ComponentInfo{
                                                             .url = kComponentUrl,
                                                             .moniker = kComponentMoniker,
                                                         });

  bool called = false;
  std::optional<std::string> out_moniker{std::nullopt};
  HandleException(std::move(exception.exception), kDefaultTimeout,
                  [&called, &out_moniker](const ::fidl::StringPtr& moniker) {
                    called = true;
                    if (moniker.has_value()) {
                      out_moniker = moniker.value();
                    }
                  });

  ASSERT_TRUE(called);
  ASSERT_TRUE(out_moniker.has_value());
  EXPECT_EQ(out_moniker.value(), "realm/path/component_name");

  // The stub shouldn't be called.
  EXPECT_EQ(crash_reporter().reports().size(), 0u);

  // We kill the jobs. This kills the underlying process. We do this so that the crashed process
  // doesn't get rescheduled. Otherwise the exception on the crash program would bubble out of our
  // environment and create noise on the overall system.
  exception.job.kill();
}

TEST_F(HandlerTest, NoException) {
  SetUpCrashReporter();
  SetUpCrashIntrospect();

  // Create the exception.
  ExceptionContext exception;
  ASSERT_TRUE(RetrieveExceptionContext(&exception));

  zx::process process;
  ASSERT_EQ(exception.exception.get_process(&process), ZX_OK);
  const std::string process_name = fsl::GetObjectName(process.get());
  const zx_koid_t process_koid = fsl::GetKoid(process.get());

  zx::thread thread;
  ASSERT_EQ(exception.exception.get_thread(&thread), ZX_OK);
  const std::string thread_name = fsl::GetObjectName(thread.get());
  const zx_koid_t thread_koid = fsl::GetKoid(thread.get());

  const std::string kComponentUrl = "component_url";
  const std::string kComponentMoniker = "/realm/path/component_name";
  introspect().AddThreadKoidToComponentInfo(thread_koid, StubCrashIntrospect::ComponentInfo{
                                                             .url = kComponentUrl,
                                                             .moniker = kComponentMoniker,
                                                         });
  exception.exception.reset();

  bool called = false;
  std::optional<std::string> out_moniker{std::nullopt};
  HandleException(std::move(process), std::move(thread), zx::duration::infinite(),
                  [&called, &out_moniker](const ::fidl::StringPtr& moniker) {
                    called = true;
                    if (moniker.has_value()) {
                      out_moniker = moniker.value();
                    }
                  });

  ASSERT_TRUE(called);
  ASSERT_TRUE(out_moniker.has_value());
  EXPECT_EQ(out_moniker.value(), "realm/path/component_name");

  ASSERT_EQ(crash_reporter().reports().size(), 1u);
  const ::fuchsia::feedback::CrashReport& report = crash_reporter().reports().front();

  ValidateCrashReport(report, kComponentUrl, process_name, process_koid, thread_name, thread_koid,
                      {{kCrashProcessStateKey, "in exception"}});
  ValidateCrashSignature(report, "fuchsia-no-minidump-exception-expired");

  // We kill the jobs. This kills the underlying process. We do this so that the crashed process
  // doesn't get rescheduled. Otherwise the exception on the crash program would bubble out of our
  // environment and create noise on the overall system.
  exception.job.kill();
}

TEST_F(HandlerTest, ProcessTerminated) {
  SetUpCrashReporter();
  SetUpCrashIntrospect();

  // Create the exception.
  ExceptionContext exception;
  ASSERT_TRUE(RetrieveExceptionContext(&exception));

  zx::process process;
  ASSERT_EQ(exception.exception.get_process(&process), ZX_OK);
  const std::string process_name = fsl::GetObjectName(process.get());
  const zx_koid_t process_koid = fsl::GetKoid(process.get());

  zx::thread thread;
  ASSERT_EQ(exception.exception.get_thread(&thread), ZX_OK);
  const std::string thread_name = fsl::GetObjectName(thread.get());
  const zx_koid_t thread_koid = fsl::GetKoid(thread.get());

  const std::string kComponentUrl = "crasher";

  // We kill the jobs. This kills the underlying process. We do this so that the crashed process
  // doesn't get rescheduled. Otherwise the exception on the crash program would bubble out of our
  // environment and create noise on the overall system.
  exception.job.kill();

  // Must explicitly wait for the job to be terminated to ensure consistency between asan and
  // non-asan.
  exception.job.wait_one(ZX_TASK_TERMINATED, zx::time_monotonic::infinite(), nullptr);

  bool called = false;
  HandleException(std::move(exception.exception), zx::duration::infinite(),
                  [&called](const ::fidl::StringPtr& moniker) { called = true; });

  ASSERT_TRUE(called);

  ASSERT_EQ(crash_reporter().reports().size(), 1u);
  const ::fuchsia::feedback::CrashReport& report = crash_reporter().reports().front();

  ValidateCrashReport(report, kComponentUrl, process_name, process_koid, "", thread_koid,
                      {
                          {"debug.crash.component.url.set", "false"},
                          {kCrashProcessStateKey, "terminated"},
                      });
  ValidateCrashSignature(report, "fuchsia-no-minidump-process-terminated");
}

TEST_F(HandlerTest, DelayForFeedbackCrash) {
  SetUpCrashReporter();
  SetUpCrashIntrospect();

  // Create the exception.
  ExceptionContext exception;
  ASSERT_TRUE(RetrieveExceptionContext(&exception, "feedback.cm"));

  zx::process process;
  ASSERT_EQ(exception.exception.get_process(&process), ZX_OK);
  const std::string process_name = fsl::GetObjectName(process.get());
  const zx_koid_t process_koid = fsl::GetKoid(process.get());

  zx::thread thread;
  ASSERT_EQ(exception.exception.get_thread(&thread), ZX_OK);
  const std::string thread_name = fsl::GetObjectName(thread.get());
  const zx_koid_t thread_koid = fsl::GetKoid(thread.get());

  const std::string kComponentUrl = "feedback.cm";
  const std::string kComponentMoniker = "/realm/path/component_name";
  introspect().AddThreadKoidToComponentInfo(thread_koid, StubCrashIntrospect::ComponentInfo{
                                                             .url = kComponentUrl,
                                                             .moniker = kComponentMoniker,
                                                         });

  bool called = false;
  HandleException(std::move(exception.exception), zx::duration::infinite(),
                  [&called](const ::fidl::StringPtr& moniker) { called = true; });

  ASSERT_FALSE(called);
  RunLoopFor(zx::sec(5));
  ASSERT_TRUE(called);

  ASSERT_EQ(crash_reporter().reports().size(), 1u);
  const ::fuchsia::feedback::CrashReport& report = crash_reporter().reports().front();

  ValidateCrashReport(report, kComponentUrl, process_name, process_koid, thread_name, thread_koid,
                      {
                          {kCrashProcessStateKey, "in exception"},
                      });

  // We kill the jobs. This kills the underlying process. We do this so that the crashed process
  // doesn't get rescheduled. Otherwise the exception on the crash program would bubble out of our
  // environment and create noise on the overall system.
  exception.job.kill();
}

TEST_F(HandlerTest, DriverHostCrash) {
  SetUpCrashReporter();
  SetUpCrashIntrospect();

  // Create the exception.
  ExceptionContext exception;
  ASSERT_TRUE(RetrieveExceptionContext(&exception));

  zx::process process;
  ASSERT_EQ(exception.exception.get_process(&process), ZX_OK);
  const std::string process_name = fsl::GetObjectName(process.get());
  const zx_koid_t process_koid = fsl::GetKoid(process.get());

  zx::thread thread;
  ASSERT_EQ(exception.exception.get_thread(&thread), ZX_OK);
  const std::string thread_name = fsl::GetObjectName(thread.get());
  const zx_koid_t thread_koid = fsl::GetKoid(thread.get());

  const std::string kComponentUrl = "driver_host_url";
  const std::string kComponentMoniker = "/bootstrap/driver-hosts:x";
  introspect().AddThreadKoidToComponentInfo(thread_koid, StubCrashIntrospect::ComponentInfo{
                                                             .url = kComponentUrl,
                                                             .moniker = kComponentMoniker,
                                                         });

  bool called = false;
  HandleException(std::move(exception.exception), zx::duration::infinite(),
                  [&called](const ::fidl::StringPtr& moniker) {
                    EXPECT_EQ(kDriverMoniker, moniker.value());
                    called = true;
                  });

  ASSERT_TRUE(called);

  ASSERT_EQ(crash_reporter().reports().size(), 1u);
  const ::fuchsia::feedback::CrashReport& report = crash_reporter().reports().front();

  ValidateCrashReport(report, kDriverUrl, process_name, process_koid, thread_name, thread_koid,
                      {
                          {kCrashProcessStateKey, "in exception"},
                      });

  // We kill the jobs. This kills the underlying process. We do this so that the crashed process
  // doesn't get rescheduled. Otherwise the exception on the crash program would bubble out of our
  // environment and create noise on the overall system.
  exception.job.kill();
}

TEST_F(HandlerTest, DriverHostCrashFallback) {
  SetUpCrashReporter();
  SetUpCrashIntrospect();

  // Create the exception.
  ExceptionContext exception;
  ASSERT_TRUE(RetrieveExceptionContext(&exception));

  zx::process process;
  ASSERT_EQ(exception.exception.get_process(&process), ZX_OK);
  const std::string process_name = fsl::GetObjectName(process.get());
  const zx_koid_t process_koid = fsl::GetKoid(process.get());

  zx::thread thread;
  ASSERT_EQ(exception.exception.get_thread(&thread), ZX_OK);
  const std::string thread_name = fsl::GetObjectName(thread.get());
  const zx_koid_t thread_koid = fsl::GetKoid(thread.get());

  const std::string kComponentUrl = "driver_host_url";
  const std::string kComponentMoniker = "/bootstrap/driver-hosts:x";
  introspect().AddThreadKoidToComponentInfo(thread_koid, StubCrashIntrospect::ComponentInfo{
                                                             .url = kComponentUrl,
                                                             .moniker = kComponentMoniker,
                                                         });

  // The driver introspect returning failure will cause us to fallback to component attribution.
  SetupFailingDriverIntrospect();

  bool called = false;
  HandleException(std::move(exception.exception), zx::duration::infinite(),
                  [&called, kComponentMoniker](const ::fidl::StringPtr& moniker) {
                    EXPECT_EQ(kComponentMoniker.substr(1), moniker.value());
                    called = true;
                  });

  ASSERT_TRUE(called);

  ASSERT_EQ(crash_reporter().reports().size(), 1u);
  const ::fuchsia::feedback::CrashReport& report = crash_reporter().reports().front();

  ValidateCrashReport(report, kComponentUrl, process_name, process_koid, thread_name, thread_koid,
                      {
                          {kCrashProcessStateKey, "in exception"},
                      });

  // We kill the jobs. This kills the underlying process. We do this so that the crashed process
  // doesn't get rescheduled. Otherwise the exception on the crash program would bubble out of our
  // environment and create noise on the overall system.
  exception.job.kill();
}

TEST_F(HandlerTest, DriverHostCrashNoDriverIntrospect) {
  SetUpCrashReporter();
  SetUpCrashIntrospect();

  // Create the exception.
  ExceptionContext exception;
  ASSERT_TRUE(RetrieveExceptionContext(&exception));

  zx::process process;
  ASSERT_EQ(exception.exception.get_process(&process), ZX_OK);
  const std::string process_name = fsl::GetObjectName(process.get());
  const zx_koid_t process_koid = fsl::GetKoid(process.get());

  zx::thread thread;
  ASSERT_EQ(exception.exception.get_thread(&thread), ZX_OK);
  const std::string thread_name = fsl::GetObjectName(thread.get());
  const zx_koid_t thread_koid = fsl::GetKoid(thread.get());

  const std::string kComponentUrl = "driver_host_url";
  const std::string kComponentMoniker = "/bootstrap/driver-hosts:x";
  introspect().AddThreadKoidToComponentInfo(thread_koid, StubCrashIntrospect::ComponentInfo{
                                                             .url = kComponentUrl,
                                                             .moniker = kComponentMoniker,
                                                         });

  bool called = false;
  // Setup without a valid driver introspect channel will cause fallback to component attribution.
  HandleException(
      std::move(exception.exception), zx::duration::infinite(),
      [&called, kComponentMoniker](const ::fidl::StringPtr& moniker) {
        EXPECT_EQ(kComponentMoniker.substr(1), moniker.value());
        called = true;
      },
      /*driver_introspect=*/false);

  ASSERT_TRUE(called);

  ASSERT_EQ(crash_reporter().reports().size(), 1u);
  const ::fuchsia::feedback::CrashReport& report = crash_reporter().reports().front();

  ValidateCrashReport(report, kComponentUrl, process_name, process_koid, thread_name, thread_koid,
                      {
                          {kCrashProcessStateKey, "in exception"},
                      });

  // We kill the jobs. This kills the underlying process. We do this so that the crashed process
  // doesn't get rescheduled. Otherwise the exception on the crash program would bubble out of our
  // environment and create noise on the overall system.
  exception.job.kill();
}

using HandlerTestWithPF = UnitTestFixture;

TEST_F(HandlerTestWithPF, HoldsWakeLeaseUntilFeedbackResponse) {
  StubCrashReporterDelaysResponse crash_reporter;
  InjectServiceProvider(&crash_reporter);

  StubCrashIntrospect introspect;
  InjectServiceProvider(&introspect);

  DriverCrashIntrospect driver_introspect;

  // Create the exception.
  ExceptionContext exception_context;
  ASSERT_TRUE(RetrieveExceptionContext(&exception_context));

  // We kill the jobs. This kills the underlying process. We do this so that the crashed process
  // doesn't get rescheduled. Otherwise the exception on the crash program would bubble out of our
  // environment and create noise on the overall system.
  exception_context.job.kill();

  // Must explicitly wait for the job to be terminated to ensure consistency between asan and
  // non-asan.
  exception_context.job.wait_one(ZX_TASK_TERMINATED, zx::time_monotonic::infinite(), nullptr);

  auto wake_lease = std::make_unique<stubs::WakeLease>(dispatcher());
  stubs::WakeLease* wake_lease_ptr = wake_lease.get();
  CrashReporter handler(dispatcher(), services(), zx::duration::infinite(),
                        /*wake_lease=*/std::move(wake_lease),
                        driver_introspect.GetClient(dispatcher()));

  zx::process process;
  exception_context.exception.get_process(&process);

  zx::thread thread;
  exception_context.exception.get_thread(&thread);

  bool called = false;
  handler.Send(std::move(exception_context.exception), std::move(process), std::move(thread),
               [&called](const ::fidl::StringPtr& moniker) { called = true; });
  RunLoopUntilIdle();

  EXPECT_FALSE(called);
  EXPECT_TRUE(wake_lease_ptr->LeaseHeld());

  crash_reporter.PopResponse();
  RunLoopUntilIdle();
  EXPECT_TRUE(called);
  EXPECT_FALSE(wake_lease_ptr->LeaseHeld());
}

}  // namespace
}  // namespace handler
}  // namespace exceptions
}  // namespace forensics
