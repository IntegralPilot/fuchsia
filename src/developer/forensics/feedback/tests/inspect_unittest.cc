// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/inspect.h"

#include <fuchsia/mem/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/fpromise/result.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include <memory>
#include <string>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/testing/backoff.h"
#include "src/developer/forensics/testing/gmatchers.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/stubs/diagnostics_archive.h"
#include "src/developer/forensics/testing/stubs/diagnostics_batch_iterator.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/timekeeper/async_test_clock.h"

namespace forensics::feedback {
namespace {

class InspectTest : public UnitTestFixture {
 public:
  InspectTest()
      : executor_(dispatcher()),
        clock_(dispatcher()),
        cobalt_(dispatcher(), services(), &clock_),
        inspect_node_manager_(&InspectRoot()),
        inspect_data_budget_(std::make_unique<feedback_data::InspectDataBudget>(
            true, &inspect_node_manager_, &cobalt_)),
        redactor_(std::make_unique<IdentityRedactor>(inspect::BoolProperty())) {}

 protected:
  void SetUpInspectServer(std::unique_ptr<stubs::DiagnosticsArchiveBase> server) {
    inspect_server_ = std::move(server);
    if (inspect_server_) {
      InjectServiceProvider(inspect_server_.get(), feedback_data::kArchiveAccessorName);
    }
  }

  void DisableDataBudget() {
    inspect_data_budget_ =
        std::make_unique<feedback_data::InspectDataBudget>(false, &inspect_node_manager_, &cobalt_);
  }

  AttachmentData Run(::fpromise::promise<AttachmentData> promise,
                     const std::optional<zx::duration> run_loop_for = std::nullopt) {
    AttachmentData attachment(Error::kNotSet);
    executor_.schedule_task(
        promise.and_then([&attachment](AttachmentData& result) { attachment = std::move(result); })
            .or_else([]() { FX_LOGS(FATAL) << "Unexpected branch"; }));

    if (run_loop_for.has_value()) {
      RunLoopFor(*run_loop_for);
    } else {
      RunLoopUntilIdle();
    }

    return attachment;
  }

  RedactorBase* GetRedactor() const { return redactor_.get(); }
  void SetRedactor(std::unique_ptr<RedactorBase> redactor) { redactor_ = std::move(redactor); }

  feedback_data::InspectDataBudget* DataBudget() { return inspect_data_budget_.get(); }

 private:
  async::Executor executor_;
  timekeeper::AsyncTestClock clock_;
  cobalt::Logger cobalt_;
  InspectNodeManager inspect_node_manager_;
  std::unique_ptr<feedback_data::InspectDataBudget> inspect_data_budget_;
  std::unique_ptr<stubs::DiagnosticsArchiveBase> inspect_server_;
  std::unique_ptr<RedactorBase> redactor_;
};

TEST_F(InspectTest, DataBudget) {
  const uint64_t kTicket = 1234;
  fuchsia_diagnostics::StreamParameters parameters;
  SetUpInspectServer(std::make_unique<stubs::DiagnosticsArchiveCaptureParameters>(&parameters));

  const size_t kBudget = DataBudget()->SizeInBytes().value();
  Inspect inspect(dispatcher(), services(), std::make_unique<MonotonicBackoff>(), DataBudget(),
                  GetRedactor());

  inspect.Get(kTicket);
  RunLoopUntilIdle();

  ASSERT_TRUE(parameters.performance_configuration().has_value());
  const ::fuchsia_diagnostics::PerformanceConfiguration& performance =
      *parameters.performance_configuration();
  ASSERT_TRUE(performance.max_aggregate_content_size_bytes().has_value());
  ASSERT_EQ(*performance.max_aggregate_content_size_bytes(), kBudget);
}

TEST_F(InspectTest, NoDataBudget) {
  const uint64_t kTicket = 1234;
  fuchsia_diagnostics::StreamParameters parameters;
  SetUpInspectServer(std::make_unique<stubs::DiagnosticsArchiveCaptureParameters>(&parameters));

  DisableDataBudget();
  Inspect inspect(dispatcher(), services(), std::make_unique<MonotonicBackoff>(), DataBudget(),
                  GetRedactor());

  inspect.Get(kTicket);
  RunLoopUntilIdle();

  EXPECT_FALSE(parameters.performance_configuration().has_value());
}

TEST_F(InspectTest, Get) {
  SetUpInspectServer(std::make_unique<stubs::DiagnosticsArchive>(
      dispatcher(),
      std::make_unique<stubs::DiagnosticsBatchIterator>(std::vector<std::vector<std::string>>({
          {"foo1", "foo2"},
          {"bar1"},
          {},
      }))));

  Inspect inspect(dispatcher(), services(), std::make_unique<MonotonicBackoff>(), DataBudget(),
                  GetRedactor());
  const AttachmentData attachment = Run(inspect.Get(1234));

  EXPECT_THAT(attachment, AttachmentDataIs(R"([
foo1,
foo2,
bar1
])"));
}

TEST_F(InspectTest, GetTerminatesDueToForceCompletion) {
  const uint64_t kTicket = 1234;
  SetUpInspectServer(std::make_unique<stubs::DiagnosticsArchive>(
      dispatcher(), std::make_unique<stubs::DiagnosticsBatchIteratorNeverRespondsAfterOneBatch>(
                        std::vector<std::string>({"foo1", "foo2"}))));

  Inspect inspect(dispatcher(), services(), std::make_unique<MonotonicBackoff>(), DataBudget(),
                  GetRedactor());
  const AttachmentData attachment = Run(inspect.Get(kTicket));

  // Giving some time to actually collect some inspect data
  RunLoopUntilIdle();

  // Forcefully terminate inspect collection
  inspect.ForceCompletion(kTicket, Error::kDefault);

  RunLoopUntilIdle();

  EXPECT_THAT(attachment, AttachmentDataIs(R"([
foo1,
foo2
])",
                                           Error::kDefault));
}

