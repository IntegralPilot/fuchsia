// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/previous_boot_inspect.h"

#include <fcntl.h>
#include <lib/async/cpp/executor.h>
#include <lib/fdio/fd.h>
#include <lib/fpromise/result.h>
#include <lib/inspect/cpp/vmo/types.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/testing/backoff.h"
#include "src/developer/forensics/testing/gmatchers.h"
#include "src/developer/forensics/testing/stubs/previous_boot_data_provider.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/files/scoped_temp_dir.h"

namespace forensics::feedback {
namespace {

class PreviousBootDataProviderClosesConnectionThenReturnsData
    : public stubs::PreviousBootDataProviderBase {
 public:
  explicit PreviousBootDataProviderClosesConnectionThenReturnsData(
      fuchsia::diagnostics::persistence::PreviousBootData data)
      : data_(std::move(data)) {}

  void WatchPreviousBootData(
      fuchsia::diagnostics::persistence::PreviousBootDataProviderOptions options,
      WatchPreviousBootDataCallback callback) override {
    if (!has_closed_) {
      has_closed_ = true;
      CloseConnection();
      return;
    }
    fuchsia::diagnostics::persistence::PreviousBootDataProvider_WatchPreviousBootData_Response
        response;
    response.data = std::move(data_);
    callback(
        fuchsia::diagnostics::persistence::PreviousBootDataProvider_WatchPreviousBootData_Result::
            WithResponse(std::move(response)));
  }

 private:
  bool has_closed_ = false;
  fuchsia::diagnostics::persistence::PreviousBootData data_;
};

class PreviousBootInspectTest : public UnitTestFixture {
 public:
  PreviousBootInspectTest()
      : executor_(dispatcher()),
        redactor_(std::make_unique<IdentityRedactor>(inspect::BoolProperty())) {}

 protected:
  void SetUpDataProviderServer(std::unique_ptr<stubs::PreviousBootDataProviderBase> server) {
    server_ = std::move(server);
    if (server_) {
      InjectServiceProvider(server_.get());
    }
  }

  fidl::InterfaceHandle<fuchsia::io::File> CreateFileHandle(const std::string& content) {
    std::string path;
    dir_.NewTempFileWithData(content, &path);
    int fd = open(path.c_str(), O_RDONLY);
    FX_CHECK(fd >= 0);
    zx_handle_t handle = ZX_HANDLE_INVALID;
    FX_CHECK(fdio_fd_transfer(fd, &handle) == ZX_OK);
    return fidl::InterfaceHandle<fuchsia::io::File>(zx::channel(handle));
  }

  AttachmentData Run(::fpromise::promise<AttachmentData> promise) {
    AttachmentData attachment(Error::kNotSet);
    executor_.schedule_task(
        promise.and_then([&attachment](AttachmentData& result) { attachment = std::move(result); })
            .or_else([]() { FX_LOGS(FATAL) << "Unexpected branch"; }));

    RunLoopUntilIdle();
    return attachment;
  }

  RedactorBase* GetRedactor() const { return redactor_.get(); }
  void SetRedactor(std::unique_ptr<RedactorBase> redactor) { redactor_ = std::move(redactor); }

  async::Executor& GetExecutor() { return executor_; }

  std::string InspectPath() { return files::JoinPath(dir_.path(), "inspect.previous_boot.json"); }

  std::string NewDirectory() {
    std::string path;
    dir_.NewTempDir(&path);
    return path;
  }

 private:
  async::Executor executor_;
  std::unique_ptr<stubs::PreviousBootDataProviderBase> server_;
  std::unique_ptr<RedactorBase> redactor_;
  files::ScopedTempDir dir_;
};

TEST_F(PreviousBootInspectTest, SucceedsReturnsInspectData) {
  const std::string kInspectData = "[{\"root\": {\"value\": 123}}]";
  fuchsia::diagnostics::persistence::PreviousBootData data;
  data.set_inspect(CreateFileHandle(kInspectData));
  SetUpDataProviderServer(
      std::make_unique<stubs::PreviousBootDataProviderReturnsData>(std::move(data)));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());
  AttachmentData result = Run(provider.Get(1));

