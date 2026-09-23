// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/process_tree.h"

#include <lib/fpromise/promise.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/exception.h>
#include <zircon/syscalls/object.h>

#include <array>
#include <iomanip>
#include <memory>
#include <sstream>
#include <string>
#include <variant>
#include <vector>

#include <task-utils/walker.h>

namespace forensics::feedback {
namespace {

// Helper struct template to enable pattern matching visitor over std::variant.
template <class... Ts>
struct overloaded : Ts... {
  using Ts::operator()...;
};

// Represents the state of a Zircon process object.
enum class ProcessState {
  kNone,
  kExited,
};

// Represents the execution state of a Zircon thread object.
enum class ThreadState {
  kNew,
  kRunning,
  kSuspended,
  kBlocked,
  kException,
  kDying,
  kDead,
  kUnknown,
};

// Details specific to a job node.
struct JobDetail {};

// Details specific to a process node.
struct ProcessDetail {
  ProcessState state;
};

// Details specific to a thread node.
struct ThreadDetail {
  ThreadState state;
};

// Type-safe variant representing task-type specific detail.
using TaskDetail = std::variant<JobDetail, ProcessDetail, ThreadDetail>;

// Represents any node (job, process, or thread) in the task tree walk.
class TaskEntry {
 public:
  TaskEntry(zx_koid_t koid, std::string name, TaskDetail detail)
      : koid_(koid), name_(std::move(name)), detail_(std::move(detail)) {}

  static std::unique_ptr<TaskEntry> Job(zx_koid_t koid, std::string name) {
    return std::make_unique<TaskEntry>(koid, std::move(name), JobDetail{});
  }

  static std::unique_ptr<TaskEntry> Process(zx_koid_t koid, std::string name, ProcessState state) {
    return std::make_unique<TaskEntry>(koid, std::move(name), ProcessDetail{.state = state});
  }

  static std::unique_ptr<TaskEntry> Thread(zx_koid_t koid, std::string name, ThreadState state) {
    return std::make_unique<TaskEntry>(koid, std::move(name), ThreadDetail{.state = state});
  }

  zx_koid_t Koid() const { return koid_; }
  const std::string& Name() const { return name_; }
  const std::vector<std::unique_ptr<TaskEntry>>& Children() const { return children_; }

  void AddChild(std::unique_ptr<TaskEntry> child) { children_.push_back(std::move(child)); }

  char TypeChar() const {
    return std::visit(overloaded{
                          [](const JobDetail&) { return 'j'; },
                          [](const ProcessDetail&) { return 'p'; },
                          [](const ThreadDetail&) { return 't'; },
                      },
                      detail_);
  }

  std::string StateString() const {
    return std::visit(overloaded{
                          [](const JobDetail&) -> std::string { return ""; },
                          [](const ProcessDetail& p) -> std::string {
                            switch (p.state) {
                              case ProcessState::kNone:
                                return "";
                              case ProcessState::kExited:
                                return "exited";
                            }
                          },
                          [](const ThreadDetail& t) -> std::string {
                            switch (t.state) {
                              case ThreadState::kNew:
                                return "new";
                              case ThreadState::kRunning:
                                return "running";
                              case ThreadState::kSuspended:
                                return "susp";
                              case ThreadState::kBlocked:
                                return "blocked";
                              case ThreadState::kException:
                                return "excp";
                              case ThreadState::kDying:
                                return "dying";
                              case ThreadState::kDead:
                                return "dead";
                              case ThreadState::kUnknown:
                                return "unknown";
                            }
                          },
                      },
                      detail_);
  }

 private:
  zx_koid_t koid_;
  std::string name_;
  TaskDetail detail_;
  std::vector<std::unique_ptr<TaskEntry>> children_;
};

// Converts a zx_info_thread_t struct into a ThreadState enum.
ThreadState GetThreadState(const zx_info_thread_t& info) {
  if (ZX_THREAD_STATE_BASIC(info.state) == ZX_THREAD_STATE_BLOCKED_EXCEPTION &&
      info.wait_exception_channel_type != ZX_EXCEPTION_CHANNEL_TYPE_NONE) {
    return ThreadState::kException;
  }
  switch (ZX_THREAD_STATE_BASIC(info.state)) {
    case ZX_THREAD_STATE_NEW:
      return ThreadState::kNew;
    case ZX_THREAD_STATE_RUNNING:
      return ThreadState::kRunning;
    case ZX_THREAD_STATE_SUSPENDED:
      return ThreadState::kSuspended;
    case ZX_THREAD_STATE_BLOCKED:
      return ThreadState::kBlocked;
    case ZX_THREAD_STATE_DYING:
      return ThreadState::kDying;
    case ZX_THREAD_STATE_DEAD:
      return ThreadState::kDead;
    default:
      return ThreadState::kUnknown;
  }
}

// Reads the ZX_PROP_NAME property of a task object. Returns an empty string if reading fails.
std::string GetTaskName(zx_handle_t task) {
  std::array<char, ZX_MAX_NAME_LEN> name_buf{};
  if (zx_object_get_property(task, ZX_PROP_NAME, name_buf.data(), name_buf.size()) == ZX_OK) {
    return std::string(name_buf.data());
  }
  return {};
}

// Subclasses TaskEnumerator to collect all jobs, processes, and threads during a job tree walk.
class DumpEnumerator : public TaskEnumerator {
 public:
  zx_status_t OnJob(int depth, zx_handle_t job, zx_koid_t koid, zx_koid_t parent_koid) override {
    AddEntry(parent_koid, TaskEntry::Job(koid, GetTaskName(job)));
    return ZX_OK;
  }

