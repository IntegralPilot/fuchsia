// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <lib/async/default.h>
#include <lib/async/task.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/stream.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <algorithm>
#include <mutex>
#include <sstream>

#include <rapidjson/document.h>
#include <rapidjson/error/en.h>
#include <rapidjson/pointer.h>
#include <src/lib/diagnostics/accessor2logger/log_message.h>
#include <src/lib/fsl/vmo/strings.h>

#include "src/lib/diagnostics/log/message/rust/cpp-log-decoder/log_decoder_api.h"

using fuchsia::diagnostics::FormattedContent;

namespace diagnostics::accessor2logger {

namespace {

const char kPidLabel[] = "pid";
const char kTidLabel[] = "tid";
const char kFileLabel[] = "file";
const char kLineLabel[] = "line";
const char kTagsLabel[] = "tags";
const char kMessageLabel[] = "value";
const char kVerbosityLabel[] = "verbosity";

inline fuchsia::logger::LogLevelFilter StringToSeverity(const std::string& input) {
  if (strcasecmp(input.c_str(), "trace") == 0) {
    return fuchsia::logger::LogLevelFilter::TRACE;
  }
  if (strcasecmp(input.c_str(), "debug") == 0) {
    return fuchsia::logger::LogLevelFilter::DEBUG;
  }
  if (strcasecmp(input.c_str(), "info") == 0) {
    return fuchsia::logger::LogLevelFilter::INFO;
  }
  if (strcasecmp(input.c_str(), "warn") == 0) {
    return fuchsia::logger::LogLevelFilter::WARN;
  }
  if (strcasecmp(input.c_str(), "error") == 0) {
    return fuchsia::logger::LogLevelFilter::ERROR;
  }
  if (strcasecmp(input.c_str(), "fatal") == 0) {
    return fuchsia::logger::LogLevelFilter::FATAL;
  }

  return fuchsia::logger::LOG_LEVEL_DEFAULT;
}

std::string GetComponentName(const std::string& moniker) {
  size_t pos = moniker.rfind('/');
  if (pos == std::string::npos) {
    return moniker;
  }
  // Monikers should never end in / since / is a special
  // character indicating a component in the topology.
  ZX_DEBUG_ASSERT(pos + 1 < moniker.size());
  return moniker.substr(pos + 1);
}

inline fpromise::result<fuchsia::logger::LogMessage, std::string> JsonToLogMessage(
    rapidjson::Value& value) {
  fuchsia::logger::LogMessage ret = {};
  std::stringstream kv_mapping;

  if (!value.IsObject()) {
    return fpromise::error("Value is not an object");
  }

  auto metadata = value.FindMember("metadata");
  auto payload = value.FindMember("payload");

  if (metadata == value.MemberEnd() || payload == value.MemberEnd() ||
      !metadata->value.IsObject() || !payload->value.IsObject()) {
    return fpromise::error("Expected metadata and payload objects");
  }
  auto root = payload->value.FindMember("root");
  if (root == payload->value.MemberEnd() || !root->value.IsObject()) {
    return fpromise::error("Expected payload.root to be an object if present");
  }

  auto timestamp = metadata->value.FindMember("timestamp");
  if (timestamp == metadata->value.MemberEnd() || !timestamp->value.IsUint64()) {
    return fpromise::error("Expected metadata.timestamp key");
  }
  ret.time = zx::time_boot(timestamp->value.GetInt64());

  auto severity = metadata->value.FindMember("severity");
  if (severity == metadata->value.MemberEnd() || !severity->value.IsString()) {
    return fpromise::error("Expected metadata.severity key");
  }
  ret.severity = static_cast<int32_t>(StringToSeverity(severity->value.GetString()));

  auto moniker = value.FindMember("moniker");
  std::string moniker_string;
  if (moniker != value.MemberEnd() && moniker->value.IsString()) {
    moniker_string = std::move(moniker->value.GetString());
  }

  uint32_t dropped_logs = 0;
  if (metadata->value.HasMember("errors")) {
    auto& errors = metadata->value["errors"];
    if (errors.IsArray()) {
      for (rapidjson::SizeType i = 0; i < errors.Size(); i++) {
        auto* val = rapidjson::Pointer("/dropped_logs/count").Get(errors[i]);
        if (val && val->IsUint()) {
          dropped_logs += val->GetUint();
        }
      }
    }
  }

  // Flatten payloads containing a "root" node.
  // TODO(https://fxbug.dev/42141910): Remove this when "root" is omitted from logs.
  if (payload->value.MemberCount() == 1 && payload->value.HasMember("root")) {
    auto root_member = payload->value.FindMember("root");
    if (root_member == payload->value.MemberEnd() || !root_member->value.IsObject()) {
      return fpromise::error("Expected payload.root to be an object if present");
    }
    auto msg_member = root_member->value.FindMember("message");
    if (msg_member == root_member->value.MemberEnd() || !msg_member->value.IsObject()) {
      return fpromise::error("Expected payload.root.message to be an object if present");
    }
    payload = msg_member;
  }

  std::string msg;
  std::string filename;
  std::optional<int> line_number;
  std::optional<int> verbosity;

  for (auto it = payload->value.MemberBegin(); it != payload->value.MemberEnd(); ++it) {
    if (!it->name.IsString()) {
      return fpromise::error("A key is not a string");
    }
    std::string name = it->name.GetString();
    if (name == kMessageLabel && it->value.IsString()) {
      msg = std::move(it->value.GetString());
    }

    if ((name == kVerbosityLabel) && it->value.IsInt()) {
      verbosity = it->value.GetInt();
      if (ret.severity != verbosity.value_or(ret.severity)) {
        ret.severity = GetSeverityFromVerbosity(static_cast<uint8_t>(verbosity.value()));
      }
    }
  }

  for (auto it = metadata->value.MemberBegin(); it != metadata->value.MemberEnd(); ++it) {
    if (!it->name.IsString()) {
      return fpromise::error("A key is not a string");
    }
    std::string name = it->name.GetString();
    if (name == kTagsLabel) {
      if (it->value.IsString()) {
        ret.tags.emplace_back(std::move(it->value.GetString()));
      } else if (it->value.IsArray()) {
        for (rapidjson::SizeType i = 0; i < it->value.Size(); ++i) {
          auto& val = it->value[i];
          if (!val.IsString()) {
            return fpromise::error("Tags array must contain strings");
          }
          ret.tags.emplace_back(std::move(val.GetString()));
        }
      } else {
        return fpromise::error("Tags must be a string or array of strings");
      }
    } else if (name == kTidLabel && it->value.IsUint64()) {
      ret.tid = it->value.GetUint64();
    } else if (name == kPidLabel && it->value.IsUint64()) {
      ret.pid = it->value.GetUint64();
    } else if (name == kFileLabel && it->value.IsString()) {
      filename = it->value.GetString();
    } else if (name == kLineLabel && it->value.IsUint64()) {
      line_number = it->value.GetUint64();
    }
  }

  auto kvps = root->value.FindMember("keys");

  if ((kvps != root->value.MemberEnd()) && kvps->value.IsObject() && (kvps->name == "keys")) {
    for (auto it = kvps->value.MemberBegin(); it != kvps->value.MemberEnd(); ++it) {
      if (!it->name.IsString()) {
        return fpromise::error("A key is not a string");
      }
      std::string name = it->name.GetString();
      // If the name of the field is not a known special field, treat it as a key/value pair and
      // append to the message.
      kv_mapping << " " << std::move(name) << "=";
      if (it->value.IsInt64()) {
        kv_mapping << it->value.GetInt64();
      } else if (it->value.IsUint64()) {
        kv_mapping << it->value.GetUint64();
      } else if (it->value.IsDouble()) {
        kv_mapping << it->value.GetDouble();
      } else if (it->value.IsString()) {
        kv_mapping << "\"";
        auto str_value = it->value.GetString();
        if (strchr(str_value, '"') != nullptr) {
          // Escape quotes in strings per host encoding.
          size_t len = strlen(str_value);
          for (size_t i = 0; i < len; ++i) {
            char c = str_value[i];
            if (c == '"') {
              kv_mapping << '\\';
            }
            kv_mapping << c;
          }
        } else {
          kv_mapping << std::move(it->value.GetString());
        }
        kv_mapping << "\"";
      } else {
        kv_mapping << "<unknown>";
      }
    }
  }

  if (!filename.empty() && line_number.has_value()) {
    std::stringstream enc;
    enc << "[" << filename << "(" << line_number.value() << ")] ";
    ret.msg = enc.str();
  }

  ret.msg += msg;
  bool needs_add = true;

  if (!ret.msg.empty()) {
    if (ret.msg[ret.msg.size() - 1] == ' ') {
      std::string truncated = kv_mapping.str();
      truncated.erase(0, 1);
      ret.msg += truncated;
      needs_add = false;
    }
  }

  if (needs_add) {
    ret.msg += kv_mapping.str();
  }

  // If there are no tags, automatically tag with the component name derived from the moniker.
  auto component_name = GetComponentName(moniker_string);
  if ((ret.tags.size() == 0 && !component_name.empty()) ||
      (ret.tags.size() && (std::ranges::find(ret.tags, component_name) == ret.tags.end()) &&
       !component_name.empty())) {
    if (component_name != ".") {
      ret.tags.emplace(ret.tags.begin(), component_name);
    }
  }

  if (dropped_logs > 0) {
    ret.dropped_logs = dropped_logs;
  }

  return fpromise::ok(std::move(ret));
}

fpromise::result<void, std::string> ConvertFormattedFxtToLogMessagesInternal(
    std::span<const uint8_t> data, bool expect_extended_attribution, MessageParser* parser,
    std::vector<fpromise::result<fuchsia::logger::LogMessage, std::string>>& output) {
  auto log_messages = fuchsia_decode_log_messages_to_struct(data.data(), data.size(),
                                                            expect_extended_attribution, parser);
  if (log_messages.error_str) {
    // Copy the error string so that we can free it before returning
    std::string copied_error_str = log_messages.error_str;
    FX_LOG_KV(ERROR, "Failed to decode log from Archivist", FX_KV("message", copied_error_str));
    fuchsia_free_log_messages(log_messages);
    return fpromise::error(copied_error_str);
  }
  output.reserve(log_messages.messages.len + output.size());
  for (size_t i = 0; i < log_messages.messages.len; i++) {
    auto msg = log_messages.messages.ptr[i];
    output.emplace_back(log_decoder::ToFidlLogMessage(*msg));
  }
  fuchsia_free_log_messages(log_messages);
  return fpromise::ok();
}

fpromise::result<void, std::string> ConvertFormattedContentToLogMessagesInternal(
    FormattedContent content, MessageParser* parser,
    std::vector<fpromise::result<fuchsia::logger::LogMessage, std::string>>& output) {
  if (content.is_fxt()) {
    uint64_t size = 0;
    zx_status_t status = content.fxt().get_prop_content_size(&size);
    if (status != ZX_OK) {
      return fpromise::error("Failed to get content size from FXT VMO: " + std::to_string(status));
    }
    std::unique_ptr<unsigned char[]> data = std::make_unique<unsigned char[]>(size);
    status = content.fxt().read(data.get(), 0, size);
    if (status != ZX_OK) {
      return fpromise::error("Failed to read from FXT VMO: " + std::to_string(status));
    }
    // Note: If |parser| is non-null, |expect_extended_attribution| is ignored.
    // If |parser| is null, |expect_extended_attribution| is used to determine
    // whether extended attribution is expected.
    return ConvertFormattedFxtToLogMessagesInternal(std::span<const uint8_t>(data.get(), size),
                                                    true, parser, output);
  } else if (content.is_json()) {
    std::string data;
    if (!fsl::StringFromVmo(content.json(), &data)) {
      return fpromise::error("Failed to read string from VMO");
    }
    content.json().vmo.reset();

    rapidjson::Document d;
    d.Parse(std::move(data));
    if (d.HasParseError()) {
      std::string error = "Failed to parse content as JSON. Offset " +
                          std::to_string(d.GetErrorOffset()) + ": " +
                          rapidjson::GetParseError_En(d.GetParseError());
      return fpromise::error(std::move(error));
    }

    if (!d.IsArray()) {
      return fpromise::error("Expected content to contain an array");
    }

    for (rapidjson::SizeType i = 0; i < d.Size(); ++i) {
      output.emplace_back(JsonToLogMessage(d[i]));
    }

    return fpromise::ok();
  } else {
    // Expecting JSON or FXT in all cases.
    return fpromise::error("Expected json or FXT content");
  }
}

MessageParser* CreateMessageParserForFormat(fuchsia::diagnostics::Format format) {
  switch (format) {
    case fuchsia::diagnostics::Format::JSON:
    case fuchsia::diagnostics::Format::TEXT:
    case fuchsia::diagnostics::Format::CBOR:
    case fuchsia::diagnostics::Format::LEGACY_FXT:
      return nullptr;
    case fuchsia::diagnostics::Format::FXT:
      return fuchsia_new_message_parser();
  }
}
}  // namespace

void MessageParserDeleter::operator()(MessageParser* parser) const {
  if (parser) {
    fuchsia_free_message_parser(parser);
  }
}

LogBatchIterator::LogBatchIterator(fuchsia::diagnostics::BatchIteratorPtr iterator,
                                   fuchsia::diagnostics::Format format)
    : iterator_(std::move(iterator)), parser_(CreateMessageParserForFormat(format)) {}

void LogBatchIterator::GetNext(GetNextCallback callback) {
  iterator_->GetNext([this, callback = std::move(callback)](auto result) mutable {
    if (result.is_err()) {
      callback(fpromise::error("BatchIterator error: " +
                               std::to_string(static_cast<int32_t>(result.err()))));
      return;
    }

    std::vector<fpromise::result<fuchsia::logger::LogMessage, std::string>> all_messages;
    for (auto& content : result.response().batch) {
      auto formatted = ConvertFormattedContentToLogMessagesInternal(std::move(content),
                                                                    parser_.get(), all_messages);
      if (formatted.is_error()) {
        callback(fpromise::error(formatted.error()));
        return;
      }
    }
    callback(fpromise::ok(std::move(all_messages)));
  });
}

fpromise::result<std::vector<fpromise::result<fuchsia::logger::LogMessage, std::string>>,
                 std::string>
ConvertFormattedFxtToLogMessages(std::span<const uint8_t> data, bool expect_extended_attribution) {
  std::vector<fpromise::result<fuchsia::logger::LogMessage, std::string>> messages;
  auto ret = ConvertFormattedFxtToLogMessagesInternal(data, expect_extended_attribution, nullptr,
                                                      messages);
  if (ret.is_error()) {
    return fpromise::error(ret.take_error());
  }
  return fpromise::ok(std::move(messages));
}

fpromise::result<std::vector<fpromise::result<fuchsia::logger::LogMessage, std::string>>,
                 std::string>
ConvertFormattedContentToLogMessages(FormattedContent content) {
  std::vector<fpromise::result<fuchsia::logger::LogMessage, std::string>> messages;
  auto ret = ConvertFormattedContentToLogMessagesInternal(std::move(content), nullptr, messages);
  if (ret.is_error()) {
    return fpromise::error(ret.take_error());
  }
  return fpromise::ok(std::move(messages));
}

fuchsia_logging::RawLogSeverity GetSeverityFromVerbosity(uint8_t verbosity) {
  // Clamp verbosity scale to the interstitial space between INFO and DEBUG
  uint8_t max_verbosity =
      (fuchsia_logging::LogSeverity::Info - fuchsia_logging::LogSeverity::Debug);
  if (verbosity > max_verbosity) {
    verbosity = max_verbosity;
  }
  int severity = fuchsia_logging::LogSeverity::Info - verbosity;
  if (severity < fuchsia_logging::LogSeverity::Debug + 1) {
    return fuchsia_logging::LogSeverity::Debug + 1;
  }
  return static_cast<fuchsia_logging::RawLogSeverity>(severity);
}

}  // namespace diagnostics::accessor2logger
