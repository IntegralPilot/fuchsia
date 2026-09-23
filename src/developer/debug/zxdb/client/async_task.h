// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ASYNC_TASK_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ASYNC_TASK_H_

#include <stdint.h>

#include <string>
#include <vector>

#include "src/developer/debug/zxdb/client/client_object.h"
#include "src/developer/debug/zxdb/expr/expr_value.h"
#include "src/developer/debug/zxdb/symbols/identifier.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/lib/fxl/macros.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {

// Abstractly represents a single asynchronous task (e.g. a Rust future).
class AsyncTask : public ClientObject {
 public:
  explicit AsyncTask(Session* session);
  virtual ~AsyncTask();

  fxl::WeakPtr<AsyncTask> GetWeakPtr();

  // Returns a unique identifier for this task if available, or 0.
  virtual uint64_t GetId() const = 0;

  enum class Type {
    kTask,
    kFuture,
    kScope,
    kFunction,
    kOther,
  };
  virtual Type GetType() const = 0;

  // Returns the location where this task was defined or is currently executing.
  virtual const Location& GetLocation() const = 0;

  // Returns the identifier of this asynchronous task. This can be anything from a function name or
  // a type name, like JoinHandles, Timers, Scopes, or literal Task objects in the respective
  // runtime.
  virtual const Identifier& GetIdentifier() const = 0;

  // Returns a string representation of the task's current state (e.g. "Pending", "Ready").
  virtual std::string GetState() const = 0;

  struct NamedValue {
    std::optional<std::string> name;
    ExprValue value;
  };
  // Returns all variables that are associated with this task, these can be lambda captures, state
  // variables, or anything else that the runtime decided to include in the task object.
  virtual const std::vector<NamedValue>& GetValues() const = 0;

  // Returns the children of this task (tasks that this task is awaiting).
  using Ref = std::reference_wrapper<const AsyncTask>;
  virtual std::vector<Ref> GetChildren() const = 0;

 private:
  fxl::WeakPtrFactory<AsyncTask> weak_factory_;

  FXL_DISALLOW_COPY_AND_ASSIGN(AsyncTask);
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ASYNC_TASK_H_
