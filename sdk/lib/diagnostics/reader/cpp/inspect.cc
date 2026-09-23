// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/diagnostics/reader/cpp/archive_reader.h>
#include <lib/diagnostics/reader/cpp/constants.h>
#include <lib/diagnostics/reader/cpp/inspect.h>

#include <stack>

#include <rapidjson/document.h>
#include <rapidjson/prettywriter.h>
#include <rapidjson/rapidjson.h>
#include <rapidjson/stringbuffer.h>

namespace {

bool AllDigits(const std::string& value) {
  for (char c : value) {
    if (!std::isdigit(c)) {
      return false;
    }
  }
  return !value.empty();
}

void SortObject(rapidjson::Value* object) {
  std::sort(object->MemberBegin(), object->MemberEnd(),
            [](const rapidjson::Value::Member& lhs, const rapidjson::Value::Member& rhs) {
              auto lhs_name = lhs.name.GetString();
              auto rhs_name = rhs.name.GetString();
              if (AllDigits(lhs_name) && AllDigits(rhs_name)) {
                uint64_t lhs_val = atoll(lhs_name);
                uint64_t rhs_val = atoll(rhs_name);
                return lhs_val < rhs_val;
              } else {
                return strcmp(lhs_name, rhs_name) < 0;
              }
            });
}

struct StackItem {
  inspect::Hierarchy hierarchy;
  rapidjson::Value::MemberIterator next;
  rapidjson::Value::MemberIterator end;
};

void ParseArray(const std::string& name, const rapidjson::Value::Array& arr,
                inspect::NodeValue* node_ptr) {
  if (arr.Empty()) {
    node_ptr->add_property(inspect::PropertyValue(
        name, inspect::IntArrayValue(std::vector<int64_t>{}, inspect::ArrayDisplayFormat::kFlat)));
    return;
  }

  switch (arr.Begin()->GetType()) {
    case rapidjson::kStringType: {
      std::vector<std::string> values;
      for (auto& v : arr) {
        values.emplace_back(v.GetString());
      }
      node_ptr->add_property(inspect::PropertyValue(
          name, inspect::StringArrayValue(std::move(values), inspect::ArrayDisplayFormat::kFlat)));
      break;
    }
    case rapidjson::kNumberType: {
      if (arr.Begin()->IsInt64()) {
        std::vector<std::int64_t> values;
        for (auto& v : arr) {
          values.emplace_back(v.GetInt64());
        }
        node_ptr->add_property(inspect::PropertyValue(
            name, inspect::IntArrayValue(std::move(values), inspect::ArrayDisplayFormat::kFlat)));
      } else if (arr.Begin()->IsUint64()) {
        std::vector<std::uint64_t> values;
        for (auto& v : arr) {
          values.emplace_back(v.GetUint64());
        }
        node_ptr->add_property(inspect::PropertyValue(
            name, inspect::UintArrayValue(std::move(values), inspect::ArrayDisplayFormat::kFlat)));
      } else if (arr.Begin()->IsDouble()) {
        std::vector<double> values;
        for (auto& v : arr) {
          values.emplace_back(v.GetDouble());
        }
        node_ptr->add_property(inspect::PropertyValue(
            name,
            inspect::DoubleArrayValue(std::move(values), inspect::ArrayDisplayFormat::kFlat)));
      }
      break;
    }
    default:
      break;
  }
}

inspect::Hierarchy ParsePayload(std::string name, const rapidjson::Value::Object& obj) {
  std::stack<StackItem> pending;
  inspect::Hierarchy root, completedHierarchy;
  root.node_ptr()->set_name(std::move(name));
  pending.push({std::move(root), obj.MemberBegin(), obj.MemberEnd()});

  while (!pending.empty()) {
    auto foundObj = false;
    auto& curr = pending.top();
    for (auto& itr = curr.next; itr != curr.end; itr++) {
      auto valueName = itr->name.GetString();

      if (itr->value.IsObject()) {
        inspect::Hierarchy child;
        child.node_ptr()->set_name(valueName);
        pending.push({std::move(child), itr->value.GetObject().MemberBegin(),
                      itr->value.GetObject().MemberEnd()});
        itr++;
        foundObj = true;
        // We first need to take care of the child before we can process the remaining entries
        break;
      }

      // If it's not an object it's a property
      switch (itr->value.GetType()) {
        case rapidjson::kNullType:
          break;
        case rapidjson::kFalseType:
        case rapidjson::kTrueType:
          itr->value.GetBool();
          curr.hierarchy.node_ptr()->add_property(
              inspect::PropertyValue(valueName, inspect::BoolPropertyValue(itr->value.GetBool())));
          break;
        case rapidjson::kStringType:
          curr.hierarchy.node_ptr()->add_property(inspect::PropertyValue(
              valueName, inspect::StringPropertyValue(itr->value.GetString())));
          break;
        case rapidjson::kNumberType:
          if (itr->value.IsInt64()) {
            curr.hierarchy.node_ptr()->add_property(inspect::PropertyValue(
                valueName, inspect::IntPropertyValue(itr->value.GetInt64())));
          } else if (itr->value.IsUint64()) {
            curr.hierarchy.node_ptr()->add_property(inspect::PropertyValue(
                valueName, inspect::UintPropertyValue(itr->value.GetUint64())));
          } else {
            curr.hierarchy.node_ptr()->add_property(inspect::PropertyValue(
                valueName, inspect::DoublePropertyValue(itr->value.GetDouble())));
          }
          break;
        case rapidjson::kArrayType:
          ParseArray(valueName, itr->value.GetArray(), curr.hierarchy.node_ptr());
          break;
        default:
          break;
      }
    }
    if (!foundObj) {
      completedHierarchy = std::move(curr.hierarchy);
      pending.pop();
      if (!pending.empty()) {
        pending.top().hierarchy.add_child(std::move(completedHierarchy));
      }
    }
  }
  return completedHierarchy;
}

}  // namespace

