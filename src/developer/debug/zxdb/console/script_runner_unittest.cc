// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/script_runner.h"

#include <memory>

#include <gtest/gtest.h>

#include "llvm/BinaryFormat/Dwarf.h"
#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/common/scoped_temp_file.h"
#include "src/developer/debug/zxdb/console/console_test.h"
#include "src/developer/debug/zxdb/symbols/code_block.h"
#include "src/developer/debug/zxdb/symbols/dwarf_expr.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/symbol_test_parent_setter.h"
#include "src/developer/debug/zxdb/symbols/type_test_support.h"
#include "src/developer/debug/zxdb/symbols/variable_test_support.h"
#include "src/lib/fxl/memory/ref_ptr.h"

namespace zxdb {

namespace {

class ScriptRunnerTest : public ConsoleTest {
 public:
  bool RunScript(const std::string& script_content) {
    ScopedTempFile temp;
    {
      std::ofstream out(temp.name());
      out << script_content;
    }

    runner_ = std::make_unique<ScriptRunner>(&session(), &console());
    runner_->set_timeout_s(1);
    bool script_success;
    runner_->Run(temp.name(), [&](bool success) {
      script_success = success;
      loop().QuitNow();
    });

    loop().Run();

    return script_success;
  }

  std::string GetFailureContext() const { return runner_->GetFailureContext().AsString(); }

 private:
  std::unique_ptr<ScriptRunner> runner_;
};

TEST_F(ScriptRunnerTest, Basic) {
  EXPECT_TRUE(
      RunScript("[zxdb] help\n"
                "Help!\n"))
      << GetFailureContext();
}

TEST_F(ScriptRunnerTest, Failure) {
  EXPECT_FALSE(
      RunScript("[zxdb] help\n"
                "something that won't match\n"))
      << GetFailureContext();
}

TEST_F(ScriptRunnerTest, CommentsAndEmptyLines) {
  EXPECT_TRUE(
      RunScript("\n"
                "# This is a comment\n"
                "[zxdb] help\n"
                "\n"
                "Help!\n"))
      << GetFailureContext();
}

TEST_F(ScriptRunnerTest, MultipleCommands) {
  EXPECT_TRUE(
      RunScript("[zxdb] help\n"
                "Help!\n"
                "[zxdb] help\n"
                "Help!\n"))
      << GetFailureContext();
}

TEST_F(ScriptRunnerTest, MultipleOutputs) {
  EXPECT_TRUE(
      RunScript("[zxdb] help\n"
                "Help!\n"
                "Type \"help <command>\" for command-specific help.\n"))
      << GetFailureContext();
}

TEST_F(ScriptRunnerTest, Wildcard) {
  EXPECT_TRUE(
      RunScript("[zxdb] help\n"
                "He??p!\n"))
      << GetFailureContext();
}

// This tests out of order output when the output comes as a single |OnOutput| event.
TEST_F(ScriptRunnerTest, OutOfOrder) {
  EXPECT_TRUE(
      RunScript("[zxdb] help\n"
                "## allow-out-of-order-output\n"
                "Type \"help <command>\" for command-specific help.\n"
                "Help!\n"))
      << GetFailureContext();

  // This should fail because ## allow-out-of-order-output is not specified.
  EXPECT_FALSE(
      RunScript("[zxdb] help\n"
                "Type \"help <command>\" for command-specific help.\n"
                "Help!\n"))
      << GetFailureContext();  // Stack frame info, still first output event.
}

// This test is very similar to the above. The difference is the output comes in two separate
// |OnOutput| events. The script runner accumulates all output across all events that are issued
// after dispatching a particular command, and then runs the matcher against the entire corpus of
// output, rather than against each output event individually.
TEST_F(ScriptRunnerTest, OutOfOrderMultipleEvents) {
  std::vector<std::unique_ptr<Frame>> frames;

  constexpr uint64_t kAddress0 = 0x1234;

  {
    const char kFileName[] = "file0.cc";
    FileLine file_line(kFileName, 1);
    auto source_file_provider = std::make_unique<MockSourceFileProvider>();
    source_file_provider->SetFileData(kFileName,
                                      SourceFileProvider::FileData("loop.Run()\n", kFileName, 0));
    Location loc(kAddress0, file_line, 0, SymbolContext::ForRelativeAddresses(), nullptr);
    auto frame = std::make_unique<MockFrame>(&session(), thread(), loc, 0x2000);
    frame->set_source_file_provider(std::move(source_file_provider));
    frames.push_back(std::move(frame));
  }

  InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames), true);

