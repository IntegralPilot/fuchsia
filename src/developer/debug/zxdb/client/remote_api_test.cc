// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/remote_api_test.h"

#include <inttypes.h>

#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/zxdb/client/frame.h"
#include "src/developer/debug/zxdb/client/mock_remote_api.h"
#include "src/developer/debug/zxdb/client/process_impl.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/client/target_impl.h"
#include "src/developer/debug/zxdb/client/thread_impl.h"
#include "src/developer/debug/zxdb/common/string_util.h"
#include "src/developer/debug/zxdb/symbols/loaded_module_symbols.h"
#include "src/developer/debug/zxdb/symbols/mock_module_symbols.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

void RemoteAPITest::SetUp() {
  session_ = std::make_unique<Session>(GetRemoteAPIImpl(), GetArch(), GetPlatform(), 4096);
}

void RemoteAPITest::TearDown() { session_.reset(); }

void RemoteAPITest::InjectModule(Process* process, fxl::RefPtr<ModuleSymbols> mod_sym,
                                 const std::string& name, uint64_t load_address,
                                 const std::string& build_id) {
  // This injects the mock module to the system symbols, which the real ProcessImpl will query
  // during symbol loading routines invoked from |OnModules| below.
  session().system().GetSymbols()->InjectModuleForTesting(build_id, mod_sym.get());

  std::vector<debug_ipc::Module> modules;
  // Make sure we don't completely overwrite the module list, since everything's local we can just
  // ask the process what it already thinks it has loaded.
  for (const auto& loaded : process->GetSymbols()->GetLoadedModuleSymbols()) {
    auto& load = modules.emplace_back();
    load.base = loaded->load_address();
    load.build_id = loaded->build_id();
    load.name = loaded->module_symbols()->GetStatus().name;
  }

  // And now add in our new module.
  debug_ipc::Module new_remote_module;
  new_remote_module.name = name;
  new_remote_module.base = load_address;
  new_remote_module.build_id = build_id;
  modules.push_back(new_remote_module);

  // TODO(https://fxbug.dev/441982241): This should probably use a MockProcess instead of a
  // ProcessImpl.
  ProcessImpl* process_impl = session().system().ProcessImplFromKoid(process->GetKoid());
  FX_CHECK(process_impl);
  process_impl->OnModules(modules);
}

fxl::RefPtr<MockModuleSymbols> RemoteAPITest::InjectMockModule(Process* process,
                                                               uint64_t load_address,
                                                               const std::string& build_id,
                                                               bool loaded) {
  // Index to generate unique names for each mock module created. Must start > 0 because this is
  // used to generate a load address that can't be null.
  static int next_mock_module_id = 0;
  next_mock_module_id++;

  // Generate a load address if necessary.
  uint64_t effective_load_address;
  if (load_address) {
    effective_load_address = load_address;
  } else {
    // Use our unique index as the high 32-bits, with the low bits as 0.
    effective_load_address = static_cast<uint64_t>(next_mock_module_id) << 32;
  }

  std::string effective_build_id;
  if (build_id.empty()) {
    effective_build_id = "mock_build_id_" + std::to_string(next_mock_module_id);
  } else {
    effective_build_id = build_id;
  }

  auto module =
      fxl::MakeRefCounted<MockModuleSymbols>("mock_modules.so", effective_build_id, loaded);

  InjectModule(process, module, "mock_module", effective_load_address, effective_build_id);

  return module;
}

Process* RemoteAPITest::InjectProcess(uint64_t process_koid) {
  // TODO(https://fxbug.dev/441982241): This should not be using a TargetImpl to create ProcessImpl
  // here, instead we should be using the respective mocks which can better cope with some of the
  // broken abstraction boundaries that are created in test environments.
  auto target = session().system().GetNextTargetForTesting();
  target->CreateProcessForTesting(
      process_koid, fxl::StringPrintf("process-%s", to_hex_string(process_koid).c_str()));
  return target->GetProcess();
}

Process* RemoteAPITest::InjectProcessWithModule(uint64_t process_koid, uint64_t load_address) {
  auto process = InjectProcess(process_koid);
  InjectMockModule(process, load_address);

  // This is a conditional in case a derived class has overridden GetRemoteAPIImpl to provide a
  // different implementation.
  if (mock_remote_api_)
    mock_remote_api_->GetAndResetResumeCount();

  return process;
}

