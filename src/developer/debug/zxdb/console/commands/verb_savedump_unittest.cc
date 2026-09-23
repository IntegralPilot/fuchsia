// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <filesystem>

#include <gtest/gtest.h>

#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/status.h"
#include "src/developer/debug/zxdb/client/mock_remote_api.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/console/console_test.h"
#include "src/lib/files/file.h"
#include "src/lib/files/scoped_temp_dir.h"

namespace zxdb {

namespace {

std::vector<uint8_t> GetCannedFileData() {
  const std::vector<uint8_t> kData = {0, 1, 2, 3, 4};
  return kData;
}

class SavedumpRemoteAPI : public MockRemoteAPI {
  void SaveMinidump(const debug_ipc::SaveMinidumpRequest& request,
                    fit::callback<void(const Err&, debug_ipc::SaveMinidumpReply)> cb) override {
    debug::MessageLoop::Current()->PostTask(FROM_HERE, [cb = std::move(cb)]() mutable {
      debug_ipc::SaveMinidumpReply reply;
      reply.status = debug::Status();
      // The core data is not actually validated, it's just written directly to the file.
      reply.core_data = GetCannedFileData();
      cb(Err(), reply);
    });
  }
};

class VerbSavedumpTest : public ConsoleTest {
 public:
  std::filesystem::path GetRootPath() { return temp_dir_.path(); }
  SavedumpRemoteAPI* mock_remote_api() { return remote_api_; }

  void SetUp() override {
    ConsoleTest::SetUp();

    debug_ipc::ThreadRecord thread_record;
    thread_record.id.process = process()->GetKoid();
    thread_record.id.thread = thread()->GetKoid();
    thread_record.state = debug_ipc::ThreadRecord::State::kSuspended;

    debug_ipc::PauseReply pause_reply;
    pause_reply.threads.push_back(thread_record);

    mock_remote_api()->set_pause_reply(pause_reply);

    process()->Pause([loop = &loop()]() { loop->QuitNow(); });
    loop().Run();
  }

 protected:
  std::unique_ptr<RemoteAPI> GetRemoteAPIImpl() override {
    auto remote_api = std::make_unique<SavedumpRemoteAPI>();
    remote_api_ = remote_api.get();
    return remote_api;
  }

 private:
  SavedumpRemoteAPI* remote_api_;
  files::ScopedTempDir temp_dir_;
};

TEST_F(VerbSavedumpTest, NormalizesPath) {
  const auto& relative_path = std::filesystem::path("tmp2") / ".." / "mydir" / ".." / "mini.dump";

  console().ProcessInputLine("savedump " + (GetRootPath() / relative_path).string());

  auto output_event = console().GetOutputEvent();
  EXPECT_EQ("Saving minidump...\n", output_event.output.AsString());

  loop().RunUntilNoTasks();

  output_event = console().GetOutputEvent();

  // The string should have been normalized to remove the ".."'s and other directories that are not
  // relevant in the above path.
  EXPECT_EQ("Minidump written to " + GetRootPath().string() + "/mini.dump",
            output_event.output.AsString());
  EXPECT_TRUE(std::filesystem::exists(GetRootPath() / "mini.dump"));

  std::vector<uint8_t> data;
  files::ReadFileToVector(GetRootPath() / "mini.dump", &data);

  EXPECT_TRUE(std::ranges::equal(data, GetCannedFileData()));
}

TEST_F(VerbSavedumpTest, CreateDirs) {
  const auto& relative_path = std::filesystem::path("dir1") / "dir2" / "dir3" / "mini.dump";

  console().ProcessInputLine("savedump " + (GetRootPath() / relative_path).string());

  auto output_event = console().GetOutputEvent();
  EXPECT_EQ("Saving minidump...\n", output_event.output.AsString());

  loop().RunUntilNoTasks();

  output_event = console().GetOutputEvent();

  // All directories should have been created as needed.
  EXPECT_EQ("Minidump written to " + GetRootPath().string() + "/" + relative_path.string(),
            output_event.output.AsString());
  EXPECT_TRUE(std::filesystem::exists(GetRootPath() / relative_path));

  std::vector<uint8_t> data;
  files::ReadFileToVector(GetRootPath() / relative_path, &data);

  EXPECT_TRUE(std::ranges::equal(data, GetCannedFileData()));
}

}  // namespace

}  // namespace zxdb