  // This is the actual order of output when selecting a frame, these come as two separate output
  // events in this order.
  EXPECT_TRUE(
      RunScript("[zxdb] frame 0\n"
                "file0.cc:1\n"  // Stack frame info, this is the first output event.
                "loop.Run()\n"))
      << GetFailureContext();  // Simulated file data, second output event.

  // This should fail because ## allow-out-of-order-output is not specified.
  EXPECT_FALSE(
      RunScript("[zxdb] frame 0\n"
                "loop.Run()\n"  // Simulated file data, still second output event.
                "file0.cc:1"))
      << GetFailureContext();  // Stack frame info, still first output event.

  // Now we want to be more relaxed with our output matching. This should also match the same output
  // but in a different order than it was produced.
  EXPECT_TRUE(
      RunScript("[zxdb] frame 0\n"
                "## allow-out-of-order-output\n"
                "loop.Run()\n"  // Simulated file data, still second output event.
                "file0.cc:1"))
      << GetFailureContext();  // Stack frame info, still first output event.
}

TEST_F(ScriptRunnerTest, FileNotFound) {
  ScriptRunner runner(&session(), &console());
  bool script_success = true;
  runner.Run("/non/existent/path", [&](bool success) {
    script_success = success;
    loop().QuitNow();
  });

  // No loop().Run() needed because it should fail synchronously.
  EXPECT_FALSE(script_success) << GetFailureContext();
}

TEST_F(ScriptRunnerTest, FrameInteraction) {
  std::vector<std::unique_ptr<Frame>> frames;
  frames.push_back(std::make_unique<MockFrame>(&session(), thread(), 0x12345678, 0x1000,
                                               "MyFunction", FileLine("my_file.cc", 42)));
  frames.push_back(std::make_unique<MockFrame>(&session(), thread(), 0x12345600, 0x1010,
                                               "CallerFunction", FileLine("caller.cc", 100)));

  InjectExceptionWithStack(ConsoleTest::kProcessKoid, ConsoleTest::kThreadKoid,
                           debug_ipc::ExceptionType::kSingleStep, std::move(frames), true);

  std::string script =
      "[zxdb] frame\n"
      "▶ 0 MyFunction() • my_file.cc:42\n"
      "  1 CallerFunction() • caller.cc:100\n";
  EXPECT_TRUE(RunScript(script)) << GetFailureContext();
}

TEST_F(ScriptRunnerTest, Backtrace) {
  uint64_t kAddress1 = 0x12345678;
  auto int32_type = MakeInt32Type();

  auto func = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  func->set_assigned_name("MyFunction");
  func->set_code_ranges(AddressRanges(AddressRange(kAddress1, kAddress1 + 0x100)));

  auto param = MakeVariableForTest(
      "my_param", int32_type,
      VariableLocation(DwarfExpr({llvm::dwarf::DW_OP_lit20, llvm::dwarf::DW_OP_stack_value})));
  func->set_parameters({LazySymbol(param)});

  // Local variable in an inner block
  auto block = fxl::MakeRefCounted<CodeBlock>(DwarfTag::kLexicalBlock);
  block->set_code_ranges(AddressRanges(AddressRange(kAddress1, kAddress1 + 0x100)));
  SymbolTestParentSetter block_parent(block, func);
  func->set_inner_blocks({LazySymbol(block)});

  auto local_var = MakeVariableForTest(
      "my_local", int32_type,
      VariableLocation(DwarfExpr({llvm::dwarf::DW_OP_lit30, llvm::dwarf::DW_OP_stack_value})));
  block->set_variables({LazySymbol(local_var)});

  Location loc(kAddress1, FileLine("my_file.cc", 42), 0, SymbolContext::ForRelativeAddresses(),
               LazySymbol(func));

  std::vector<std::unique_ptr<Frame>> frames;
  frames.push_back(std::make_unique<MockFrame>(&session(), thread(), loc, 0x1000));
  frames.push_back(std::make_unique<MockFrame>(&session(), thread(), 0x12345600, 0x1010,
                                               "CallerFunction", FileLine("caller.cc", 100)));

  InjectExceptionWithStack(ConsoleTest::kProcessKoid, ConsoleTest::kThreadKoid,
                           debug_ipc::ExceptionType::kSingleStep, std::move(frames), true);

  // Backtrace includes parameters and locals includes local variables.
  std::string script =
      "[zxdb] backtrace\n"
      "▶ 0 MyFunction(…) • my_file.cc:42\n"
      "      my_param = 20\n"
      "  1 CallerFunction() • caller.cc:100\n"
      "[zxdb] locals\n"
      "my_local = 30\n"
      "my_param = 20\n";
  EXPECT_TRUE(RunScript(script)) << GetFailureContext();
}

}  // namespace

}  // namespace zxdb