  EXPECT_THAT(result, AttachmentDataIs(kInspectData));
}

TEST_F(PreviousBootInspectTest, CachesInspectDataOnDisk) {
  const std::string kInspectData = "[{\"root\": {\"value\": 123}}]";
  fuchsia::diagnostics::persistence::PreviousBootData data;
  data.set_inspect(CreateFileHandle(kInspectData));
  SetUpDataProviderServer(
      std::make_unique<stubs::PreviousBootDataProviderReturnsData>(std::move(data)));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());
  Run(provider.Get(1));

  EXPECT_TRUE(files::IsFile(InspectPath()));
  std::string on_disk;
  EXPECT_TRUE(files::ReadFileToString(InspectPath(), &on_disk));
  EXPECT_EQ(on_disk, kInspectData);
}

TEST_F(PreviousBootInspectTest, FailsMissingInspect) {
  fuchsia::diagnostics::persistence::PreviousBootData data;
  SetUpDataProviderServer(
      std::make_unique<stubs::PreviousBootDataProviderReturnsData>(std::move(data)));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());
  AttachmentData result = Run(provider.Get(1));

  EXPECT_THAT(result, AttachmentDataIs(Error::kMissingValue));
  EXPECT_FALSE(files::IsFile(InspectPath()));
}

TEST_F(PreviousBootInspectTest, ReconnectsOnConnectionClosed) {
  auto server = std::make_unique<stubs::PreviousBootDataProviderNeverReturns>();
  auto* server_ptr = server.get();
  SetUpDataProviderServer(std::move(server));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());
  RunLoopUntilIdle();
  EXPECT_TRUE(server_ptr->IsBound());

  server_ptr->CloseConnection(ZX_ERR_PEER_CLOSED);
  RunLoopUntilIdle();
  EXPECT_FALSE(server_ptr->IsBound());

  RunLoopFor(zx::sec(1));
  EXPECT_TRUE(server_ptr->IsBound());
}

TEST_F(PreviousBootInspectTest, ReconnectsReturnsDataAfterRetry) {
  const std::string kInspectData = "[{\"root\": {\"value\": 123}}]";
  fuchsia::diagnostics::persistence::PreviousBootData data;
  data.set_inspect(CreateFileHandle(kInspectData));
  SetUpDataProviderServer(
      std::make_unique<PreviousBootDataProviderClosesConnectionThenReturnsData>(std::move(data)));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());

  AttachmentData result(Error::kNotSet);
  GetExecutor().schedule_task(
      provider.Get(1)
          .and_then([&result](AttachmentData& val) { result = std::move(val); })
          .or_else([]() { FX_LOGS(FATAL) << "Unexpected branch"; }));

  RunLoopUntilIdle();
  EXPECT_FALSE(result.HasValue());

  RunLoopFor(zx::sec(1));
  EXPECT_THAT(result, AttachmentDataIs(kInspectData));
  EXPECT_TRUE(files::IsFile(InspectPath()));
}

TEST_F(PreviousBootInspectTest, CachedResultMultipleGets) {
  const std::string kInspectData = "[{\"root\": {\"value\": 456}}]";
  fuchsia::diagnostics::persistence::PreviousBootData data;
  data.set_inspect(CreateFileHandle(kInspectData));
  SetUpDataProviderServer(
      std::make_unique<stubs::PreviousBootDataProviderReturnsData>(std::move(data)));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());
  AttachmentData result1 = Run(provider.Get(1));
  EXPECT_THAT(result1, AttachmentDataIs(kInspectData));

  // Second call should return cached value from disk immediately without needing the server.
  AttachmentData result2 = Run(provider.Get(2));
  EXPECT_THAT(result2, AttachmentDataIs(kInspectData));
}

