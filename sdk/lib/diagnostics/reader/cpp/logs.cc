// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <fidl/fuchsia.diagnostics/cpp/natural_types.h>
#include <lib/diagnostics/reader/cpp/constants.h>
#include <lib/diagnostics/reader/cpp/logs.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>

#include <src/lib/fsl/vmo/strings.h>

namespace diagnostics::reader {

namespace {

inline fuchsia_diagnostics_types::Severity StringToSeverity(const std::string& input) {
  if (strcasecmp(input.c_str(), "trace") == 0) {
    return fuchsia_diagnostics_types::Severity::kTrace;
  }
  if (strcasecmp(input.c_str(), "debug") == 0) {
    return fuchsia_diagnostics_types::Severity::kDebug;
  }
  if (strcasecmp(input.c_str(), "info") == 0) {
    return fuchsia_diagnostics_types::Severity::kInfo;
  }
  if (strcasecmp(input.c_str(), "warn") == 0) {
    return fuchsia_diagnostics_types::Severity::kWarn;
  }
  if (strcasecmp(input.c_str(), "error") == 0) {
    return fuchsia_diagnostics_types::Severity::kError;
  }
  if (strcasecmp(input.c_str(), "fatal") == 0) {
    return fuchsia_diagnostics_types::Severity::kFatal;
  }
  // We must never get here as long as we are reading data from Archivist.
  return fuchsia_diagnostics_types::Severity::kInfo;
}

}  // namespace

LogsSubscription::LogsSubscription(fidl::SharedClient<fuchsia_diagnostics::BatchIterator> iterator,
                                   async::Executor& executor)
    : iterator_(std::move(iterator)),
      pending_(std::make_shared<std::deque<LogsData>>()),
      done_(std::make_shared<bool>(false)) {}

bool LogsSubscription::Done() { return *done_; }

LogsSubscription::Promise LogsSubscription::Next() {
  return fpromise::make_promise([this] { return ReadBatch(); }).wrap_with(scope_);
}

LogsSubscription::Promise LogsSubscription::ReadBatch() {
  fpromise::bridge<std::optional<LogsData>, std::string> bridge;
  if (!pending_->empty()) {
    auto result = std::make_optional(std::move(pending_->front()));
    pending_->pop_front();
    return fpromise::make_result_promise<std::optional<LogsData>, std::string>(
        fpromise::ok(std::move(result)));
  } else if (*done_) {
    return fpromise::make_result_promise<std::optional<LogsData>, std::string>(
        fpromise::ok(std::nullopt));
  }
  iterator_->GetNext().Then([completer = std::move(bridge.completer), done = done_,
                             pending = pending_](auto& result) mutable {
    if (result.is_error()) {
      completer.complete_error("Batch iterator returned error: " +
                               result.error_value().FormatDescription());
      return;
    }

    if (result->batch().empty()) {
      *done = true;
      completer.complete_ok(std::nullopt);
      return;
    }

    for (auto& content : result->batch()) {
      if (content.Which() != fuchsia_diagnostics::FormattedContent::Tag::kJson) {
        completer.complete_error("Received an unexpected content format");
        return;
      }
      std::string json;
      fsl::SizedVmo vmo(std::move(content.json()->vmo()), content.json()->size());
      if (!fsl::StringFromVmo(vmo, &json)) {
        completer.complete_error("Failed to read returned VMO");
        return;
      }
      rapidjson::Document document;
      document.Parse(json);
      auto maybe_json = LoadJson(std::move(document), pending, done);
      if (maybe_json) {
        pending->push_front(std::move(*maybe_json));
      }
    }
    if (!pending->empty()) {
      auto result = std::make_optional(std::move(pending->front()));
      pending->pop_front();
      completer.complete_ok(std::move(result));
    }
  });
  return bridge.consumer.promise_or(fpromise::error("Failed to obtain consumer promise"));
}

std::optional<LogsData> LogsSubscription::LoadJson(rapidjson::Document document,
                                                   std::shared_ptr<std::deque<LogsData>> pending,
                                                   std::shared_ptr<bool> done) {
  if (document.IsArray()) {
    for (auto& value : document.GetArray()) {
      // We need to ensure that the value is safely moved between documents, which may involve
      // copying.
      //
      // It is an error to maintain a reference to a Value in a Document after that Document is
      // destroyed, and the input |document| is destroyed immediately after this branch.
      rapidjson::Document value_document;
      rapidjson::Value temp(value.Move(), value_document.GetAllocator());
      value_document.Swap(temp);
      pending->push_back(LogsData(std::move(value_document)));
    }
  } else {
    pending->push_back(LogsData(std::move(document)));
  }
  if (pending->empty()) {
    return std::nullopt;
  }
  auto result = std::make_optional(std::move(pending->front()));
  pending->pop_front();
  return result;
}

LogsData::LogsData(rapidjson::Document document) {
  if (document.HasMember(kMonikerName) && document[kMonikerName].IsString()) {
    std::string val = document[kMonikerName].GetString();
    moniker_ = document[kMonikerName].GetString();
  }
  if (document.HasMember(kVersionName) && document[kVersionName].IsNumber()) {
    version_ = document[kVersionName].GetInt64();
  } else {
    version_ = 0;
  }

  if (document.HasMember(kMetadataName) && document[kMetadataName].IsObject()) {
    const auto& metadata = document[kMetadataName].GetObject();
    if (metadata.HasMember(kMetadataComponentURL) && metadata[kMetadataComponentURL].IsString()) {
      metadata_.component_url = metadata[kMetadataComponentURL].GetString();
    }

    if (metadata.HasMember(kMetadataTimestamp) && metadata[kMetadataTimestamp].IsUint64()) {
      metadata_.timestamp = metadata[kMetadataTimestamp].GetUint64();
    }

    if (metadata.HasMember(kMetadataSeverity) && metadata[kMetadataSeverity].IsString()) {
      metadata_.severity = StringToSeverity(metadata[kMetadataSeverity].GetString());
    }

    if (metadata.HasMember(kMetadataTags) && metadata[kMetadataTags].IsArray()) {
      const auto& tags = metadata[kMetadataTags].GetArray();
      for (auto tag = tags.Begin(); tag != tags.End(); ++tag) {
        if (tag->IsString()) {
          metadata_.tags.push_back(tag->GetString());
        }
      }
    }

    if (metadata.HasMember(kMetadataPid) && metadata[kMetadataPid].IsUint64()) {
      metadata_.pid = std::make_optional(metadata[kMetadataPid].GetUint64());
    }

    if (metadata.HasMember(kMetadataTid) && metadata[kMetadataTid].IsUint64()) {
      metadata_.tid = std::make_optional(metadata[kMetadataTid].GetUint64());
    }

    if (metadata.HasMember(kMetadataFile) && metadata[kMetadataFile].IsString()) {
      metadata_.file = std::make_optional(metadata[kMetadataFile].GetString());
    }

    if (metadata.HasMember(kMetadataLine) && metadata[kMetadataLine].IsUint64()) {
      metadata_.line = std::make_optional(metadata[kMetadataLine].GetUint64());
    }

    if (metadata.HasMember(kMetadataErrors) && metadata[kMetadataErrors].IsArray()) {
      const auto& errors = metadata[kMetadataErrors].GetArray();
      for (auto item = errors.Begin(); item != errors.End(); ++item) {
        if (!item->IsObject()) {
          continue;
        }
        const auto& error = item->GetObject();
        if (error.HasMember(kErrorDroppedLogs)) {
          const auto& dropped = error[kErrorDroppedLogs];
          if (dropped.IsObject() && dropped.HasMember(kCount) && dropped[kCount].IsUint64()) {
            LogsData::Error error{DroppedLogsError{dropped[kCount].GetUint64()}};
            metadata_.errors.push_back(std::move(error));
          }
        } else if (error.HasMember(kErrorRolledOutLogs)) {
          const auto& rolled_out = error[kErrorRolledOutLogs];
          if (rolled_out.IsObject() && rolled_out.HasMember(kCount) &&
              rolled_out[kCount].IsUint64()) {
            LogsData::Error error{RolledOutLogsError{rolled_out[kCount].GetUint64()}};
            metadata_.errors.push_back(std::move(error));
          }
        } else if (error.HasMember(kErrorParseRecord)) {
          const auto& failed_to_parse_record = error[kErrorParseRecord];
          if (failed_to_parse_record.IsString()) {
            LogsData::Error error{FailedToParseRecordError{failed_to_parse_record.GetString()}};
            metadata_.errors.push_back(std::move(error));
          }
        } else if (error.HasMember(kErrorOther)) {
          const auto& other = error[kErrorOther];
          if (other.IsObject() && other.HasMember(kMessage) && other[kMessage].IsString()) {
            LogsData::Error error{OtherError{other[kMessage].GetString()}};
            metadata_.errors.push_back(std::move(error));
          }
        }
      }
    }
  }

  if (document.HasMember(kPayloadName) && document[kPayloadName].IsObject()) {
    const auto& payload = document[kPayloadName].GetObject();
    if (payload.HasMember(kPayloadRoot) && payload[kPayloadRoot].IsObject()) {
      const auto& root = payload[kPayloadRoot].GetObject();
      if (root.HasMember(kMessage) && root[kMessage].IsObject()) {
        const auto& message = root[kMessage].GetObject();
        if (message.HasMember(kPayloadMessageValue) && message[kPayloadMessageValue].IsString()) {
          message_ = message[kPayloadMessageValue].GetString();
        }
      }

      if (root.HasMember(kPayloadKeys) && root[kPayloadKeys].IsObject()) {
        const auto& keys = root[kPayloadKeys].GetObject();
        for (auto it = keys.MemberBegin(); it != keys.MemberEnd(); ++it) {
          auto name = it->name.GetString();
          switch (it->value.GetType()) {
            case rapidjson::kNullType:
              break;
            case rapidjson::kFalseType:
            case rapidjson::kTrueType:
              keys_.emplace_back(
                  inspect::PropertyValue(name, inspect::BoolPropertyValue(it->value.GetBool())));
              break;
            case rapidjson::kStringType:
              keys_.emplace_back(inspect::PropertyValue(
                  name, inspect::StringPropertyValue(it->value.GetString())));
              break;
            case rapidjson::kNumberType:
              if (it->value.IsInt64()) {
                keys_.emplace_back(
                    inspect::PropertyValue(name, inspect::IntPropertyValue(it->value.GetInt64())));
              } else if (it->value.IsUint64()) {
                keys_.emplace_back(inspect::PropertyValue(
                    name, inspect::UintPropertyValue(it->value.GetUint64())));
              } else {
                keys_.emplace_back(inspect::PropertyValue(
                    name, inspect::DoublePropertyValue(it->value.GetDouble())));
              }
              break;
            case rapidjson::kArrayType:
              LoadArray(name, it->value.GetArray());
              break;
            default:
              break;
          }
        }
      }
    }
  }
}

void LogsData::LoadArray(const std::string& name, const rapidjson::Value::Array& arr) {
  if (arr.Empty()) {
    keys_.emplace_back(inspect::PropertyValue(
        name, inspect::IntArrayValue(std::vector<int64_t>{}, inspect::ArrayDisplayFormat::kFlat)));
    return;
  }

  switch (arr.Begin()->GetType()) {
    case rapidjson::kStringType: {
      std::vector<std::string> values;
      for (auto& v : arr) {
        values.emplace_back(v.GetString());
      }
      keys_.emplace_back(inspect::PropertyValue(
          name, inspect::StringArrayValue(std::move(values), inspect::ArrayDisplayFormat::kFlat)));
      break;
    }
    case rapidjson::kNumberType: {
      if (arr.Begin()->IsInt64()) {
        std::vector<std::int64_t> values;
        for (auto& v : arr) {
          values.emplace_back(v.GetInt64());
        }
        keys_.emplace_back(inspect::PropertyValue(
            name, inspect::IntArrayValue(std::move(values), inspect::ArrayDisplayFormat::kFlat)));
      } else if (arr.Begin()->IsUint64()) {
        std::vector<std::uint64_t> values;
        for (auto& v : arr) {
          values.emplace_back(v.GetUint64());
        }
        keys_.emplace_back(inspect::PropertyValue(
            name, inspect::UintArrayValue(std::move(values), inspect::ArrayDisplayFormat::kFlat)));
      } else if (arr.Begin()->IsDouble()) {
        std::vector<double> values;
        for (auto& v : arr) {
          values.emplace_back(v.GetDouble());
        }
        keys_.emplace_back(inspect::PropertyValue(
            name,
            inspect::DoubleArrayValue(std::move(values), inspect::ArrayDisplayFormat::kFlat)));
      }
      break;
    }
    default:
      break;
  }
}
}  // namespace diagnostics::reader
