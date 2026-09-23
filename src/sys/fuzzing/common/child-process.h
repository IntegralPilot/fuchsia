// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_COMMON_CHILD_PROCESS_H_
#define SRC_SYS_FUZZING_COMMON_CHILD_PROCESS_H_

#include <lib/fdio/spawn.h>
#include <lib/zx/process.h>
#include <stddef.h>
#include <stdint.h>

#include <array>
#include <condition_variable>
#include <memory>
#include <string>
#include <string_view>
#include <thread>
#include <vector>

#include "src/lib/fxl/synchronization/thread_annotations.h"
#include "src/sys/fuzzing/common/async-deque.h"
#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/status.h"

namespace fuzzing {

class ChildProcess final {
 public:
  explicit ChildProcess(ExecutorPtr executor);
  ~ChildProcess();

  bool is_killed() const { return killed_; }

  // Adds a command line argument for the process. The first |arg| added should be the executable
  // path relative to the "/pkg" directory, i.e. the same value as might be found in
  // `program.binary` field in a component manifest.
  //
  // Returns `ZX_ERR_OUT_OF_RANGE` if unable to add the arg, e.g. due to insufficient space in the
  // `fuchsia.process.Launcher` message; `ZX_OK` otherwise.
  //
  __WARN_UNUSED_RESULT zx_status_t AddArg(std::string_view arg);

  // Adds all of the given |args|.
  //
  // Returns `ZX_ERR_OUT_OF_RANGE` if unable to add the arg, e.g. due to insufficient space in the
  // `fuchsia.process.Launcher` message; `ZX_OK` otherwise.
  //
  __WARN_UNUSED_RESULT zx_status_t AddArgs(std::initializer_list<const char*> args);

  // Sets an environment variable. Setting the same variable multiple times updates the value.
  __WARN_UNUSED_RESULT zx_status_t SetEnvVar(std::string_view name, std::string_view value);

  // Adds a writeable pipe to the standard input of the spawned process.
  // Returns ZX_ERR_BAD_STATE if called twice without `Reset`.
  __WARN_UNUSED_RESULT zx_status_t AddStdinPipe();

  // Adds a readable pipe from the indicated output file descriptors.
  // Returns ZX_ERR_BAD_STATE if called twice without `Reset`.
  __WARN_UNUSED_RESULT zx_status_t AddStdoutPipe();
  __WARN_UNUSED_RESULT zx_status_t AddStderrPipe();

  // Takes a `channel` to be passed as a startup channel to the child process with the given `id`.
  //
  // Returns `ZX_ERR_OUT_OF_RANGE` if unable to add the channel, e.g. due to insufficient space in
  // the `fuchsia.process.Launcher` message; `ZX_OK` otherwise.
  //
  __WARN_UNUSED_RESULT zx_status_t AddChannel(uint32_t id, zx::channel channel);

  // Adds the vDSO VMO handle
  __WARN_UNUSED_RESULT zx_status_t AddVmoVdso();

  // Spawns the new child process. Returns an error if a previous process was spawned but has not
  // been |Kill|ed and |Reset|, or if spawning fails.
  __WARN_UNUSED_RESULT zx_status_t Spawn();

  // Returns whether the child process has been spawned and is running.
  bool IsAlive();

  // Return a copy of the process.
  __WARN_UNUSED_RESULT zx_status_t Duplicate(zx::process* out);

  // Writes a line to process's stdin. Returns an error if the process is not alive of if stdin has
  // been closed.
  zx_status_t WriteToStdin(std::string_view line);

  // Returns a promise to read from the process's stdout or stderr, respectively. The promise will
  // read up to a newline or EOF, whichever comes first. The promise will return an error if the
  // file descriptor is closed or was cloned, and will return |ZX_ERR_STOP| on EOF.
  ZxPromise<std::string> ReadFromStdout();
  ZxPromise<std::string> ReadFromStderr();

  // Closes the indicated pipe to or from the spawned process. May be called before or after
  // the process is `Spawn`ed.
  void CloseStdin();
  void CloseStdout();
  void CloseStderr();

  // Collect process-related statistics for the child process.
  ZxResult<ProcessStats> GetStats();

  // Promises to wait for the spawned process to terminate and return its return code.
  ZxPromise<int64_t> Wait();

  // Returns a promise that kills the spawned process and waits for it to fully terminate. This
  // leaves the process in a "killed" state; it must be |Reset| before it can be reused.
  ZxPromise<> Kill();

  // Returns this object to an initial state, from which |Spawn| can be called again. This
  // effectively kills the process, but does not wait for it to fully terminate. Callers should
  // prefer to |Kill| and then |Reset|. |AddArgs|, |SetFdAction|, and/or |SetChannels| will need to
  // be called again before respawning.
  void Reset();

 private:
  // Creates a pipe and adds an `fdio_spawn_action_t` that installs one end of the pipe at the given
  // `target_fd` when the process is `Spawn`ed.
  //
  // Exactly one of `out_rpipe`  and `out_wpipe` must be non-null:
  //  * A non-null `out_rpipe` indicates the pipe should be from the process, and is used to return
  //    the readable end.
  //  * A non-null `out_wpipe` indicates the pipe should be to the process, and is used to return
  //    the writeable end.
  //
  // Returns an error if the process is already spawned or if the pipe couldn;t be created.
  zx_status_t AddPipe(int target_fd, int* out_rpipe, int* out_wpipe);

  // Returns `ZX_OK` if `num_bytes` can be added; otherwise returns `ZX_ERR_OUT_OF_RANGE`.
  zx_status_t ReserveBytes(size_t num_bytes);

  // Returns `ZX_OK` if `num_handles` can be added; otherwise returns `ZX_ERR_OUT_OF_RANGE`.
  zx_status_t ReserveHandles(size_t num_handles);

  // Kills the process and waits for the output threads to join.
  void KillSync();

  ExecutorPtr executor_;
  bool spawned_ = false;
  bool killed_ = false;
  size_t num_bytes_ = 0;
  size_t num_handles_ = 0;

  // Parameters for |fdio_spawn_etc|.
  std::vector<std::string> args_;
  std::unordered_map<std::string, std::string> envvars_;
  std::vector<fdio_spawn_action_t> actions_;

  // The handle to the spawned process.
  zx::process process_;
  zx_info_process_t info_;

  // Variables to offload writing data to standard input to a dedicated thread.
  std::mutex mutex_;
  std::thread stdin_thread_;
  bool input_closed_ FXL_GUARDED_BY(mutex_) = false;
  std::vector<std::string> input_lines_ FXL_GUARDED_BY(mutex_);
  std::condition_variable input_cond_;

  // Variables to offload reading data from standard output to a dedicated thread.
  AsyncReceiverPtr<std::string> stdout_;
  std::thread stdout_thread_;
  zx_status_t stdout_result_ = ZX_OK;

  // Variables to offload reading data from standard error to a dedicated thread.
  AsyncReceiverPtr<std::string> stderr_;
  std::thread stderr_thread_;
  zx_status_t stderr_result_ = ZX_OK;

  Scope scope_;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(ChildProcess);
};

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_COMMON_CHILD_PROCESS_H_