Thread* RemoteAPITest::InjectThread(uint64_t process_koid, uint64_t thread_koid) {
  debug_ipc::NotifyThreadStarting notify;
  notify.record.id = {.process = process_koid, .thread = thread_koid};
  notify.record.name = fxl::StringPrintf("test %" PRIu64, thread_koid);
  notify.record.state = debug_ipc::ThreadRecord::State::kRunning;

  session_->DispatchNotifyThreadStarting(notify);
  return session_->ThreadImplFromKoid(notify.record.id);
}

void RemoteAPITest::InjectException(const debug_ipc::NotifyException& exception) {
  // Set up a default ThreadStatusReply with the given information if there isn't one already. It's
  // likely that some part of the system will ask for a full stack if this one is not shortly after
  // dispatching the exception to the session. We do _not_ claim that we have a full stack so that
  // the caller may replace this reply with their own that does have a full stack later.
  if (mock_remote_api_ && !mock_remote_api()->thread_status_reply()) {
    debug_ipc::ThreadStatusReply thread_status_reply;
    thread_status_reply.record = exception.thread;
    mock_remote_api()->set_thread_status_reply(thread_status_reply);
  }

  session_->DispatchNotifyException(exception);
}

void RemoteAPITest::InjectExceptionWithStack(const debug_ipc::NotifyException& exception,
                                             std::vector<std::unique_ptr<Frame>> frames,
                                             bool has_all_frames) {
  ThreadImpl* thread = session_->ThreadImplFromKoid(exception.thread.id);
  FX_CHECK(thread);  // Tests should always pass valid KOIDs.

  // Create an exception record with a thread frame so it's valid. There must be one frame even
  // though the stack will be immediately overwritten.
  debug_ipc::NotifyException modified(exception);
  modified.thread.name = thread->GetName();
  modified.thread.stack_amount = debug_ipc::ThreadRecord::StackAmount::kMinimal;
  modified.thread.frames.clear();
  if (!frames.empty())
    modified.thread.frames.emplace_back(frames[0]->GetAddress(), frames[0]->GetStackPointer());

  // To manually set the thread state, set the general metadata which will pick up the basic flags
  // and the first stack frame. Then re-set the stack frame with the information passed in by our
  // caller.
  thread->SetMetadata(modified.thread, /*skip_frames=*/true);
  thread->GetStack().SetFramesForTest(std::move(frames), has_all_frames);

  // Set up a default ThreadStatusReply with the given information if there isn't one already. It's
  // likely that some part of the system will ask for a full stack if this one is not shortly after
  // dispatching the exception to the session. We do _not_ claim that we have a full stack so that
  // the caller may replace this reply with their own that does have a full stack later.
  if (mock_remote_api_ && !mock_remote_api()->thread_status_reply()) {
    debug_ipc::ThreadStatusReply thread_status_reply;
    thread_status_reply.record = modified.thread;
    mock_remote_api()->set_thread_status_reply(thread_status_reply);
  }

  // Normal exception dispatch path, but skipping the metadata (so the metadata set above will
  // stay).
  session_->DispatchNotifyException(modified, false);
}

void RemoteAPITest::InjectExceptionWithStack(
    uint64_t process_koid, uint64_t thread_koid, debug_ipc::ExceptionType exception_type,
    std::vector<std::unique_ptr<Frame>> frames, bool has_all_frames,
    const std::vector<debug_ipc::BreakpointStats>& breakpoints) {
  debug_ipc::NotifyException exception;
  exception.type = exception_type;
  exception.thread.id = {.process = process_koid, .thread = thread_koid};
  exception.thread.state = debug_ipc::ThreadRecord::State::kBlocked;
  exception.thread.blocked_reason = debug_ipc::ThreadRecord::BlockedReason::kException;
  exception.hit_breakpoints = breakpoints;

  InjectExceptionWithStack(exception, std::move(frames), has_all_frames);
}

std::unique_ptr<RemoteAPI> RemoteAPITest::GetRemoteAPIImpl() {
  auto remote_api = std::make_unique<MockRemoteAPI>();
  mock_remote_api_ = remote_api.get();
  return remote_api;
}

}  // namespace zxdb
