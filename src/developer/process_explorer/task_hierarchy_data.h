// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_PROCESS_EXPLORER_TASK_HIERARCHY_DATA_H_
#define SRC_DEVELOPER_PROCESS_EXPLORER_TASK_HIERARCHY_DATA_H_

#include <zircon/types.h>

#include <string>
#include <vector>

namespace process_explorer {

enum class TaskType : uint8_t {
  Job,
  Process,
  Thread,
};

// Information about a task
struct Task {
  int depth;
  zx_koid_t koid;
  zx_koid_t parent_koid;
  TaskType type;
  std::string name;
};

/* Returns the task information vector as a JSON string. In this format:
    {
        "Tasks":[
            {
                "depth":2,
                "koid":1097,
                "parent_koid":781,
                "type":"process",
                "name":"bin/component_manager"
            },
            ...
        ]
    }
*/
std::string WriteTaskHierarchyDataAsJson(std::vector<Task> tasks_data);

}  // namespace process_explorer

#endif  // SRC_DEVELOPER_PROCESS_EXPLORER_TASK_HIERARCHY_DATA_H_
