// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>
#include <zircon/types.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <rapidjson/document.h>
#include <rapidjson/error/en.h>
#include <rapidjson/pointer.h>

#include "log_decoder.h"
#include "log_decoder_api.h"

namespace log_decoder {
namespace {

fuchsia::logger::LogMessage DecodeBuffer(fuchsia_logging::LogBuffer& buffer) {
  std::span<const uint8_t> span = buffer.EndRecord();
  auto messages = fuchsia_decode_log_messages_to_struct(span.data(), span.size(), true, nullptr);
  EXPECT_EQ(messages.messages.len, 1u);
  auto fidl_res = ToFidlLogMessage(*messages.messages.ptr[0]);
  EXPECT_TRUE(fidl_res.is_ok());
  fuchsia_free_log_messages(messages);
  return fidl_res.take_value();
}

TEST(LogDecoder, DecodesCorrectly) {
  fuchsia_logging::LogBufferBuilder builder(fuchsia_logging::LogSeverity::Info);
  auto buffer = builder.WithMsg("test message").WithFile(__FILE__, __LINE__).Build();
  buffer.WriteKeyValue("tag", "some tag");
  buffer.WriteKeyValue("tag", "some other tag");
  buffer.WriteKeyValue("user property", 5.2);
  std::span<const uint8_t> span = buffer.EndRecord();
  const char* json = fuchsia_decode_log_message_to_json(span.data(), span.size());

  rapidjson::Document d;
  d.Parse(json);
  auto& entry = d[rapidjson::SizeType(0)];
  auto& tags = entry["metadata"]["tags"];
  auto& payload = entry["payload"]["root"];
  auto& keys = payload["keys"];
  ASSERT_EQ(tags[0].GetString(), std::string("some tag"));
  ASSERT_EQ(tags[1].GetString(), std::string("some other tag"));
  ASSERT_EQ(keys["user property"].GetDouble(), 5.2);
  ASSERT_EQ(payload["message"]["value"].GetString(), std::string("test message"));
  fuchsia_free_decoded_log_message(const_cast<char*>(json));
}

TEST(LogDecoder, DecodeInvalidBytesReturnsNull) {
  const uint8_t invalid_bytes[] = {0x01, 0x02, 0x03, 0x04};
  const char* json = fuchsia_decode_log_message_to_json(invalid_bytes, sizeof(invalid_bytes));
  EXPECT_EQ(json, nullptr);

  const char* json_null = fuchsia_decode_log_message_to_json(nullptr, 0);
  EXPECT_EQ(json_null, nullptr);
}

TEST(LogDecoder, DecodesArchivistArguments) {
  constexpr char kTestMoniker[] = "some_moniker";
  constexpr char kTestUrl[] = "fuchsia-pkg://fuchsia.com/test#test.cm";
  fuchsia_logging::LogBufferBuilder builder(fuchsia_logging::LogSeverity::Info);
  auto buffer = builder.WithMsg("test message").WithFile("test_file", 42).Build();
  std::span<const uint8_t> span = buffer.EndRecord();
  std::vector<uint8_t> new_buffer(span.size() + 4 + 4 + 8 + ((sizeof(kTestMoniker) - 1 + 7) & ~7) +
                                  ((sizeof(kTestUrl) - 1 + 7) & ~7));
  memcpy(new_buffer.data(), span.data(), span.size());
  size_t current_offset = span.size();
  uint32_t moniker_len = sizeof(kTestMoniker) - 1;
  uint32_t url_len = sizeof(kTestUrl) - 1;
  uint64_t rolled_out_logs = 1;
  memcpy(new_buffer.data() + current_offset, &moniker_len, 4);
  current_offset += 4;
  memcpy(new_buffer.data() + current_offset, &url_len, 4);
  current_offset += 4;
  memcpy(new_buffer.data() + current_offset, &rolled_out_logs, 8);
  current_offset += 8;
  memcpy(new_buffer.data() + current_offset, kTestMoniker, moniker_len);
  current_offset += (moniker_len + 7) & ~7;
  memcpy(new_buffer.data() + current_offset, kTestUrl, url_len);
  current_offset += (url_len + 7) & ~7;

  auto messages =
      fuchsia_decode_log_messages_to_struct(new_buffer.data(), current_offset, true, nullptr);
  ASSERT_EQ(messages.messages.len, static_cast<size_t>(1));
  ASSERT_EQ(messages.messages.ptr[0]->tags.len, static_cast<size_t>(0));
  auto fidl_res = ToFidlLogMessage(*messages.messages.ptr[0]);
  ASSERT_TRUE(fidl_res.is_ok());
  auto fidl_msg = fidl_res.take_value();
  ASSERT_EQ(fidl_msg.tags.size(), 1u);
  EXPECT_EQ(fidl_msg.tags[0], kTestMoniker);
  EXPECT_EQ(fidl_msg.msg, "[test_file(42)] test message");
  fuchsia_free_log_messages(messages);
}

TEST(LogDecoderApi, ToFidlLogMessageMonikerTag) {
  constexpr char kTestMoniker[] = "core/network/netstack";
  constexpr char kTestUrl[] = "fuchsia-pkg://fuchsia.com/netstack#meta/netstack.cm";
  fuchsia_logging::LogBufferBuilder builder(fuchsia_logging::LogSeverity::Info);
  auto buffer = builder.WithMsg("hello network").WithFile("net.cc", 100).Build();
  buffer.WriteKeyValue("tag", "custom_tag");
  std::span<const uint8_t> span = buffer.EndRecord();
  std::vector<uint8_t> new_buffer(span.size() + 4 + 4 + 8 + ((sizeof(kTestMoniker) - 1 + 7) & ~7) +
                                  ((sizeof(kTestUrl) - 1 + 7) & ~7));
  memcpy(new_buffer.data(), span.data(), span.size());
  size_t current_offset = span.size();
  uint32_t moniker_len = sizeof(kTestMoniker) - 1;
  uint32_t url_len = sizeof(kTestUrl) - 1;
  uint64_t rolled_out_logs = 0;
  memcpy(new_buffer.data() + current_offset, &moniker_len, 4);
  current_offset += 4;
  memcpy(new_buffer.data() + current_offset, &url_len, 4);
  current_offset += 4;
  memcpy(new_buffer.data() + current_offset, &rolled_out_logs, 8);
  current_offset += 8;
  memcpy(new_buffer.data() + current_offset, kTestMoniker, moniker_len);
  current_offset += (moniker_len + 7) & ~7;
  memcpy(new_buffer.data() + current_offset, kTestUrl, url_len);
  current_offset += (url_len + 7) & ~7;

  auto messages =
      fuchsia_decode_log_messages_to_struct(new_buffer.data(), current_offset, true, nullptr);
  ASSERT_EQ(messages.messages.len, 1u);
  const LogMessage* raw_msg = messages.messages.ptr[0];
  EXPECT_EQ(raw_msg->tags.len, 1u);
  auto fidl_res = ToFidlLogMessage(*raw_msg);
  ASSERT_TRUE(fidl_res.is_ok());
  auto fidl_msg = fidl_res.take_value();
  ASSERT_EQ(fidl_msg.tags.size(), 2u);
  EXPECT_EQ(fidl_msg.tags[0], "netstack");
  EXPECT_EQ(fidl_msg.tags[1], "custom_tag");
  EXPECT_EQ(fidl_msg.msg, "[net.cc(100)] hello network");
  fuchsia_free_log_messages(messages);
}

TEST(LogDecoder, HandlesInvalidInput) {
  // A simple invalid byte sequence. A valid log message would have a different structure.
  const uint8_t invalid_data[] = {0xDE, 0xAD, 0xBE, 0xEF};
  auto messages =
      fuchsia_decode_log_messages_to_struct(invalid_data, sizeof(invalid_data), true, nullptr);
  ASSERT_EQ(std::string(messages.error_str), std::string("couldn't parse message: InvalidHeader"));
  fuchsia_free_log_messages(messages);
}

TEST(LogDecoderApi, ToFidlLogMessageMultipleArguments) {
  fuchsia_logging::LogBufferBuilder builder(fuchsia_logging::LogSeverity::Info);
  auto buffer = builder.WithMsg("test message").WithFile("test_file", 42).Build();
  buffer.WriteKeyValue("tag", "my_tag");
  buffer.WriteKeyValue("int_key", int64_t{-123});
  buffer.WriteKeyValue("uint_key", uint64_t{456});
  buffer.WriteKeyValue("double_key", 3.14);
  buffer.WriteKeyValue("neg_double", -2.5);
  buffer.WriteKeyValue("zero_double", 0.0);
  buffer.WriteKeyValue("bool_key", true);
  buffer.WriteKeyValue("bool_false", false);
  buffer.WriteKeyValue("text_key", "hello \"world\" \\ test");
  buffer.WriteKeyValue("empty_text", "");

  auto fidl_msg = DecodeBuffer(buffer);
  EXPECT_EQ(fidl_msg.severity, static_cast<int32_t>(fuchsia_logging::LogSeverity::Info));
  ASSERT_EQ(fidl_msg.tags.size(), 1u);
  EXPECT_EQ(fidl_msg.tags[0], "my_tag");
  EXPECT_EQ(fidl_msg.msg,
            "[test_file(42)] test message int_key=-123 uint_key=456 double_key=3.140000 "
            "neg_double=-2.500000 zero_double=0.000000 bool_key=true bool_false=false "
            "text_key=\"hello \\\"world\\\" \\\\ test\" empty_text=\"\"");
}

TEST(LogDecoderApi, ToFidlLogMessageMessageNoFile) {
  fuchsia_logging::LogBufferBuilder builder(fuchsia_logging::LogSeverity::Info);
  auto buffer = builder.WithMsg("test message").Build();
  buffer.WriteKeyValue("foo", "bar");
  buffer.WriteKeyValue("num", int64_t{10});
  auto fidl_msg = DecodeBuffer(buffer);
  EXPECT_EQ(fidl_msg.msg, "test message foo=\"bar\" num=10");
}

TEST(LogDecoderApi, ToFidlLogMessageNoArguments) {
  fuchsia_logging::LogBufferBuilder builder(fuchsia_logging::LogSeverity::Info);
  auto buffer = builder.WithMsg("test message").WithFile("test_file", 42).Build();
  auto fidl_msg = DecodeBuffer(buffer);
  EXPECT_EQ(fidl_msg.msg, "[test_file(42)] test message");
}

TEST(LogDecoderApi, ToFidlLogMessageOneArgument) {
  fuchsia_logging::LogBufferBuilder builder(fuchsia_logging::LogSeverity::Info);
  auto buffer = builder.WithMsg("test message").WithFile("test_file", 42).Build();
  buffer.WriteKeyValue("foo", "bar");
  auto fidl_msg = DecodeBuffer(buffer);
  EXPECT_EQ(fidl_msg.msg, "[test_file(42)] test message foo=\"bar\"");
}

TEST(LogDecoderApi, ToFidlLogMessageNoMessage) {
  fuchsia_logging::LogBufferBuilder builder0(fuchsia_logging::LogSeverity::Info);
  auto buffer0 = builder0.WithMsg("").WithFile("test_file", 42).Build();
  EXPECT_EQ(DecodeBuffer(buffer0).msg, "[test_file(42)]");

  fuchsia_logging::LogBufferBuilder builder1(fuchsia_logging::LogSeverity::Info);
  auto buffer1 = builder1.WithMsg("").WithFile("test_file", 42).Build();
  buffer1.WriteKeyValue("key", int64_t{100});
  EXPECT_EQ(DecodeBuffer(buffer1).msg, "[test_file(42)] key=100");

  fuchsia_logging::LogBufferBuilder builder2(fuchsia_logging::LogSeverity::Info);
  auto buffer2 = builder2.WithMsg("").WithFile("test_file", 42).Build();
  buffer2.WriteKeyValue("k1", int64_t{1});
  buffer2.WriteKeyValue("k2", int64_t{2});
  EXPECT_EQ(DecodeBuffer(buffer2).msg, "[test_file(42)] k1=1 k2=2");
}

TEST(LogDecoderApi, ToFidlLogMessageNoMessageNoFile) {
  fuchsia_logging::LogBufferBuilder builder0(fuchsia_logging::LogSeverity::Info);
  auto buffer0 = builder0.WithMsg("").Build();
  EXPECT_EQ(DecodeBuffer(buffer0).msg, "");

  fuchsia_logging::LogBufferBuilder builder1(fuchsia_logging::LogSeverity::Info);
  auto buffer1 = builder1.WithMsg("").Build();
  buffer1.WriteKeyValue("only_key", "val");
  EXPECT_EQ(DecodeBuffer(buffer1).msg, "only_key=\"val\"");

  fuchsia_logging::LogBufferBuilder builder2(fuchsia_logging::LogSeverity::Info);
  auto buffer2 = builder2.WithMsg("").Build();
  buffer2.WriteKeyValue("a", int64_t{10});
  buffer2.WriteKeyValue("b", int64_t{20});
  EXPECT_EQ(DecodeBuffer(buffer2).msg, "a=10 b=20");
}

}  // namespace
}  // namespace log_decoder
