// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/file_backed_provider.h"

#include <lib/async/cpp/executor.h>

#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/testing/gmatchers.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/lib/files/file.h"
#include "src/lib/files/scoped_temp_dir.h"

namespace forensics::feedback {

class FileBackedProviderTest : public UnitTestFixture {
 public:
  FileBackedProviderTest() : executor_(dispatcher()) {}

 protected:
  async::Executor& GetExecutor() { return executor_; }

  std::string NewFile() {
    std::string path;
    dir_.NewTempFile(&path);
    return path;
  }

  std::string NewFile(const std::string& data) {
    std::string path;
    dir_.NewTempFileWithData(data, &path);
    return path;
  }

 private:
  async::Executor executor_;
  files::ScopedTempDir dir_;
};

TEST_F(FileBackedProviderTest, MalformedFilePath) {
  const uint64_t kTicket = 21;
  const std::string kBadPath = "/bad/path";

  FileBackedProvider file_backed_provider_(kBadPath);

  AttachmentData attachment(Error::kNotSet);
  GetExecutor().schedule_task(
      file_backed_provider_.Get(kTicket)
          .and_then([&attachment](AttachmentData& res) { attachment = std::move(res); })
          .or_else([] { FX_LOGS(FATAL) << "Logic error"; }));

  RunLoopUntilIdle();

  EXPECT_THAT(attachment, AttachmentDataIs(Error::kFileReadFailure));
}

TEST_F(FileBackedProviderTest, EmptyFile) {
  const std::string path = NewFile();
  const uint64_t kTicket = 21;

  FileBackedProvider file_backed_provider_(path);

  AttachmentData attachment(Error::kNotSet);
  GetExecutor().schedule_task(
      file_backed_provider_.Get(kTicket)
          .and_then([&attachment](AttachmentData& res) { attachment = std::move(res); })
          .or_else([] { FX_LOGS(FATAL) << "Logic error"; }));

  RunLoopUntilIdle();

  EXPECT_THAT(attachment, AttachmentDataIs(Error::kMissingValue));
}

TEST_F(FileBackedProviderTest, NonEmptyFile) {
  const std::string data = "content";
  const std::string path = NewFile(data);
  const uint64_t kTicket = 21;

  FileBackedProvider file_backed_provider_(path);

  AttachmentData attachment(Error::kNotSet);
  GetExecutor().schedule_task(
      file_backed_provider_.Get(kTicket)
          .and_then([&attachment](AttachmentData& res) { attachment = std::move(res); })
          .or_else([] { FX_LOGS(FATAL) << "Logic error"; }));

  RunLoopUntilIdle();

  EXPECT_THAT(attachment, AttachmentDataIs(data));
}

TEST_F(FileBackedProviderTest, ForceCompletionCalledWhenPromiseIsIncomplete) {
  const std::string path = NewFile();
  const uint64_t kTicket = 21;

  FileBackedProvider file_backed_provider_(path);

  AttachmentData attachment(Error::kNotSet);
  GetExecutor().schedule_task(
      file_backed_provider_.Get(kTicket)
          .and_then([&attachment](AttachmentData& res) { attachment = std::move(res); })
          .or_else([] { FX_LOGS(FATAL) << "Logic error"; }));

  file_backed_provider_.ForceCompletion(kTicket, Error::kDefault);

  EXPECT_TRUE(files::IsFile(path));
}

}  // namespace forensics::feedback