TEST_F(PreviousBootInspectTest, ForceCompletionTimeout) {
  SetUpDataProviderServer(std::make_unique<stubs::PreviousBootDataProviderNeverReturns>());

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());
  AttachmentData result(Error::kNotSet);
  GetExecutor().schedule_task(
      provider.Get(1)
          .and_then([&result](AttachmentData& val) { result = std::move(val); })
          .or_else([]() { FX_LOGS(FATAL) << "Unexpected branch"; }));

  RunLoopUntilIdle();
  EXPECT_FALSE(result.HasValue());
  EXPECT_EQ(result.Error(), Error::kNotSet);

  provider.ForceCompletion(1, Error::kTimeout);
  RunLoopUntilIdle();

  EXPECT_THAT(result, AttachmentDataIs(Error::kTimeout));
}

TEST_F(PreviousBootInspectTest, RedactsWithJsonReplacers) {
  const std::string kInspectData =
      "[\"1.2.3.4\",\n"  // IPv4 Addresses are redacted
      "\"5.6.7.8\",\n"
      "\"2001::1\",\n"  // IPv6 Addresses are redacted
      "\"2001::2\",\n"
      "\"AA-BB-CC-DD-EE-FF\",\n"  // MAC Addresses are redacted with manufacturer component
                                  // unredacted
      "\"11:22:33:44:55:66\",\n"
      "1234567890abcdefABCDEF0123456789,\n"  // Long Hex numbers are not redacted
      "\"106986199446298680449\"]";          // Obfuscated Gaia IDs are not redacted
  fuchsia::diagnostics::persistence::PreviousBootData data;
  data.set_inspect(CreateFileHandle(kInspectData));
  SetUpDataProviderServer(
      std::make_unique<stubs::PreviousBootDataProviderReturnsData>(std::move(data)));

  inspect::BoolProperty redaction_enabled;
  redaction_enabled.Set(true);
  SetRedactor(std::make_unique<Redactor>(0, inspect::UintProperty(), std::move(redaction_enabled)));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());
  AttachmentData result = Run(provider.Get(1));

  EXPECT_THAT(result, AttachmentDataIs(R"(["<REDACTED-IPV4: 1>",
"<REDACTED-IPV4: 2>",
"<REDACTED-IPV6: 3>",
"<REDACTED-IPV6: 4>",
"AA-BB-CC-<REDACTED-MAC: 5>",
"11:22:33:<REDACTED-MAC: 6>",
1234567890abcdefABCDEF0123456789,
"106986199446298680449"])"));
}

TEST_F(PreviousBootInspectTest, CachesRedactedInspectDataOnDisk) {
  const std::string kInspectData =
      "[\"1.2.3.4\",\n"  // IPv4 Addresses are redacted
      "\"5.6.7.8\",\n"
      "\"2001::1\",\n"  // IPv6 Addresses are redacted
      "\"2001::2\",\n"
      "\"AA-BB-CC-DD-EE-FF\",\n"  // MAC Addresses are redacted with manufacturer component
                                  // unredacted
      "\"11:22:33:44:55:66\",\n"
      "1234567890abcdefABCDEF0123456789,\n"  // Long Hex numbers are not redacted
      "\"106986199446298680449\"]";          // Obfuscated Gaia IDs are not redacted
  fuchsia::diagnostics::persistence::PreviousBootData data;
  data.set_inspect(CreateFileHandle(kInspectData));
  SetUpDataProviderServer(
      std::make_unique<stubs::PreviousBootDataProviderReturnsData>(std::move(data)));

  inspect::BoolProperty redaction_enabled;
  redaction_enabled.Set(true);
  SetRedactor(std::make_unique<Redactor>(0, inspect::UintProperty(), std::move(redaction_enabled)));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());
  Run(provider.Get(1));

  const std::string kExpected =
      R"(["<REDACTED-IPV4: 1>",
