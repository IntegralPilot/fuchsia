// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/process_explorer/task_hierarchy_data.h"

#include <unordered_map>

#include "third_party/rapidjson/include/rapidjson/document.h"
#include "third_party/rapidjson/include/rapidjson/rapidjson.h"
#include "third_party/rapidjson/include/rapidjson/stringbuffer.h"
#include "third_party/rapidjson/include/rapidjson/writer.h"

namespace process_explorer {

std::string WriteTaskHierarchyDataAsJson(std::vector<Task> tasks_data) {
  rapidjson::Document json_document;
  json_document.SetObject();
  auto& allocator = json_document.GetAllocator();

  rapidjson::Value tasks_json(rapidjson::kArrayType);
  tasks_json.Reserve(static_cast<unsigned int>(tasks_data.size()), allocator);

  auto get_task_type = [&](TaskType type) -> rapidjson::Value {
    switch (type) {
      case TaskType::Job:
        return rapidjson::Value("job", allocator);
      case TaskType::Process:
        return rapidjson::Value("process", allocator);
      case TaskType::Thread:
        return rapidjson::Value("thread", allocator);
    }
  };

  for (const auto& task : tasks_data) {
    rapidjson::Value task_name(rapidjson::kObjectType);
    const std::string s(task.name);
    task_name.SetString(s.c_str(), static_cast<rapidjson::SizeType>(s.length()), allocator);

    rapidjson::Value task_json(rapidjson::kObjectType);
    task_json.AddMember("depth", task.depth, allocator)
        .AddMember("koid", task.koid, allocator)
        .AddMember("parent_koid", task.parent_koid, allocator)
        .AddMember("type", get_task_type(task.type), allocator)
        .AddMember("name", task_name, allocator);
    tasks_json.PushBack(task_json, allocator);
  }

  json_document.AddMember("Tasks", tasks_json, allocator);

  rapidjson::StringBuffer buffer;
  buffer.Clear();
  rapidjson::Writer<rapidjson::StringBuffer> writer(buffer);
  json_document.Accept(writer);

  return std::string(buffer.GetString(), buffer.GetSize());
}

}  // namespace process_explorer