namespace diagnostics::reader {

InspectData::InspectData(rapidjson::Document document) : document_(std::move(document)) {
  if (document_.HasMember(kMonikerName) && document_[kMonikerName].IsString()) {
    std::string val = document_[kMonikerName].GetString();
    moniker_ = document_[kMonikerName].GetString();
  }
  if (document_.HasMember(kVersionName) && document_[kVersionName].IsNumber()) {
    version_ = document_[kVersionName].GetInt64();
  } else {
    version_ = 0;
  }

  metadata_.timestamp = 0;
  metadata_.filename = {};
  metadata_.name = {};
  metadata_.component_url = {};
  metadata_.errors = {};

  if (document_.HasMember(kMetadataName) && document_[kMetadataName].IsObject()) {
    const auto& metadata = document_[kMetadataName].GetObject();
    if (metadata.HasMember(kMetadataFilename) && metadata[kMetadataFilename].IsString()) {
      metadata_.filename = metadata[kMetadataFilename].GetString();
    } else if (metadata.HasMember(kMetadataNameOfInspectTree) &&
               metadata[kMetadataNameOfInspectTree].IsString()) {
      metadata_.name = metadata[kMetadataNameOfInspectTree].GetString();
    }
    if (metadata.HasMember(kMetadataComponentURL) && metadata[kMetadataComponentURL].IsString()) {
      metadata_.component_url = metadata[kMetadataComponentURL].GetString();
    }
    if (metadata.HasMember(kMetadataTimestamp) && metadata[kMetadataTimestamp].IsNumber()) {
      metadata_.timestamp = metadata[kMetadataTimestamp].GetUint64();
    }
    if (metadata.HasMember(kMetadataErrors) && metadata[kMetadataErrors].IsArray()) {
      const auto& errors = metadata[kMetadataErrors].GetArray();
      std::vector<InspectError> inspectErrors;
      for (auto error = errors.Begin(); error != errors.End(); error++) {
        if (error->IsObject() && error->GetObject()[kMetadataErrorsMessage].IsString()) {
          InspectError ie = {error->GetObject()[kMetadataErrorsMessage].GetString()};
          inspectErrors.emplace_back(ie);
        }
      }
      metadata_.errors = std::make_optional(inspectErrors);
    }
  }
  if (document_.HasMember(kPayloadName) && document_[kPayloadName].IsObject()) {
    const auto& payload = document_[kPayloadName].GetObject();
    if (payload.MemberBegin() != payload.MemberEnd() && payload.MemberBegin()->value.IsObject()) {
      payload_ = ParsePayload(payload.MemberBegin()->name.GetString(),
                              payload.MemberBegin()->value.GetObject());
    }
  }
}

const rapidjson::Value& InspectData::content() const {
  static rapidjson::Value default_ret;

  if (!document_.IsObject() || !document_.HasMember(kPayloadName)) {
    return default_ret;
  }

  return document_[kPayloadName];
}

const rapidjson::Value& InspectData::GetByPath(const std::vector<std::string>& path) const {
  static rapidjson::Value default_ret;

  const rapidjson::Value* cur = &content();
  for (size_t i = 0; i < path.size(); i++) {
    if (!cur->IsObject()) {
      return default_ret;
    }

    auto it = cur->FindMember(path[i]);
    if (it == cur->MemberEnd()) {
      return default_ret;
    }

    cur = &it->value;
  }

  return *cur;
}

std::string InspectData::PrettyJson() {
  rapidjson::StringBuffer buffer;
  rapidjson::PrettyWriter<rapidjson::StringBuffer> writer(buffer);
  document_.Accept(writer);
  return buffer.GetString();
}

void InspectData::Sort() {
  std::vector<rapidjson::Value*> pending;
  auto& object = document_;
  pending.push_back(&object);
  while (!pending.empty()) {
    rapidjson::Value* pending_object = pending.back();
    pending.pop_back();
    SortObject(pending_object);
    for (auto m = pending_object->MemberBegin(); m != pending_object->MemberEnd(); ++m) {
      if (m->value.IsObject()) {
        pending.push_back(&m->value);
      }
    }
  }
}

}  // namespace diagnostics::reader
