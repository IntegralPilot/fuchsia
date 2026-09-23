// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ASYNC_TASK_TREE_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ASYNC_TASK_TREE_H_

#include <lib/fit/function.h>

#include <memory>
#include <vector>

#include "src/developer/debug/zxdb/client/async_task.h"
#include "src/developer/debug/zxdb/client/frame.h"
#include "src/lib/fxl/macros.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {

class Err;
class Stack;

// Represents a logical tree of async tasks, possibly associated with an executor.
//
// This object is like the "stack" of asynchronous tasks that are registered with some asynchronous
// runtime (from here on, just "the runtime").
//
// Unlike the Stack, which is allocated by the operating system at thread creation time, and has a
// single list of stack frames, asynchronous runtimes are opted in by particular programs if they so
// choose, which can operate in single or multithreaded environments, and coordinate trees of
// asynchronous tasks. The tasks may be in many states, such as waiting on I/O operations,
// suspended, completed, or any other of many states that are defined by a particular runtime. These
// asynchronous tasks will always have some memory associated with them to hold the current state
// (e.g. "stackless" coroutines), or they may contain a small "stack" region within themselves to
// perform allocations (e.g. "stackful" coroutines).
//
// Our goal is to abstract away as many of these details as possible from different runtimes and
// languages to present a common interface for interacting with these objects for inspection and
// traversal, just like stack frames are today.
//
// The interface for this class is the entrypoint for interacting with the runtime and all of the
// currently live task objects that are registered with the runtime.
//
// The suppliers are responsible for providing the Delegate interface to construct new task objects
// and inject them into the tree.
//
// The consumers may use the getters or provided iteration methods to perform operations against the
// tasks present in the tree.
class AsyncTaskTree {
 public:
  class Delegate {
   public:
    virtual ~Delegate() = default;
    // Requests that the AsyncTaskTree be provided with a new set of tasks. The implementation
    // should asynchronously request the task information, call AsyncTaskTree::SetTasks(), then
    // issue the callback to indicate completion. The optionally provided |frame| argument to the
    // callback is plumbed from callers of |AsyncTaskTree::Sync| so that they may use the correct
    // stack frame's EvalContext.
    virtual void SyncAsyncTasks(fit::callback<void(const Err&, const Frame* frame)> callback) = 0;
  };

  explicit AsyncTaskTree(Delegate* delegate);
  ~AsyncTaskTree();

  fxl::WeakPtr<AsyncTaskTree> GetWeakPtr();

  // Returns whether or not the task tree is non-empty. If this method returns true it means the
  // traversal methods below will work without calling |Sync| first. This is primarily an
  // optimization for callers to use to avoid calling |Sync| repeatedly on a task tree that has
  // already been cached.
  bool has_tasks() const { return !root_tasks_.empty(); }

  // Requests that all task information be updated. Automatically requests the full stack if it
  // isn't already present.
  void Sync(Stack& stack, fit::callback<void(const Err&, const Frame* frame)> callback);

  // Provides a new set of root tasks.
  void SetTasks(std::vector<std::unique_ptr<AsyncTask>> tasks);

  // Removes all tasks.
  void ClearTasks();

  struct TaskEntry {
    const AsyncTask& task;
    int depth = 0;
  };

  // The following are efficient accessor methods for all of the tasks in the tree. The root tasks
  // are not exposed publicly from this class intentionally so that callers use one of the below
  // iteration methods to accomplish their goals rather than implementing their own recursion
  // through the tree that could blow up our stack for large task trees.

  // Performs a depth-first iteration of all tasks in the tree, calling |fn| on each entry.
  void ForEach(fit::function<void(const TaskEntry&)> fn) const;

  // Transforms the tree of AsyncTasks into a tree of arbitrary Node types.
  //
  // This method uses an iterative, stack-based approach to traverse the tree,
  // avoiding potential stack overflow issues that can occur with recursive
  // traversal of deeply nested task structures. Prefer using this or ForEach
  // when possible.
  //
  // Parameters:
  //   populate_fn: A callable with signature `void(const AsyncTask& task, Node*
  //                node)`. It is responsible for copying data from the `task`
  //                into the `node`.
  //   children_fn: A callable with signature `std::vector<Node>*(Node* node)`.
  //                It must return a pointer to a `std::vector` that holds the
  //                children of the given `node`. This allows the Map function
  //                to resize the container and push child nodes onto its
  //                processing stack.
  //
  // Example:
  // ```cpp
  //   struct MyNode {
  //     std::string name;
  //     std::vector<MyNode> children;
  //   };
  //
  //   std::vector<MyNode> result = tree.Map<MyNode>(
  //       [](const zxdb::AsyncTask& task, MyNode* node) {
  //         node->name = task.GetIdentifier().GetFullName();
  //       },
  //       [](MyNode* node) {
  //         return &node->children;
  //       });
  // ```
  template <typename Node, typename PopulateFn, typename ChildrenFn>
  std::vector<Node> Map(PopulateFn populate_fn, ChildrenFn children_fn) const {
    std::vector<Node> root_nodes;
    auto roots = GetRootTasks();
    root_nodes.resize(roots.size());

    struct StackItem {
      AsyncTask::Ref task;
      Node* node;
    };
    std::vector<StackItem> stack;

    for (size_t i = 0; i < roots.size(); ++i) {
      stack.push_back({roots[i], &root_nodes[i]});
    }

    while (!stack.empty()) {
      StackItem item = stack.back();
      stack.pop_back();

      populate_fn(item.task.get(), item.node);

      auto children = item.task.get().GetChildren();
      if (!children.empty()) {
        if (std::vector<Node>* out_children = children_fn(item.node)) {
          out_children->resize(children.size());
          for (size_t i = 0; i < children.size(); ++i) {
            stack.push_back({children[i], &(*out_children)[i]});
          }
        }
      }
    }

    return root_nodes;
  }

 private:
  std::vector<AsyncTask::Ref> GetRootTasks() const;

  Delegate* delegate_;

  // These are the roots of the task tree.
  std::vector<std::unique_ptr<AsyncTask>> root_tasks_;

  fxl::WeakPtrFactory<AsyncTaskTree> weak_factory_;

  FXL_DISALLOW_COPY_AND_ASSIGN(AsyncTaskTree);
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ASYNC_TASK_TREE_H_