TEST_F(InspectTest, ForceCompletionCalledAfterTermination) {
  const uint64_t kTicket = 1234;
  SetUpInspectServer(std::make_unique<stubs::DiagnosticsArchive>(
      dispatcher(),
      std::make_unique<stubs::DiagnosticsBatchIterator>(std::vector<std::vector<std::string>>({
          {"foo1", "foo2"},
          {"bar1"},
          {},
      }))));

  Inspect inspect(dispatcher(), services(), std::make_unique<MonotonicBackoff>(), DataBudget(),
                  GetRedactor());
  const AttachmentData attachment = Run(inspect.Get(kTicket));

  inspect.ForceCompletion(kTicket, Error::kDefault);

  EXPECT_THAT(attachment, AttachmentDataIs(R"([
foo1,
foo2,
bar1
])"));
}

TEST_F(InspectTest, GetCalledWithSameTicket) {
  const uint64_t kTicket = 1234;
  Inspect inspect(dispatcher(), services(), std::make_unique<MonotonicBackoff>(), DataBudget(),
                  GetRedactor());

  // Expect a crash because a ticket cannot be reused.
  ASSERT_DEATH(
      {
        const ::fpromise::promise<AttachmentData> attachment1 = inspect.Get(kTicket);
        const ::fpromise::promise<AttachmentData> attachment2 = inspect.Get(kTicket);
      },
      "Ticket used twice: ");
}

TEST_F(InspectTest, GetConnectionError) {
  const uint64_t kTicket = 1234;
  SetUpInspectServer(std::make_unique<stubs::DiagnosticsArchiveClosesIteratorConnection>());

  Inspect inspect(dispatcher(), services(), std::make_unique<MonotonicBackoff>(), DataBudget(),
                  GetRedactor());
  const AttachmentData attachment = Run(inspect.Get(kTicket));

  EXPECT_THAT(attachment, AttachmentDataIs(Error::kConnectionError));
}

TEST_F(InspectTest, GetIteratorReturnsError) {
  const uint64_t kTicket = 1234;
  SetUpInspectServer(std::make_unique<stubs::DiagnosticsArchive>(
      dispatcher(), std::make_unique<stubs::DiagnosticsBatchIteratorReturnsError>()));

  Inspect inspect(dispatcher(), services(), std::make_unique<MonotonicBackoff>(), DataBudget(),
                  GetRedactor());
  const AttachmentData attachment = Run(inspect.Get(kTicket));

  EXPECT_THAT(attachment, AttachmentDataIs(Error::kMissingValue));
}

TEST_F(InspectTest, Reconnects) {
  fuchsia_diagnostics::StreamParameters parameters;
  auto archive = std::make_unique<stubs::DiagnosticsArchiveCaptureParameters>(&parameters);

  InjectServiceProvider(archive.get(), feedback_data::kArchiveAccessorName);

  Inspect inspect(dispatcher(), services(), std::make_unique<MonotonicBackoff>(), DataBudget(),
                  GetRedactor());
  RunLoopUntilIdle();

  EXPECT_TRUE(archive->IsBound());

  archive->CloseConnection(ZX_ERR_PEER_CLOSED);
  RunLoopUntilIdle();

  EXPECT_FALSE(archive->IsBound());

  RunLoopFor(zx::sec(1));
  EXPECT_TRUE(archive->IsBound());
}

TEST_F(InspectTest, RedactsWithJsonReplacers) {
  std::string json(
      "[\"1.2.3.4\",\n"  // IPv4 Addresses are redacted
      "\"5.6.7.8\",\n"
      "\"2001::1\",\n"  // IPv6 Addresses are redacted
      "\"2001::2\",\n"
      "\"AA-BB-CC-DD-EE-FF\",\n"  // MAC Addresses are redacted with manufacturer component
                                  // unredacted
      "\"11:22:33:44:55:66\",\n"
      "1234567890abcdefABCDEF0123456789,\n"  // Long Hex numbers are not redacted
      "\"106986199446298680449\"]");         // Obfuscated Gaia IDs are not redacted
  SetUpInspectServer(std::make_unique<stubs::DiagnosticsArchive>(
      dispatcher(),
      std::make_unique<stubs::DiagnosticsBatchIterator>(std::vector<std::vector<std::string>>({
          {json},
          {},
      }))));
  inspect::BoolProperty redaction_enabled;
  redaction_enabled.Set(true);
  SetRedactor(std::make_unique<Redactor>(0, inspect::UintProperty(), std::move(redaction_enabled)));

  Inspect inspect(dispatcher(), services(), std::make_unique<MonotonicBackoff>(), DataBudget(),
                  GetRedactor());
  const AttachmentData attachment = Run(inspect.Get(1234));
  EXPECT_EQ(attachment.Value(), R"([
["<REDACTED-IPV4: 1>",
"<REDACTED-IPV4: 2>",
"<REDACTED-IPV6: 3>",
"<REDACTED-IPV6: 4>",
"AA-BB-CC-<REDACTED-MAC: 5>",
"11:22:33:<REDACTED-MAC: 6>",
1234567890abcdefABCDEF0123456789,
"106986199446298680449"]
])");
}
}  // namespace
}  // namespace forensics::feedback