  zx_status_t OnProcess(int depth, zx_handle_t process, zx_koid_t koid,
                        zx_koid_t parent_koid) override {
    ProcessState state = ProcessState::kNone;
    zx_info_process_t info;
    if (zx_object_get_info(process, ZX_INFO_PROCESS, &info, sizeof(info), nullptr, nullptr) ==
        ZX_OK) {
      if (info.flags & ZX_INFO_PROCESS_FLAG_EXITED) {
        state = ProcessState::kExited;
      }
    }

    AddEntry(parent_koid, TaskEntry::Process(koid, GetTaskName(process), state));
    return ZX_OK;
  }

  zx_status_t OnThread(int depth, zx_handle_t thread, zx_koid_t koid,
                       zx_koid_t parent_koid) override {
    ThreadState state = ThreadState::kUnknown;
    zx_info_thread_t info;
    if (zx_object_get_info(thread, ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr) ==
        ZX_OK) {
      state = GetThreadState(info);
    }

    AddEntry(parent_koid, TaskEntry::Thread(koid, GetTaskName(thread), state));
    return ZX_OK;
  }

  // Formats the collected job/process/thread entries into a text table.
  // Trims trailing whitespace from each line.
  //
  // Example output:
  // TASK            STATE   NAME
  // j: 1024                 root
  //   p: 1035               app.cm
  //     t: 1045     running main
  //     t: 1056     blocked worker
  //   p: 1060       exited dead_process
  std::string RenderTable() const {
    if (entries_.empty()) {
      return "";
    }

    int id_w = 4;  // minimum width for header "TASK"
    for (const std::unique_ptr<TaskEntry>& entry : entries_) {
      id_w = std::max(id_w, MaxIdWidth(*entry));
    }

    std::ostringstream ss;
    std::string header = (std::ostringstream() << std::left << std::setw(id_w) << "TASK" << " "
                                               << std::setw(7) << "STATE" << " NAME")
                             .str();
    while (!header.empty() && header.back() == ' ') {
      header.pop_back();
    }
    ss << header << "\n";

    for (const std::unique_ptr<TaskEntry>& entry : entries_) {
      RenderNode(*entry, /*depth=*/0, id_w, ss);
    }

    return ss.str();
  }

 protected:
  bool has_on_job() const override { return true; }
  bool has_on_process() const override { return true; }
  bool has_on_thread() const override { return true; }

 private:
  void AddEntry(zx_koid_t parent_koid, std::unique_ptr<TaskEntry> entry) {
    while (!task_stack_.empty() && task_stack_.back()->Koid() != parent_koid) {
      task_stack_.pop_back();
    }

    TaskEntry* added = entry.get();
    if (task_stack_.empty()) {
      entries_.push_back(std::move(entry));
    } else {
      task_stack_.back()->AddChild(std::move(entry));
    }

    task_stack_.push_back(added);
  }

  // Recursively calculates the maximum width of the formatted task ID string
  // (including indentation, task type character, colon-space separator, and KOID)
  // for a task entry and all its subtree descendants.
  static int MaxIdWidth(const TaskEntry& entry, int depth = 0) {
    int max_w = depth * 2 + 3 + static_cast<int>(std::to_string(entry.Koid()).length());
    for (const std::unique_ptr<TaskEntry>& child : entry.Children()) {
      max_w = std::max(max_w, MaxIdWidth(*child, depth + 1));
    }
    return max_w;
  }

  // Recursively renders a task entry and its children as table rows into |ss|.
  // Formats each row with proper depth indentation, aligned TASK ID column of width |id_w|,
  // STATE string, and task NAME, stripping any trailing spaces.
  static void RenderNode(const TaskEntry& entry, int depth, int id_w, std::ostringstream& ss) {
    std::string task_id(depth * 2, ' ');
    task_id += entry.TypeChar();
    task_id += ": ";
    task_id += std::to_string(entry.Koid());

    std::ostringstream line_stream;
    line_stream << std::left << std::setw(id_w) << task_id << " " << std::setw(7)
                << entry.StateString() << " " << entry.Name();

    std::string line_str = line_stream.str();
    while (!line_str.empty() && line_str.back() == ' ') {
      line_str.pop_back();
    }
    ss << line_str << "\n";

    for (const std::unique_ptr<TaskEntry>& child : entry.Children()) {
      RenderNode(*child, depth + 1, id_w, ss);
    }
  }

  std::vector<std::unique_ptr<TaskEntry>> entries_;
  std::vector<TaskEntry*> task_stack_;
};

}  // namespace

ProcessTree::ProcessTree(zx::job job) : job_(std::move(job)) {}

::fpromise::promise<AttachmentData> ProcessTree::Get(uint64_t ticket) {
  if (!job_.is_valid()) {
    return fpromise::make_ok_promise(AttachmentData(Error::kMissingValue));
  }

  DumpEnumerator enumerator;
  if (const zx_status_t status = enumerator.WalkJobTree(job_.get()); status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "Failed to walk job tree for process tree";
    return fpromise::make_ok_promise(AttachmentData(Error::kMissingValue));
  }

  std::string dump = enumerator.RenderTable();
  if (dump.empty()) {
    return fpromise::make_ok_promise(AttachmentData(Error::kMissingValue));
  }

  return fpromise::make_ok_promise(AttachmentData(std::move(dump)));
}

void ProcessTree::ForceCompletion(uint64_t ticket, Error error) {}

}  // namespace forensics::feedback