"<REDACTED-IPV4: 2>",
"<REDACTED-IPV6: 3>",
"<REDACTED-IPV6: 4>",
"AA-BB-CC-<REDACTED-MAC: 5>",
"11:22:33:<REDACTED-MAC: 6>",
1234567890abcdefABCDEF0123456789,
"106986199446298680449"])";

  EXPECT_TRUE(files::IsFile(InspectPath()));
  std::string on_disk;
  EXPECT_TRUE(files::ReadFileToString(InspectPath(), &on_disk));
  EXPECT_EQ(on_disk, kExpected);
}

TEST_F(PreviousBootInspectTest, FileAlreadyExistsDoesNotFetchFromPersistence) {
  const std::string kInspectData = "[{\"root\": {\"value\": 789}}]";
  ASSERT_TRUE(files::WriteFile(InspectPath(), kInspectData));

  // Data provider that would never return if connected.
  auto server = std::make_unique<stubs::PreviousBootDataProviderNeverReturns>();
  auto* server_ptr = server.get();
  SetUpDataProviderServer(std::move(server));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());
  AttachmentData result = Run(provider.Get(1));

  EXPECT_THAT(result, AttachmentDataIs(kInspectData));
  EXPECT_FALSE(server_ptr->IsBound());
}

TEST_F(PreviousBootInspectTest, FileWriteFailure) {
  const std::string kInspectData = "[{\"root\": {\"value\": 123}}]";
  fuchsia::diagnostics::persistence::PreviousBootData data;
  data.set_inspect(CreateFileHandle(kInspectData));
  SetUpDataProviderServer(
      std::make_unique<stubs::PreviousBootDataProviderReturnsData>(std::move(data)));

  // Use an invalid directory path where file creation will fail.
  const std::string invalid_path = "/dev/null/cannot/write/here.json";

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), invalid_path);
  AttachmentData result = Run(provider.Get(1));

  EXPECT_THAT(result, AttachmentDataIs(Error::kFileWriteFailure));
}

TEST_F(PreviousBootInspectTest, FileDeletedAfterBeingCached) {
  const std::string kInspectData = "[{\"root\": {\"value\": 123}}]";
  fuchsia::diagnostics::persistence::PreviousBootData data;
  data.set_inspect(CreateFileHandle(kInspectData));
  SetUpDataProviderServer(
      std::make_unique<stubs::PreviousBootDataProviderReturnsData>(std::move(data)));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), InspectPath());
  AttachmentData result = Run(provider.Get(1));

  EXPECT_THAT(result, AttachmentDataIs(kInspectData));
  EXPECT_TRUE(files::IsFile(InspectPath()));

  ASSERT_TRUE(files::DeletePath(InspectPath(), false));
  EXPECT_FALSE(files::IsFile(InspectPath()));

  AttachmentData result2(Error::kNotSet);
  GetExecutor().schedule_task(
      provider.Get(2)
          .and_then([&result2](AttachmentData& val) { result2 = std::move(val); })
          .or_else([]() { FX_LOGS(FATAL) << "Unexpected branch"; }));

  RunLoopUntilIdle();
  EXPECT_FALSE(result2.HasValue());
  EXPECT_EQ(result2.Error(), Error::kNotSet);

  provider.ForceCompletion(2, Error::kTimeout);
  RunLoopUntilIdle();

  EXPECT_THAT(result2, AttachmentDataIs(Error::kTimeout));
}

TEST_F(PreviousBootInspectTest, ProvidedPathIsDirectory) {
  const std::string kInspectData = "[{\"root\": {\"value\": 123}}]";
  fuchsia::diagnostics::persistence::PreviousBootData data;
  data.set_inspect(CreateFileHandle(kInspectData));
  SetUpDataProviderServer(
      std::make_unique<stubs::PreviousBootDataProviderReturnsData>(std::move(data)));

  PreviousBootInspect provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                               GetRedactor(), NewDirectory());
  AttachmentData result = Run(provider.Get(1));

  EXPECT_THAT(result, AttachmentDataIs(Error::kFileWriteFailure));
}

}  // namespace
}  // namespace forensics::feedback
