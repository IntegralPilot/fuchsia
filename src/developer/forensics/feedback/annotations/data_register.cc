// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/data_register.h"

#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>

#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "third_party/rapidjson/include/rapidjson/document.h"
#include "third_party/rapidjson/include/rapidjson/error/en.h"
#include "third_party/rapidjson/include/rapidjson/prettywriter.h"
#include "third_party/rapidjson/include/rapidjson/stringbuffer.h"

namespace forensics::feedback {
namespace {

const char kDefaultNamespace[] = "misc";

const char kNamespaceSeparator[] = ".";

Annotations Flatten(const std::map<std::string, Annotations>& namespaced_annotations) {
  Annotations flat_annotations;
  for (const auto& [ns, annotations] : namespaced_annotations) {
    for (const auto& [k, v] : annotations) {
      flat_annotations.insert({ns + kNamespaceSeparator + k, v});
    }
  }

  return flat_annotations;
}

}  // namespace

DataRegister::DataRegister(size_t max_num_annotations,
                           std::set<std::string> disallowed_annotation_namespaces,
                           std::string register_filepath)
    : max_num_annotations_(max_num_annotations),
      disallowed_annotation_namespaces_(std::move(disallowed_annotation_namespaces)),
      register_filepath_(std::move(register_filepath)) {
  RestoreFromJson();
}

void DataRegister::Upsert(fuchsia::feedback::ComponentData data, UpsertCallback callback) {
  auto execute_callback = ::fit::defer(std::move(callback));

  if (!data.has_annotations()) {
    FX_LOGS(WARNING) << "No non-platform annotations to upsert";
    return;
  }

  if (data.has_namespace() && disallowed_annotation_namespaces_.contains(data.namespace_())) {
    FX_LOGS(WARNING) << fxl::StringPrintf(
        "Ignoring non-platform annotations, %s is a reserved namespace", data.namespace_().c_str());

    // TODO(https://fxbug.dev/42125546): close connection with ZX_ERR_INVALID_ARGS instead.
    return;
  }

  if (!data.has_namespace()) {
    FX_LOGS(WARNING) << "No namespace specified, defaulting to " << kDefaultNamespace;
  }
  const std::string ns = (data.has_namespace()) ? data.namespace_() : kDefaultNamespace;

  Annotations new_annotations = namespaced_annotations_[ns];
  for (const auto& [key, value] : data.annotations()) {
    new_annotations.insert_or_assign(key, ErrorOrString(value));
  }

  size_t new_size = new_annotations.size();
  for (const auto& [n, annotations] : namespaced_annotations_) {
    if (n != ns) {
      new_size += annotations.size();
    }
  }

  if (new_size > max_num_annotations_) {
    FX_LOGS(WARNING) << fxl::StringPrintf(
        "Ignoring all %lu new non-platform annotations as only %lu non-platform annotations "
        "are allowed",
        new_size, max_num_annotations_);
    is_missing_annotations_ = true;

    // TODO(https://fxbug.dev/42125548): close all connections.
    return;
  }

  namespaced_annotations_[ns] = std::move(new_annotations);

  UpdateJson();
}

Annotations DataRegister::Get() { return Flatten(namespaced_annotations_); }

// The content of the data register will be stored as json where each namespace is comprised of an
// object made up of string-string pairs.
//
// For example, if there are 2 namespaces, "foo" and "bar". "foo" has 2 set of annotations,
// {"k1", "v1} and {"k2, "v2"}, and "bar" has 1 annotation, {"k3", "v3"}, the json will look like:
// {
//     "foo": {
//         "k1": "v1",
//         "k2": "v2"
//     },
//     "bar": {
//         "k3": "v3"
//     }
// }
void DataRegister::UpdateJson() {
  using namespace rapidjson;

  Document register_json;
  register_json.SetObject();
  auto& allocator = register_json.GetAllocator();

  for (const auto& [ns, annotations] : namespaced_annotations_) {
    register_json.AddMember(Value(ns, allocator), Value(rapidjson::kObjectType), allocator);

    auto json_annotations = register_json[ns].GetObject();
    for (const auto& [k, v] : annotations) {
      auto key = Value(k, allocator);
      auto val = Value(v.Value(), allocator);
      json_annotations.AddMember(key, val, allocator);
    }
  }

  StringBuffer buffer;
  PrettyWriter<StringBuffer> writer(buffer);

  register_json.Accept(writer);

  if (!files::WriteFile(register_filepath_, buffer.GetString(), buffer.GetLength())) {
    FX_LOGS(ERROR) << "Failed to write data register contents to " << register_filepath_;
  }
}

void DataRegister::RestoreFromJson() {
  using namespace rapidjson;

  // If the file doesn't exit, return.
  if (!files::IsFile(register_filepath_)) {
    return;
  }

  // Check-fail if the file can't be read.
  std::string json;
  FX_CHECK(files::ReadFileToString(register_filepath_, &json));

  Document register_json;
  ParseResult ok = register_json.Parse(json);
  if (!ok) {
    FX_LOGS(ERROR) << "Error parsing data register as JSON at offset " << ok.Offset() << " "
                   << GetParseError_En(ok.Code());
    files::DeletePath(register_filepath_, /*recursive=*/true);
    return;
  }

  // Each namespace in the register is represented by an object containing at string-string pairs
  // that are the annotations.
  FX_CHECK(register_json.IsObject());
  for (const auto& member : register_json.GetObject()) {
    // Skip any non-object members.
    if (!member.value.IsObject()) {
      continue;
    }

    const std::string _namespace = member.name.GetString();
    for (const auto& annotation : member.value.GetObject()) {
      // Annotations must be string pairs.
      if (!annotation.value.IsString()) {
        continue;
      }

      const std::string key = annotation.name.GetString();
      const std::string value = annotation.value.GetString();
      namespaced_annotations_[_namespace].emplace(key, value);
    }
  }
}

}  // namespace forensics::feedback
