// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/symbolizer/log_parser.h"

#include <sstream>
#include <string_view>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/debug/ipc/records.h"
#include "tools/symbolizer/symbolizer.h"

namespace symbolizer {

namespace {

using ::testing::_;

class MockSymbolizer : public Symbolizer {
 public:
  MOCK_METHOD(void, Reset, (bool symbolizing_dart, ResetType type), (override));
  MOCK_METHOD(void, Module, (uint64_t id, std::string_view name, std::string_view build_id),
              (override));
  MOCK_METHOD(void, MMap,
              (uint64_t address, uint64_t size, uint64_t module_id, std::string_view flags,
               uint64_t module_offset, StringOutputFn output),
              (override));
  MOCK_METHOD(void, Backtrace,
              (uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
               std::string_view message, StringOutputFn output),
              (override));
  MOCK_METHOD(void, DumpFile, (std::string_view type, std::string_view name), (override));
};

class LogParserTest : public ::testing::Test {
 public:
  LogParserTest() : log_parser(input_, output_, &symbolizer_) {}

  void ProcessOneLine(const char* input) {
    input_ << input << std::endl;
    ASSERT_TRUE(log_parser.ProcessNextLine());
  }

 protected:
  std::stringstream input_;
  std::stringstream output_;
  MockSymbolizer symbolizer_;
  LogParser log_parser;
};

TEST_F(LogParserTest, NoMarkup) {
  ProcessOneLine("normal content");
  ASSERT_EQ(output_.str(), "normal content\n");
  ProcessOneLine("{{{invalid_tag}}}");
  ASSERT_EQ(output_.str(), "normal content\n{{{invalid_tag}}}\n");
}

TEST_F(LogParserTest, ResetWithContext) {
  EXPECT_CALL(symbolizer_, Reset(false, Symbolizer::ResetType::kUnknown)).Times(1);
  ProcessOneLine("prefix {{{reset}}} suffix");
  ASSERT_EQ(output_.str(), "");
}

TEST_F(LogParserTest, Module) {
  EXPECT_CALL(symbolizer_, Module(0, "libc.so", "8ce60b"));
  ProcessOneLine("context1: {{{module:0x0:libc.so:elf:8ce60b}}}");
  EXPECT_CALL(symbolizer_, Module(5, "libc.so", "8ce60b"));
  ProcessOneLine("context2: {{{module:0x5:libc.so:elf:8ce60b:unnecessary_content}}}");
  EXPECT_CALL(symbolizer_, Module(3, "", "8ce60b"));
  ProcessOneLine("context3: {{{module:0x3::elf:8ce60b}}}");
  EXPECT_CALL(symbolizer_, Module(4, "<VMO#16651=bootfs:blob/29c1ed67...>", "cd01d7d47fe..."));
  ProcessOneLine(
      "context5: {{{module:0x4:<VMO#16651=bootfs:blob/29c1ed67...>:elf:cd01d7d47fe...}}}");
  EXPECT_CALL(symbolizer_, Module(6, "<VMO#16651=bootfs:blob/29c1ed67...>", "cd01d7d47fe..."));
  ProcessOneLine(
      "context6: "
      "{{{module:0x6:<VMO#16651=bootfs:blob/29c1ed67...>:elf:cd01d7d47fe...:unnecessary_stuff}}}");
  ASSERT_EQ(output_.str(), "");
  EXPECT_CALL(symbolizer_, Module).Times(0);
  ProcessOneLine("context4: {{{module:0x5:libc.so:not_elf:8ce60b}}}");
  ASSERT_EQ(output_.str(), "context4: {{{module:0x5:libc.so:not_elf:8ce60b}}}\n");
}

TEST_F(LogParserTest, MMap) {
  EXPECT_CALL(symbolizer_, MMap(0xbb57d35000, 0x2000, 0, "r", 0, _));
  ProcessOneLine("{{{mmap:0xbb57d35000:0x2000:load:0:r:0}}}");
}

TEST_F(LogParserTest, Backtrace) {
  EXPECT_CALL(symbolizer_,
              Backtrace(1, 0xbb57d370b0, debug_ipc::StackFrame::AddressType::kUnknown, "", _));
  ProcessOneLine("{{{bt:1:0xbb57d370b0}}}");
  EXPECT_CALL(symbolizer_, Backtrace(1, 0xbb57d370b0, debug_ipc::StackFrame::AddressType::kUnknown,
                                     "sp 0x3f540e65ef0", _));
  ProcessOneLine("{{{bt:1:0xbb57d370b0:sp 0x3f540e65ef0}}}");
  EXPECT_CALL(symbolizer_,
              Backtrace(1, 0xbb57d370b0, debug_ipc::StackFrame::AddressType::kExact, "", _));
  ProcessOneLine("{{{bt:1:0xbb57d370b0:pc}}}");
  EXPECT_CALL(symbolizer_, Backtrace(1, 0xbb57d370b0, debug_ipc::StackFrame::AddressType::kExact,
                                     "sp 0x3f540e65ef0", _));
  ProcessOneLine("{{{bt:1:0xbb57d370b0:pc:sp 0x3f540e65ef0}}}");
  ASSERT_EQ(output_.str(), "");
}

TEST_F(LogParserTest, DumpFile) {
  EXPECT_CALL(symbolizer_, DumpFile("type", "name"));
  ProcessOneLine("{{{dumpfile:type:name}}}");
}

TEST_F(LogParserTest, MultipleMarkup) {
  // Multiple bt tags on the same line.
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_0_symbolized"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(1, 0x2000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_1_symbolized"); });
  ProcessOneLine("prefix {{{bt:0:0x1000}}} middle {{{bt:1:0x2000}}} suffix");
  EXPECT_EQ(output_.str(), "prefix frame_0_symbolized\n middle frame_1_symbolized suffix\n");
  output_.str("");
  output_.clear();

  // module + bt tags on the same line.
  EXPECT_CALL(symbolizer_, Module(0, "libc.so", "8ce60b"));
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x3000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_bt_symbolized"); });
  ProcessOneLine(
      "context1: {{{module:0x0:libc.so:elf:8ce60b}}} context2: {{{bt:0:0x3000}}} suffix");
  EXPECT_EQ(output_.str(), "context1:  context2: frame_bt_symbolized suffix\n");
  output_.str("");
  output_.clear();

  // Invalid tags intermingled with valid tags.
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x4000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_valid_symbolized"); });
  ProcessOneLine("foo {{{invalid:tag}}} bar {{{bt:0:0x4000}}} baz {{{unclosed:tag");
  EXPECT_EQ(output_.str(),
            "foo {{{invalid:tag}}} bar frame_valid_symbolized baz {{{unclosed:tag\n");
  output_.str("");
  output_.clear();

  // Zero-length prefix and suffix logic.
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x5000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_zero_len"); });
  ProcessOneLine("{{{bt:0:0x5000}}}");
  EXPECT_EQ(output_.str(), "frame_zero_len\n");
  output_.str("");
  output_.clear();

  // 10+ consecutive tags on a single line.
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) { output("f0"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(1, 0x2000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) { output("f1"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(2, 0x3000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) { output("f2"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(3, 0x4000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) { output("f3"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(4, 0x5000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) { output("f4"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(5, 0x6000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) { output("f5"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(6, 0x7000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) { output("f6"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(7, 0x8000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) { output("f7"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(8, 0x9000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) { output("f8"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(9, 0xa000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) { output("f9"); });
  ProcessOneLine(
      "{{{bt:0:0x1000}}}{{{bt:1:0x2000}}}{{{bt:2:0x3000}}}{{{bt:3:0x4000}}}{{{bt:4:0x5000}}}{{{"
      "bt:5:0x6000}}}{{{bt:6:0x7000}}}{{{bt:7:0x8000}}}{{{bt:8:0x9000}}}{{{bt:9:0xa000}}}");
  EXPECT_EQ(output_.str(), "f0\nf1\nf2\nf3\nf4\nf5\nf6\nf7\nf8\nf9\n");
  output_.str("");
  output_.clear();

  // Intermingled valid backtraces, module resets, and arbitrary text.
  EXPECT_CALL(symbolizer_, Module(0, "libc.so", "8ce60b"));
  EXPECT_CALL(symbolizer_, Reset(false, Symbolizer::ResetType::kUnknown));
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_0"); });
  EXPECT_CALL(symbolizer_, DumpFile("type", "name"));
  EXPECT_CALL(symbolizer_,
              Backtrace(1, 0x2000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_1"); });

  ProcessOneLine(
      "text1 {{{module:0x0:libc.so:elf:8ce60b}}} text2 {{{reset}}} text3 "
      "{{{bt:0:0x1000}}} text4 {{{dumpfile:type:name}}} text5 {{{bt:1:0x2000}}} text6");
  EXPECT_EQ(output_.str(), "text1  text2  text3 frame_0\n text4  text5 frame_1 text6\n");
}

TEST_F(LogParserTest, MalformedAndUnclosedTagsMixed) {
  EXPECT_CALL(symbolizer_, Backtrace(0, 0x100, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_0"); });
  EXPECT_CALL(symbolizer_, Reset(false, Symbolizer::ResetType::kUnknown)).Times(1);
  ProcessOneLine("{{{bt:0:0x100}}}{{{invalid{{{reset}}}");
  EXPECT_EQ(output_.str(), "frame_0\n{{{invalid\n");
  output_.str("");
  output_.clear();

  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_valid"); });
  ProcessOneLine("prefix {{{bt:0:0x1000}}} middle {{{unclosed_tag_without_end");
  EXPECT_EQ(output_.str(), "prefix frame_valid middle {{{unclosed_tag_without_end\n");
}

TEST_F(LogParserTest, InvalidTagWithValidNonPrintingTag) {
  EXPECT_CALL(symbolizer_, Module(0, "libc.so", "8ce60b")).Times(1);
  ProcessOneLine("prefix {{{module:0x0:libc.so:elf:8ce60b}}} middle {{{invalid_tag}}} suffix");
  EXPECT_EQ(output_.str(), "prefix  middle {{{invalid_tag}}} suffix\n");
}

TEST_F(LogParserTest, ZeroLengthPrefixesAndSuffixes) {
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_0"); });
  EXPECT_CALL(symbolizer_,
              Backtrace(1, 0x2000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_1"); });
  ProcessOneLine("{{{bt:0:0x1000}}}{{{bt:1:0x2000}}}");
  EXPECT_EQ(output_.str(), "frame_0\nframe_1\n");
}

TEST_F(LogParserTest, TenPlusConsecutiveTagsOnSingleLine) {
  for (uint64_t i = 0; i < 12; ++i) {
    EXPECT_CALL(symbolizer_, Backtrace(i, 0x1000 + i * 0x10,
                                       debug_ipc::StackFrame::AddressType::kUnknown, "", _))
        .WillOnce([i](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                      std::string_view message,
                      Symbolizer::StringOutputFn output) { output("frame_" + std::to_string(i)); });
  }
  ProcessOneLine(
      "{{{bt:0:0x1000}}}{{{bt:1:0x1010}}}{{{bt:2:0x1020}}}{{{bt:3:0x1030}}}"
      "{{{bt:4:0x1040}}}{{{bt:5:0x1050}}}{{{bt:6:0x1060}}}{{{bt:7:0x1070}}}"
      "{{{bt:8:0x1080}}}{{{bt:9:0x1090}}}{{{bt:10:0x10a0}}}{{{bt:11:0x10b0}}}");
  EXPECT_EQ(output_.str(),
            "frame_0\nframe_1\nframe_2\nframe_3\nframe_4\nframe_5\n"
            "frame_6\nframe_7\nframe_8\nframe_9\nframe_10\nframe_11\n");
}

TEST_F(LogParserTest, IntermingledTagsAndArbitraryText) {
  EXPECT_CALL(symbolizer_, Module(0, "libc.so", "8ce60b"));
  EXPECT_CALL(symbolizer_, Reset(false, Symbolizer::ResetType::kUnknown));
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_0"); });
  EXPECT_CALL(symbolizer_, DumpFile("type", "name"));
  EXPECT_CALL(symbolizer_,
              Backtrace(1, 0x2000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_1"); });

  ProcessOneLine(
      "text1 {{{module:0x0:libc.so:elf:8ce60b}}} text2 {{{reset}}} text3 "
      "{{{bt:0:0x1000}}} text4 {{{dumpfile:type:name}}} text5 {{{bt:1:0x2000}}} text6");
  EXPECT_EQ(output_.str(), "text1  text2  text3 frame_0\n text4  text5 frame_1 text6\n");
}

TEST_F(LogParserTest, OutputBufferQueueLifecycle) {
  Symbolizer::StringOutputFn cb1;
  Symbolizer::StringOutputFn cb2;

  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([&cb1](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                       std::string_view message,
                       Symbolizer::StringOutputFn output) { cb1 = std::move(output); });
  EXPECT_CALL(symbolizer_,
              Backtrace(1, 0x2000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([&cb2](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                       std::string_view message,
                       Symbolizer::StringOutputFn output) { cb2 = std::move(output); });

  ProcessOneLine("line1 {{{bt:0:0x1000}}}");
  ProcessOneLine("line2 {{{bt:1:0x2000}}}");

  // Output hasn't completed yet.
  EXPECT_EQ(output_.str(), "");

  // Out of order callback destruction: destroy cb2 first.
  cb2("frame_1_symbolized");
  cb2 = nullptr;

  // cb2 completion is buffered because cb1 (front of queue) is not ready yet.
  EXPECT_EQ(output_.str(), "");

  // Now complete cb1.
  cb1("frame_0_symbolized");
  cb1 = nullptr;

  // Both should now be flushed in FIFO order.
  EXPECT_EQ(output_.str(), "line1 frame_0_symbolized\nline2 frame_1_symbolized\n");
}

TEST_F(LogParserTest, OutputRawWithPendingBuffer) {
  Symbolizer::StringOutputFn cb1;

  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([&cb1](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                       std::string_view message,
                       Symbolizer::StringOutputFn output) { cb1 = std::move(output); });

  ProcessOneLine("line1 {{{bt:0:0x1000}}}");
  ProcessOneLine("raw line 1");
  ProcessOneLine("raw line 2");

  // Output is buffered waiting for cb1.
  EXPECT_EQ(output_.str(), "");

  // Invoking cb1 and destroying it flushes cb1 output followed by raw lines in order.
  cb1("frame_0_symbolized");
  cb1 = nullptr;
  EXPECT_EQ(output_.str(), "line1 frame_0_symbolized\nraw line 1\nraw line 2\n");
}

TEST_F(LogParserTest, MultipleFramesPerCallback) {
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message, Symbolizer::StringOutputFn output) {
        output("frame_0_inline_1");
        output("frame_0_inline_2");
        output("frame_0_physical");
      });
  ProcessOneLine("prefix {{{bt:0:0x1000}}} suffix");
  EXPECT_EQ(output_.str(),
            "prefix frame_0_inline_1 suffix\n"
            "prefix frame_0_inline_2 suffix\n"
            "prefix frame_0_physical suffix\n");
}

TEST_F(LogParserTest, DroppedCallback) {
  Symbolizer::StringOutputFn cb1;
  Symbolizer::StringOutputFn cb2;

  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([&cb1](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                       std::string_view message,
                       Symbolizer::StringOutputFn output) { cb1 = std::move(output); });
  EXPECT_CALL(symbolizer_,
              Backtrace(1, 0x2000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([&cb2](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                       std::string_view message,
                       Symbolizer::StringOutputFn output) { cb2 = std::move(output); });

  ProcessOneLine("line1 {{{bt:0:0x1000}}}");
  ProcessOneLine("line2 {{{bt:1:0x2000}}}");

  // cb1 is dropped without being invoked.
  cb1 = nullptr;

  // cb1 dropped, so queue advances to cb2, which is still pending.
  EXPECT_EQ(output_.str(), "");

  // cb2 is invoked and destroyed.
  cb2("frame_1_symbolized");
  cb2 = nullptr;
  EXPECT_EQ(output_.str(), "line2 frame_1_symbolized\n");
}

TEST_F(LogParserTest, StrayOpenDelimiters) {
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_0"); });
  ProcessOneLine("random {{{ text {{{bt:0:0x1000}}}");
  EXPECT_EQ(output_.str(), "random {{{ text frame_0\n");
  output_.str("");
  output_.clear();

  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x2000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_nested"); });
  ProcessOneLine("{{{ {{{ {{{bt:0:0x2000}}} suffix");
  EXPECT_EQ(output_.str(), "{{{ {{{ frame_nested suffix\n");
}

TEST_F(LogParserTest, StrayCloseDelimiters) {
  EXPECT_CALL(symbolizer_,
              Backtrace(0, 0x1000, debug_ipc::StackFrame::AddressType::kUnknown, "", _))
      .WillOnce([](uint64_t frame_id, uint64_t address, debug_ipc::StackFrame::AddressType type,
                   std::string_view message,
                   Symbolizer::StringOutputFn output) { output("frame_0"); });
  ProcessOneLine("foo }}} bar {{{bt:0:0x1000}}}");
  EXPECT_EQ(output_.str(), "foo }}} bar frame_0\n");
}

TEST_F(LogParserTest, Dart) {
  {
    EXPECT_CALL(symbolizer_, Reset(true, Symbolizer::ResetType::kUnknown));
    ProcessOneLine("*** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***");
    EXPECT_FALSE(output_.str().empty());
    output_.clear();
    ProcessOneLine("pid: 12, tid: 30221, name some.ui");
    EXPECT_FALSE(output_.str().empty());
    output_.clear();
  }
  {
    EXPECT_CALL(symbolizer_, Module(0, "some.ui", "0123456789abcdef"));
    ProcessOneLine("build_id: '0123456789abcdef'");
    EXPECT_FALSE(output_.str().empty());
    output_.clear();
  }
  {
    EXPECT_CALL(symbolizer_, MMap(0xf2e4c8000, 0x800000000, 0, "", 0, _));
    ProcessOneLine("isolate_dso_base: f2e4c8000, vm_dso_base: f2e4c8000");
    EXPECT_FALSE(output_.str().empty());
    output_.clear();
  }
  ProcessOneLine("isolate_instructions: f2f9f8e60, vm_instructions: f2f9f4000");
  EXPECT_FALSE(output_.str().empty());
  output_.clear();
  {
    EXPECT_CALL(symbolizer_, Backtrace(0, 0x0000000f2fbb51c7,
                                       debug_ipc::StackFrame::AddressType::kUnknown, "", _));
    ProcessOneLine(
        "#00 abs 0000000f2fbb51c7 virt 00000000016ed1c7 "
        "_kDartIsolateSnapshotInstructions+0x1bc367");
    EXPECT_FALSE(output_.str().empty());
    output_.clear();
  }
}

TEST_F(LogParserTest, MultiLineMarkup) {
  ProcessOneLine("line1 prefix {{{bt:0:0x1000");
  EXPECT_EQ(output_.str(), "line1 prefix {{{bt:0:0x1000\n");
  output_.str("");
  output_.clear();
  ProcessOneLine("pc}}} line2 suffix");
  EXPECT_EQ(output_.str(), "pc}}} line2 suffix\n");
}

TEST_F(LogParserTest, AsymmetricBraces) {
  ProcessOneLine("{{{{bt:0:0x1000}}}}");
  EXPECT_EQ(output_.str(), "{{{{bt:0:0x1000}}}}\n");
}

TEST_F(LogParserTest, EmptyTag) {
  ProcessOneLine("prefix {{{}}} middle {{{   }}} suffix");
  EXPECT_EQ(output_.str(), "prefix {{{}}} middle {{{   }}} suffix\n");
}

TEST_F(LogParserTest, NestedBracesInTag) {
  ProcessOneLine("{{{module:0:foo{{{bar:elf:1234}}}");
  EXPECT_EQ(output_.str(), "{{{module:0:foo{{{bar:elf:1234}}}\n");
}

}  // namespace

}  // namespace symbolizer
