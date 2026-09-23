// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/format_async_task.h"

#include <utility>

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/async_task.h"
#include "src/developer/debug/zxdb/client/async_task_tree.h"
#include "src/developer/debug/zxdb/common/test_with_loop.h"
#include "src/developer/debug/zxdb/expr/mock_eval_context.h"
#include "src/developer/debug/zxdb/format/async_output_buffer_test_util.h"
#include "src/developer/debug/zxdb/symbols/identifier.h"
#include "src/developer/debug/zxdb/symbols/location.h"

namespace zxdb {

namespace {

const std::string kAwaiteeMarker = "└─ ";

class MockAsyncTask : public AsyncTask {
 public:
  MockAsyncTask(uint64_t id, Type type, Identifier identifier, std::string state)
      : AsyncTask(nullptr),
        id_(id),
        type_(type),
        identifier_(std::move(identifier)),
        state_(std::move(state)) {}

  uint64_t GetId() const override { return id_; }
  Type GetType() const override { return type_; }
  const Location& GetLocation() const override { return location_; }
  const Identifier& GetIdentifier() const override { return identifier_; }
  std::string GetState() const override { return state_; }
  const std::vector<NamedValue>& GetValues() const override { return values_; }
  std::vector<Ref> GetChildren() const override { return children_; }

  void set_location(const Location& loc) { location_ = loc; }
  void set_values(std::vector<NamedValue> values) { values_ = std::move(values); }
  void set_children(std::vector<Ref> children) { children_ = std::move(children); }

 private:
  uint64_t id_;
  Type type_;
  Identifier identifier_;
  std::string state_;
  Location location_;
  std::vector<NamedValue> values_;
  std::vector<Ref> children_;
};

class MockAsyncTaskTreeDelegate : public AsyncTaskTree::Delegate {
 public:
  void SyncAsyncTasks(fit::callback<void(const Err&, const Frame* const)> callback) override {
    // No-op for testing.
  }
};

class FormatAsyncTaskTest : public TestWithLoop {};

}  // namespace

TEST_F(FormatAsyncTaskTest, Basic) {
  auto eval_context = fxl::MakeRefCounted<MockEvalContext>();
  FormatTaskOptions options;

  // Test Function type
  MockAsyncTask task(1, AsyncTask::Type::kFunction, Identifier(std::string("my_func")), "Running");
  auto buffer = FormatAsyncTask(task, nullptr, options, eval_context, 0);
  EXPECT_EQ("my_func (Running)\n", LoopUntilAsyncOutputBufferComplete(buffer).AsString());

  // Test Scope type
  MockAsyncTask scope(2, AsyncTask::Type::kScope, Identifier(std::string("MyScope")), "Scope_A");
  buffer = FormatAsyncTask(scope, nullptr, options, eval_context, 0);
  EXPECT_EQ("MyScope(\"Scope_A\")\n", LoopUntilAsyncOutputBufferComplete(buffer).AsString());

  // Test with indent
  buffer = FormatAsyncTask(task, nullptr, options, eval_context, 3);
  EXPECT_EQ(kAwaiteeMarker + "my_func (Running)\n",
            LoopUntilAsyncOutputBufferComplete(buffer).AsString());
}

TEST_F(FormatAsyncTaskTest, Verbose) {
  auto eval_context = fxl::MakeRefCounted<MockEvalContext>();
  FormatTaskOptions options;
  options.verbose = true;

  MockAsyncTask task(1, AsyncTask::Type::kFunction, Identifier(std::string("my_func")), "Running");

  std::vector<AsyncTask::NamedValue> values;
  values.push_back({.name = "var1", .value = ExprValue(123)});
  task.set_values(std::move(values));

  auto buffer = FormatAsyncTask(task, nullptr, options, eval_context, 0);
  // Indent for values is indent + 2.
  EXPECT_EQ("my_func (Running)\n  var1 = 123\n",
            LoopUntilAsyncOutputBufferComplete(buffer).AsString());
}

TEST_F(FormatAsyncTaskTest, EmptyTree) {
  MockAsyncTaskTreeDelegate delegate;
  AsyncTaskTree tree(&delegate);
  auto eval_context = fxl::MakeRefCounted<MockEvalContext>();
  FormatTaskOptions options;

  auto buffer = FormatAsyncTaskTree(tree, nullptr, options, eval_context);
  EXPECT_EQ("No async tasks found.\n", LoopUntilAsyncOutputBufferComplete(buffer).AsString());
}

TEST_F(FormatAsyncTaskTest, NestedTree) {
  MockAsyncTaskTreeDelegate delegate;
  AsyncTaskTree tree(&delegate);
  auto eval_context = fxl::MakeRefCounted<MockEvalContext>();
  FormatTaskOptions options;

  auto root_owned = std::make_unique<MockAsyncTask>(1, AsyncTask::Type::kFunction,
                                                    Identifier(std::string("root")), "Running");
  MockAsyncTask* root_raw = root_owned.get();

  auto child_owned = std::make_unique<MockAsyncTask>(2, AsyncTask::Type::kFunction,
                                                     Identifier(std::string("child")), "Pending");
  MockAsyncTask* child_raw = child_owned.get();

  root_raw->set_children({*child_raw});

  std::vector<std::unique_ptr<AsyncTask>> root_tasks;
  root_tasks.push_back(std::move(root_owned));
  // Note: child_owned must be kept alive, but SetTasks only takes root_tasks.
  // In this test, child_owned will stay alive because it's in this scope.

  tree.SetTasks(std::move(root_tasks));

  auto buffer = FormatAsyncTaskTree(tree, nullptr, options, eval_context);
  EXPECT_EQ("root (Running)\n" + kAwaiteeMarker + "child (Pending)\n",
            LoopUntilAsyncOutputBufferComplete(buffer).AsString());
}

}  // namespace zxdb
