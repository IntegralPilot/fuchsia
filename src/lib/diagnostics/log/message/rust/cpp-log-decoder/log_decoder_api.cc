// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/diagnostics/log/message/rust/cpp-log-decoder/log_decoder_api.h"

#include <lib/zx/time.h>

#include <sstream>
#include <string>
#include <string_view>
#include <vector>

namespace log_decoder {

namespace {

void AppendEscaped(std::string* out, std::string_view input) {
  for (char ch : input) {
    if (ch == '"' || ch == '\\') {
      out->push_back('\\');
    }
    out->push_back(ch);
  }
}

std::string_view StringViewFromRustString(CppString rust_string) {
  if (rust_string.ptr && rust_string.len > 0) {
    return std::string_view(reinterpret_cast<const char*>(rust_string.ptr), rust_string.len);
  }
  return {};
}

std::string FormatMessageWithKvps(const LogMessage& message) {
  std::string_view file_view = StringViewFromRustString(message.file);
  std::string_view msg_view = StringViewFromRustString(message.message);

  size_t estimated_size = msg_view.size();
  if (!file_view.empty()) {
    estimated_size += file_view.size() + 26;
  }

  for (size_t i = 0; i < message.kvps.len; i++) {
    const auto& kvp = message.kvps.ptr[i];
    estimated_size += 2 + kvp.key.len;
    switch (kvp.value.tag) {
      case CppValue::Tag::CPP_VALUE_SIGNED_INT:
      case CppValue::Tag::CPP_VALUE_UNSIGNED_INT:
        estimated_size += 24;
        break;
      case CppValue::Tag::CPP_VALUE_FLOATING:
        estimated_size += 32;
        break;
      case CppValue::Tag::CPP_VALUE_BOOLEAN:
        estimated_size += 5;
        break;
      case CppValue::Tag::CPP_VALUE_TEXT:
        estimated_size += 2 + 2 * kvp.value.TEXT._0.len;
        break;
    }
  }

  std::string result;
  result.reserve(estimated_size);

  if (!file_view.empty()) {
    result.push_back('[');
    result.append(file_view);
    result.push_back('(');
    result.append(std::to_string(message.line));
    result.append(")]");
    if (!msg_view.empty()) {
      result.push_back(' ');
    }
  }

  result.append(msg_view);

  for (size_t i = 0; i < message.kvps.len; i++) {
    if (!result.empty()) {
      result.push_back(' ');
    }
    const auto& kvp = message.kvps.ptr[i];
    result.append(StringViewFromRustString(kvp.key));
    result.push_back('=');
    switch (kvp.value.tag) {
      case CppValue::Tag::CPP_VALUE_SIGNED_INT:
        result.append(std::to_string(kvp.value.SIGNED_INT._0));
        break;
      case CppValue::Tag::CPP_VALUE_UNSIGNED_INT:
        result.append(std::to_string(kvp.value.UNSIGNED_INT._0));
        break;
      case CppValue::Tag::CPP_VALUE_FLOATING:
        result.append(std::to_string(kvp.value.FLOATING._0));
        break;
      case CppValue::Tag::CPP_VALUE_BOOLEAN:
        result.append(kvp.value.BOOLEAN._0 ? "true" : "false");
        break;
      case CppValue::Tag::CPP_VALUE_TEXT:
        result.push_back('"');
        AppendEscaped(&result, StringViewFromRustString(kvp.value.TEXT._0));
        result.push_back('"');
        break;
    }
  }
  return result;
}

}  // namespace

fpromise::result<fuchsia::logger::LogMessage, std::string> ToFidlLogMessage(
    const LogMessage& message) {
  std::vector<std::string> tags;
  tags.reserve(message.tags.len + 1);
  std::string_view moniker_tag = StringViewFromRustString(message.moniker_tag);
  bool has_moniker_tag = false;
  for (size_t i = 0; i < message.tags.len; i++) {
    std::string_view tag_view = StringViewFromRustString(message.tags.ptr[i]);
    if (tag_view == moniker_tag) {
      has_moniker_tag = true;
    }
    tags.emplace_back(tag_view);
  }
  if (!moniker_tag.empty() && !has_moniker_tag) {
    tags.insert(tags.begin(), std::string(moniker_tag));
  }
  fuchsia::logger::LogMessage ret = {
      .pid = message.pid,
      .tid = message.tid,
      .time = zx::time_boot(message.timestamp),
      .severity = message.severity,
      .dropped_logs = static_cast<uint32_t>(message.dropped),
      .tags = std::move(tags),
      .msg = FormatMessageWithKvps(message),
  };
  return fpromise::ok(std::move(ret));
}

}  // namespace log_decoder
