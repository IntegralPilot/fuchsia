// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/session.h"

#include <arpa/inet.h>
#include <ifaddrs.h>
#include <inttypes.h>
#include <netdb.h>
#include <stdio.h>
#include <sys/socket.h>
#include <unistd.h>

#include <thread>
#include <utility>
#include <variant>

#include "lib/syslog/cpp/macros.h"
#include "src/developer/debug/ipc/message_reader.h"
#include "src/developer/debug/ipc/message_writer.h"
#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/shared/buffered_fd.h"
#include "src/developer/debug/shared/logging/debug.h"
#include "src/developer/debug/shared/logging/file_line_function.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/stream_buffer.h"
#include "src/developer/debug/shared/zx_status.h"
#include "src/developer/debug/zxdb/client/arch_info.h"
#include "src/developer/debug/zxdb/client/breakpoint.h"
#include "src/developer/debug/zxdb/client/breakpoint_action.h"
#include "src/developer/debug/zxdb/client/breakpoint_impl.h"
#include "src/developer/debug/zxdb/client/filter.h"
#include "src/developer/debug/zxdb/client/minidump_remote_api.h"
#include "src/developer/debug/zxdb/client/process_impl.h"
#include "src/developer/debug/zxdb/client/remote_api_impl.h"
#include "src/developer/debug/zxdb/client/session_observer.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/client/socket_connect.h"
#include "src/developer/debug/zxdb/client/target_impl.h"
#include "src/developer/debug/zxdb/client/thread_impl.h"
#include "src/lib/fxl/memory/ref_counted.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {

// Max message size before considering it corrupt. This is very large so we can send nontrivial
// memory dumps over the channel, but ensures we won't crash trying to allocate an unreasonable
// buffer size if the stream is corrupt.
constexpr uint32_t kMaxMessageSize = 16777216;

// Remove conditional and no-sop breakpoints from stop_info; return whether we'll need to skip this
// stop_info and continue execution, which happens when the exception is a breakpoint one and all
// breakpoints in it are conditional.
bool FilterApplicableBreakpoints(StopInfo* info) {
  bool skip = false;

  if (info->exception_type == debug_ipc::ExceptionType::kHardwareBreakpoint ||
      info->exception_type == debug_ipc::ExceptionType::kWatchpoint ||
      info->exception_type == debug_ipc::ExceptionType::kSoftwareBreakpoint) {
    // It's possible that hit_breakpoints is empty even when exception_type is kSoftware,
    // e.g. the process explicitly called "int 3" on x64. In this case, we should still pause.
    if (!info->hit_breakpoints.empty()) {
      skip = true;
    }
  }

  // TODO(dangyi): Consider whether to move this logic to the Breakpoint class.
  auto breakpoint_iter = info->hit_breakpoints.begin();
  while (breakpoint_iter != info->hit_breakpoints.end()) {
    Breakpoint* breakpoint = breakpoint_iter->get();
    BreakpointSettings settings = breakpoint->GetSettings();

    if (settings.stop_mode == BreakpointSettings::StopMode::kNone) {
      // This breakpoint should be auto-resumed always. This could be done automatically by the
      // debug agent which will give better performance, but in the future we likely want to
      // add some kind of logging features that will require evaluation in the client.
      breakpoint_iter = info->hit_breakpoints.erase(breakpoint_iter);
    } else {
      skip = false;
      breakpoint_iter++;
    }
  }

  return skip;
}

}  // namespace

// PendingConnection -----------------------------------------------------------

// Storage for connection information when connecting dynamically. Making a connection has three
// asynchronous steps:
//
//  1. Resolving the host and connecting the socket. Since this is blocking, it happens on a
//     background thread.
//  2. Sending the hello message. Happens on the main thread.
//  3. Waiting for the reply and deserializing, then notifying the Session.
//
// Various things can happen in the middle.
//
//  - Any step can fail.
//  - The Session object can be destroyed (weak pointer checks).
//  - The connection could be canceled by the user (the session callback checks for this).
class Session::PendingConnection : public fxl::RefCountedThreadSafe<PendingConnection> {
 public:
  using Callback = fit::callback<void(const Err&, const debug_ipc::HelloReply&,
                                      std::unique_ptr<debug::BufferedFD>)>;
  void Initiate(Callback callback);

  // Use only when non-multithreaded.
  const SessionConnectionInfo& connection_info() { return connection_info_; }

  // There are no other functions since this will be running on a background thread and the class
  // state can't be safely retrieved. It reports all of the output state via the callback.

 private:
  FRIEND_REF_COUNTED_THREAD_SAFE(PendingConnection);
  FRIEND_MAKE_REF_COUNTED(PendingConnection);

  PendingConnection(const SessionConnectionInfo& info) : connection_info_(info) {}
  ~PendingConnection() {}

  // These are the steps of connection, in order. They each take a RefPtr to |this| to ensure the
  // class is in scope for the full flow.
  void ConnectBackgroundThread(fxl::RefPtr<PendingConnection> owner);
  void ConnectCompleteMainThread(fxl::RefPtr<PendingConnection> owner, const Err& err);
  void DataAvailableMainThread(fxl::RefPtr<PendingConnection> owner);
  void HelloCompleteMainThread(fxl::RefPtr<PendingConnection> owner, const Err& err,
                               const debug_ipc::HelloReply& reply);

  // Creates the connection (called on the background thread). On success the socket_ is populated.
  Err DoConnectBackgroundThread();

  SessionConnectionInfo connection_info_;

  // Only non-null when in the process of connecting.
  std::unique_ptr<std::thread> thread_;

  debug::MessageLoop* main_loop_ = nullptr;

  // The constructed socket and buffer.
  //
  // The socket is created by ConnectBackgroundThread and read by HelloCompleteMainThread to create
  // the buffer so needs no synchronization. It would be cleaner to pass this in the lambdas to
  // avoid threading confusion, but move-only types can't be bound.
  fbl::unique_fd socket_;
  std::unique_ptr<debug::BufferedFD> buffer_;

  // Callback when the connection is complete (or fails). Access only on the main thread.
  Callback callback_;
};

void Session::PendingConnection::Initiate(Callback callback) {
  FX_DCHECK(!thread_.get());  // Duplicate Initiate() call.

  main_loop_ = debug::MessageLoop::Current();
  callback_ = std::move(callback);

  // Create the background thread, and run the background function. The context will keep a ref to
  // this class.
  thread_ = std::make_unique<std::thread>(
      [owner = fxl::RefPtr<PendingConnection>(this)]() { owner->ConnectBackgroundThread(owner); });
}

void Session::PendingConnection::ConnectBackgroundThread(fxl::RefPtr<PendingConnection> owner) {
  Err err = DoConnectBackgroundThread();
  main_loop_->PostTask(FROM_HERE, [owner = std::move(owner), err]() {
    owner->ConnectCompleteMainThread(owner, err);
  });
}

void Session::PendingConnection::ConnectCompleteMainThread(fxl::RefPtr<PendingConnection> owner,
                                                           const Err& err) {
  // The background thread function has now completed so the thread can be destroyed. We do want to
  // join with the thread here to ensure there are no references to the PendingConnection on the
  // background thread, which might in turn cause the PendingConnection to be destroyed on the
  // background thread.
  thread_->join();
  thread_.reset();

  if (err.has_error()) {
    // Skip sending hello and forward the error.
    HelloCompleteMainThread(owner, err, debug_ipc::HelloReply());
    return;
  }

  FX_DCHECK(socket_.is_valid());

  // The buffer must be created here on the main thread since it will register with the message
  // loop to watch the FD.
  buffer_ = std::make_unique<debug::BufferedFD>(std::move(socket_));
  buffer_->Start();

  // The connection is now established, so we set up the handlers before we send the first request
  // over to the agent. Even though we're in a message loop and these handlers won't be called
  // within this stack frame, it's a good mental model to set up handlers before actually sending
  // the first piece of data.
  buffer_->set_data_available_callback([owner]() { owner->DataAvailableMainThread(owner); });
  buffer_->set_error_callback([owner]() {
    owner->HelloCompleteMainThread(owner, Err("Connection error."), debug_ipc::HelloReply());
  });

  // Send "Hello" message. We can't use the Session::Send infrastructure since the connection hasn't
  // technically been established yet.
  debug_ipc::HelloRequest request;
  request.version = debug_ipc::kCurrentProtocolVersion;
  buffer_->stream().Write(debug_ipc::Serialize(request, 1, 0));  // version not negotiated yet.
}

void Session::PendingConnection::DataAvailableMainThread(fxl::RefPtr<PendingConnection> owner) {
  // This function needs to manually deserialize the hello message since the Session stuff isn't
  // connected yet. In version 58 a 32-bit "platform" enum was added.
  if (!buffer_->stream().IsAvailable(debug_ipc::MsgHeader::kSerializedHeaderSize))
    return;  // Wait for more data.

  std::vector<char> serialized_header;
  serialized_header.resize(debug_ipc::MsgHeader::kSerializedHeaderSize);
  buffer_->stream().Peek(serialized_header.data(), debug_ipc::MsgHeader::kSerializedHeaderSize);

  // Header doesn't have a version.
  debug_ipc::MessageReader reader(std::move(serialized_header), 0);
  debug_ipc::MsgHeader header;
  reader | header;
  // Since we already validated there is enough data for the header, the header read should not
  // fail (it's just a memcpy).
  FX_CHECK(!reader.has_error());

  // Sanity checking on the size to prevent crashes.
  if (header.size > kMaxMessageSize) {
    LOGS(Error) << "Bad message received of size " << static_cast<uint32_t>(header.size) << "."
                << "(type = " << static_cast<unsigned>(header.type)
                << ", transaction = " << static_cast<unsigned>(header.transaction_id) << ")";
    HelloCompleteMainThread(std::move(owner), Err("Reply too large"), debug_ipc::HelloReply());
    return;
  }

  if (!buffer_->stream().IsAvailable(header.size))
    return;  // Wait for more data.

  std::vector<char> serialized;
  serialized.resize(header.size);
  buffer_->stream().Read(serialized.data(), header.size);

  debug_ipc::HelloReply reply;
  Err err;

  // Deserialize with version 0 to get the initial fields including the version.
  uint32_t transaction_id = 0;
  if (!debug_ipc::Deserialize(serialized, &reply, &transaction_id, 0) ||
      reply.signature != debug_ipc::HelloReply::kStreamSignature) {
    // Corrupt.
    err = Err("Corrupted reply, service is probably not the debug agent.");
  } else {
    // Now deserialize with the given version to get all of the fields.
    if (!debug_ipc::Deserialize(serialized, &reply, &transaction_id, reply.version)) {
      err = Err("Version mismatch in reply.");
    }
  }

  HelloCompleteMainThread(std::move(owner), err, reply);
}

void Session::PendingConnection::HelloCompleteMainThread(fxl::RefPtr<PendingConnection> owner,
                                                         const Err& err,
                                                         const debug_ipc::HelloReply& reply) {
  // Prevent future notifications.
  if (buffer_.get()) {
    buffer_->set_data_available_callback({});
    buffer_->set_error_callback({});
  }

  callback_(err, reply, std::move(buffer_));
}

Err Session::PendingConnection::DoConnectBackgroundThread() {
  switch (connection_info_.type) {
    case SessionConnectionType::kNetwork:
      return ConnectToHost(connection_info_.host, connection_info_.port, &socket_);
    case SessionConnectionType::kUnix:
      return ConnectToUnixSocket(connection_info_.host, &socket_);
  }
}

// Session -----------------------------------------------------------------------------------------

Session::Session()
    : remote_api_(std::make_unique<RemoteAPIImpl>(this)), system_(this), weak_factory_(this) {
  SetArch(debug::Arch::kUnknown, debug::Platform::kUnknown, 0);

  ListenForSystemSettings();
}

Session::Session(std::unique_ptr<RemoteAPI> remote_api, debug::Arch arch, debug::Platform platform,
                 uint64_t page_size)
    : remote_api_(std::move(remote_api)), system_(this), arch_(arch), weak_factory_(this) {
  Err err = SetArch(arch, platform, page_size);
  FX_DCHECK(!err.has_error());  // Should not fail for synthetically set-up architectures.

  ListenForSystemSettings();
}

Session::Session(debug::StreamBuffer* stream)
    : stream_(stream),
      remote_api_(std::make_unique<RemoteAPIImpl>(this)),
      system_(this),
      weak_factory_(this) {
  // The architecture will get set by the "local hello" reply but that's asynchronous. We want to
  // be sure the architecture pointer is guaranteed non-null during that time.
  SetArch(debug::Arch::kUnknown, debug::Platform::kUnknown, 0);

  ListenForSystemSettings();
  SendLocalHello([](const Err&) {});
}

Session::~Session() {
  // This class is guaranteed to destruct before the analytics cleanup routine is called. See
  // zxdb/main.cc.
  analytics_reporter_.ReportSessionEnded();
}

void Session::OnStreamReadable() {
  if (!stream_)
    return;  // Notification could have raced with detaching the stream.

  while (true) {
    if (!stream_ || !stream_->IsAvailable(debug_ipc::MsgHeader::kSerializedHeaderSize))
      return;

    std::vector<char> serialized_header;
    serialized_header.resize(debug_ipc::MsgHeader::kSerializedHeaderSize);
    stream_->Peek(serialized_header.data(), debug_ipc::MsgHeader::kSerializedHeaderSize);

    // header doesn't have a version.
    debug_ipc::MessageReader reader(std::move(serialized_header), 0);
    debug_ipc::MsgHeader header;
    reader | header;
    // Since we already validated there is enough data for the header, the header read should not
    // fail (it's just a memcpy).
    FX_CHECK(!reader.has_error());

    if (header.size > kMaxMessageSize) {
      LOGS(Error) << "Bad message received of size " << static_cast<uint32_t>(header.size) << "."
                  << "(type = " << static_cast<unsigned>(header.type)
                  << ", transaction = " << static_cast<unsigned>(header.transaction_id) << ")";
      // TODO(brettw) close the stream due to this fatal error.
      return;
    }

    if (!stream_->IsAvailable(header.size))
      return;  // Wait for more data.

    // Consume the message now that we know the size. Do this before doing anything else so the data
    // is consumed if the size is right, even if the transaction ID is wrong.
    std::vector<char> serialized;
    serialized.resize(header.size);
    stream_->Read(serialized.data(), header.size);

    // Transaction ID 0 is reserved for notifications.
    if (header.transaction_id == 0) {
      DispatchNotification(header, std::move(serialized));
      continue;
    }

    // Find the transaction.
    auto found = pending_.find(header.transaction_id);
    if (found == pending_.end()) {
      LOGS(Error) << "Received reply for unexpected transaction "
                  << static_cast<unsigned>(header.transaction_id)
                  << " (type = " << static_cast<unsigned>(header.type) << ".";
      // Just ignore this bad message.
      continue;
    }

    // Do the type-specific deserialization and callback.
    found->second(Err(), std::move(serialized));

    pending_.erase(found);
  }
}

void Session::OnStreamError() {
  if (ClearConnectionData()) {
    LOGS(Error) << "The debug agent has disconnected.\n"
                   "The system may have halted, or this may be a bug. "
                   "If you believe it is a bug, please file a report, "
                   "adding the system crash log (ffx log) if possible.";
  }
}

bool Session::ConnectCanProceed(fit::callback<void(const Err&)>& callback, bool opening_dump) {
  Err err;
  if (stream_) {
    if (opening_dump) {
      err = Err("Cannot open a dump while connected to a debugged system.");
    } else {
      err = Err("Already connected.");
    }
  } else if (is_minidump_) {
    err = Err("A dump file is currently open.");
  } else if (pending_connection_.get()) {
    err = Err("A connection is already pending.");
  }

  if (err.has_error()) {
    if (callback) {
      debug::MessageLoop::Current()->PostTask(
          FROM_HERE, [callback = std::move(callback), err]() mutable { callback(err); });
    }
    return false;
  }

  // If the connection can proceed, then we are starting a session.
  analytics_reporter_.ReportSessionStarted();
  return true;
}

void Session::Connect(const SessionConnectionInfo& info, fit::callback<void(const Err&)> cb) {
  if (!ConnectCanProceed(cb, false))
    return;

  if (info.host.empty() && last_connection_.host.empty()) {
    debug::MessageLoop::Current()->PostTask(FROM_HERE, [cb = std::move(cb)]() mutable {
      cb(Err("No previous destination to reconnect to."));
    });
    return;
  }

  connected_info_ = info;
  if (!connected_info_.host.empty()) {
    last_connection_ = info;
  }

  pending_connection_ = fxl::MakeRefCounted<PendingConnection>(last_connection_);
  pending_connection_->Initiate(
      [weak_this = GetWeakPtr(), pending = pending_connection_, cb = std::move(cb)](
          Err err, const debug_ipc::HelloReply& reply,
          std::unique_ptr<debug::BufferedFD> buffer) mutable {
        if (!weak_this) {
          cb(Err("Session was destroyed."));
        } else {
          if (!err.has_error()) {
            err = weak_this->ResolvePendingConnection(pending, reply, std::move(buffer));
          }
          for (auto& observer : weak_this->observers_) {
            observer.DidResolveConnection(err);
          }
          cb(err);
          weak_this->last_connection_error_ = err;
        }
      });
}

Err Session::SetArch(debug::Arch arch, debug::Platform platform, uint64_t page_size) {
  arch_info_ = std::make_unique<ArchInfo>();

  Err arch_err = arch_info_->Init(arch, page_size);
  if (!arch_err.has_error()) {
    arch_ = arch;
    platform_ = platform;
  } else {
    LOGS(Error) << "Fail to init ArchInfo: " << arch_err.msg();
    // Rollback to default-initialized ArchInfo;
    arch_info_ = std::make_unique<ArchInfo>();
  }

  return arch_err;
}

void Session::OpenMinidump(const std::string& path, fit::callback<void(const Err&)> callback) {
  if (!ConnectCanProceed(callback, true)) {
    return;
  }

  auto minidump_remote_api = std::make_unique<MinidumpRemoteAPI>(this);
  auto minidump = reinterpret_cast<MinidumpRemoteAPI*>(minidump_remote_api.get());
  Err err = minidump->Open(path);

  if (err.has_error()) {
    debug::MessageLoop::Current()->PostTask(
        FROM_HERE, [callback = std::move(callback), err]() mutable { callback(err); });
    return;
  }

  // Wait to set these internal variables until we are sure that the minidump was properly opened.
  // This delay means that a failed "opendump" command from the user does not put the session in a
  // weird state where the user then has to issue "disconnect" before another "opendump" can be
  // completed.
  remote_api_ = std::move(minidump_remote_api);
  is_minidump_ = true;
  minidump_path_ = path;

  // We need to "connect" to the |MinidumpRemoteAPI| instance before attaching to the process(es) in
  // the core file in order to properly populate the architecture information in time to print it to
  // the UI with all the exception information correctly decoded, which is architecture specific and
  // can only happen after the architecture information has been given here.
  SendLocalHello(std::move(callback));

  system().GetTargets()[0]->Attach(
      minidump->ProcessID(), debug_ipc::AttachConfig(),
      [](fxl::WeakPtr<Target> target, const Err&, uint64_t timestamp) {});
}

Err Session::Disconnect() {
  if (!stream_ && !is_minidump_) {
    if (pending_connection_) {
      // Cancel pending connection.
      pending_connection_ = nullptr;
      return Err();
    } else {
      return Err("Not connected.");
    }
  }

  if (is_minidump_) {
    is_minidump_ = false;
    minidump_path_.clear();
    remote_api_ = std::make_unique<RemoteAPIImpl>(this);
  } else if (!connection_storage_) {
    // The connection is persistent (passed in via the constructor) and can't
    // be disconnected.
    return Err(ErrType::kGeneral,
               "The connection can't be disconnected in this build of the debugger.");
  }

  ClearConnectionData();
  return Err();
}

bool Session::ClearConnectionData() {
  if (!connection_storage_)
    return false;

  stream_ = nullptr;
  connected_info_.host.clear();
  connected_info_.port = 0;
  arch_info_ = std::make_unique<ArchInfo>();  // Reset to default one (always keep non-null).
  connection_storage_.reset();
  arch_ = debug::Arch::kUnknown;
  system_.DidDisconnect();
  return true;
}

void Session::DispatchNotifyThreadStarting(const debug_ipc::NotifyThreadStarting& notify) {
  ProcessImpl* process = system_.ProcessImplFromKoid(notify.record.id.process);
  if (!process) {
    LOGS(Warn) << "Received thread starting notification for an "
                  "unexpected process "
               << notify.record.id.process << ".";
    return;
  }

  process->OnThreadStarting(notify.record);
}

void Session::DispatchNotifyThreadExiting(const debug_ipc::NotifyThreadExiting& notify) {
  ProcessImpl* process = system_.ProcessImplFromKoid(notify.record.id.process);
  if (!process) {
    LOGS(Warn) << "Received thread exiting notification for an "
                  "unexpected process "
               << notify.record.id.process << ".";
    return;
  }

  process->OnThreadExiting(notify.record);
}

void Session::HandleException(ThreadImpl* thread, const debug_ipc::NotifyException& notify,
                              HandleExceptionSettings settings) {
  if (!thread) {
    LOGS(Warn) << "Received thread exception for an unknown thread: pr:" << notify.thread.id.process
               << " thread: " << notify.thread.id.thread;
    return;
  }

  // First update the thread state so the breakpoint code can query it. This should not issue any
  // notifications.
  if (settings.set_metadata)
    thread->SetMetadata(notify.thread, settings.skip_metadata_frames);

  // The breakpoints that were hit to pass to the thread stop handler.
  StopInfo info;
  info.exception_type = notify.type;
  info.exception_record = notify.exception;
  info.timestamp = notify.timestamp;

  if (settings.notify_only) {
    // At this point there is no more live thread on the target. We cannot query for any more stack,
    // and we cannot ask to load modules. There should be no breakpoints since this implies we were
    // only attached to the parent job of the process that this excepting thread is running within.
    // We also don't need to worry about continuation, since the backend immediately releases the
    // exception in this path.
    FX_DCHECK(notify.hit_breakpoints.empty());
    thread->OnException(info);
    return;
  }

  ProcessImpl* process = thread->process();
  process->SetMemoryBlocks(thread->GetKoid(), notify.memory_blocks);

  if (!notify.hit_breakpoints.empty()) {
    // Update breakpoints' hit counts and stats. This is done before any notifications are sent so
    // that all breakpoint state is consistent.
    for (const debug_ipc::BreakpointStats& stats : notify.hit_breakpoints) {
      BreakpointImpl* impl = system_.BreakpointImplForId(stats.id);
      if (impl) {
        impl->UpdateStats(stats);
        info.hit_breakpoints.push_back(impl->GetWeakPtr());
      }
    }
  }

  // Continue if it's a conditional breakpoint.
  if (FilterApplicableBreakpoints(&info)) {
    // For simplicity, we're resuming all threads right now.
    // TODO(dangyi): It's better to continue only the affected threads.
    system_.Continue(false);
  } else {
    // This is the main notification of an exception.
    thread->OnException(info);
  }

  // Delete all one-shot breakpoints the backend deleted. This happens after the thread
  // notifications so observers can tell why the thread stopped.
  for (const auto& stats : notify.hit_breakpoints) {
    if (!stats.should_delete)
      continue;

    // Breakpoint needs deleting.
    BreakpointImpl* impl = system_.BreakpointImplForId(stats.id);
    if (impl) {
      // Need to tell the breakpoint it was removed in the backend before deleting it or it will try
      // to uninstall itself.
      impl->BackendBreakpointRemoved();
      system_.DeleteBreakpoint(impl);
    }
  }
}

// This is the main entrypoint for all thread stops notifications in the client.
void Session::DispatchNotifyException(const debug_ipc::NotifyException& notify, bool set_metadata) {
  ThreadImpl* thread = ThreadImplFromKoid(notify.thread.id);
  if (!thread) {
    // Don't crash if we get an invalid KOID from the agent. However, the agent should be sending
    // us valid IDs so debug assert to try to identify that error.
    FX_DCHECK(thread) << "Got notification for nonexistant process/thread "
                      << notify.thread.id.process << "/" << notify.thread.id.thread;
    return;
  }

  HandleExceptionSettings settings;
  settings.set_metadata = set_metadata;
  settings.skip_metadata_frames = false;

  if (thread->process()->HasLoadedSymbols()) {
    // Normal case, just handle the exception.
    HandleException(thread, notify, settings);
    return;
  }

  // This came from the notification traveling up the job tree, rather than because we were attached
  // to this process directly. In this configuration we need to dispatch the exception notification,
  // but not load modules or unwind the stack on the target, since the backend is not holding the
  // exception for us after sending this notification.
  if (notify.job_only) {
    settings.notify_only = true;
    HandleException(thread, notify, settings);
    return;
  }

  // If we were weakly attached, we may not have symbols yet. Check now if we need to load them,
  // then dispatch the exception to be handled.
  thread->process()->GetModules(true, [weak_this = GetWeakPtr(), notify, settings, thread](
                                          const Err& err, const std::vector<debug_ipc::Module>&) {
    if (err.has_error())
      LOGS(Warn) << err.msg();

    thread->process()->SyncThreads([weak_this, notify, settings, thread]() {
      if (weak_this && thread) {
        thread->GetStack().SyncFrames(
            {.force_update = true}, [weak_this, notify, settings, thread](const Err& err) mutable {
              if (weak_this && thread) {
                // Make sure we don't set the thread's stack to that from the exception
                // notification, which will not contain the same amount of information that we just
                // synced from the target.
                settings.skip_metadata_frames = true;
                weak_this->HandleException(thread, notify, settings);
              }
            });
      }
    });
  });
}

void Session::DispatchNotifyModules(const debug_ipc::NotifyModules& notify) {
  ProcessImpl* process = system_.ProcessImplFromKoid(notify.process_koid);
  if (process) {
    process->OnModules(std::move(notify.modules));
  } else {
    LOGS(Warn) << "Received modules notification for an unexpected process: "
               << notify.process_koid;
  }
}

void Session::DispatchNotifyProcessStarting(const debug_ipc::NotifyProcessStarting& notify) {
  if (notify.type == debug_ipc::NotifyProcessStarting::Type::kLimbo) {
    if (auto_attach_limbo_) {
      AttachToLimboProcessAndNotify(notify.koid, notify.name);
    } else {
      LOGS(Info) << "Process " << notify.name << "(" << notify.koid
                 << ") crashed and is being held in limbo.\n"
                    "Use `attach "
                 << notify.koid << "` to attach.";
    }
    return;
  }

  // Search the targets to see if there is a non-attached empty one. Normally this would be the
  // initial one. Assume that targets that have a name have been set up by the user which we don't
  // want to overwrite.
  TargetImpl* found_target = nullptr;
  for (TargetImpl* target : system_.GetTargetImpls()) {
    if (target->GetState() == Target::State::kNone && target->GetArgs().empty()) {
      found_target = target;
      break;
    }
  }

  if (!found_target)  // No empty target, make a new one.
    found_target = system_.CreateNewTargetImpl(nullptr);

  auto start_type = notify.type == debug_ipc::NotifyProcessStarting::Type::kAttach
                        ? Process::StartType::kAttach
                        : Process::StartType::kLaunch;

  found_target->CreateProcess(start_type, notify.koid, notify.name, notify.timestamp,
                              notify.components, notify.shared_address_space);

  auto have_any_matching_filter = std::ranges::any_of(
      notify.filter_ids, [&](const auto& filter_id) { return system().GetFilterForId(filter_id); });

  // If the notification is coming from a weak filter, defer fetching modules until later.
  if (have_any_matching_filter &&
      !debug_ipc::AttachConfig::ShouldDeferModules(notify.attach_config)) {
    found_target->process()->GetModules(true, [](const Err&, std::vector<debug_ipc::Module>) {});
  }
}

void Session::DispatchNotifyProcessExiting(const debug_ipc::NotifyProcessExiting& notify) {
  if (Process* process = system_.ProcessFromKoid(notify.process_koid))
    process->GetTarget()->OnProcessExiting(notify.return_code, notify.timestamp);
}

void Session::DispatchNotifyIO(const debug_ipc::NotifyIO& notify) {
  ProcessImpl* process = system_.ProcessImplFromKoid(notify.process_koid);

  // If there's no process, it's a general IO which should be printed.
  if (!process || process->HandleIO(notify)) {
    LOGS(Info) << notify.data;
  }
}

void Session::DispatchNotifyLog(const debug_ipc::NotifyLog& notify) {
  debug::LogSeverity severity;
  switch (notify.severity) {
    case debug_ipc::NotifyLog::Severity::kDebug:
    case debug_ipc::NotifyLog::Severity::kInfo:
      severity = debug::LogSeverity::kInfo;
      break;
    case debug_ipc::NotifyLog::Severity::kWarn:
      severity = debug::LogSeverity::kWarn;
      break;
    case debug_ipc::NotifyLog::Severity::kError:
      severity = debug::LogSeverity::kError;
      break;
    case debug_ipc::NotifyLog::Severity::kLast:
      FX_NOTREACHED();
      return;
  }
  debug::LogStatement(severity,
                      debug::FileLineFunction(notify.location.file.c_str(), notify.location.line,
                                              notify.location.function.c_str()))
          .stream()
      << notify.log;
}

void Session::DispatchNotifyComponentDiscovered(
    const debug_ipc::NotifyComponentDiscovered& notify) {
  auto filter = system_.CreateNewFilter();
  filter->SetPattern(notify.filter.pattern);
  filter->SetType(notify.filter.type);
  filter->SetJobKoid(notify.filter.job_koid);
  filter->SetWeak(notify.filter.config.weak);
  filter->SetRecursive(notify.filter.config.recursive);
  filter->SetJobOnly(notify.filter.config.job_only);
}

void Session::DispatchNotifyComponentStarting(const debug_ipc::NotifyComponentStarting& notify) {
  if (notify.filter) {
    // Handle older backends that still send us a filter along with this notification. When we get a
    // new filter via this older mechanism, we need to be sure to update the backend.
    auto filter = system_.CreateNewFilter();
    filter->SetFilter(*notify.filter);
  }

  for (auto& observer : component_observers_) {
    observer.OnComponentStarted(notify.component.moniker, notify.component.url);
  }

  system().OnFilterMatches(notify.matching_filters);
}

void Session::DispatchNotifyComponentExiting(const debug_ipc::NotifyComponentExiting& notify) {
  for (auto& observer : component_observers_) {
    observer.OnComponentExited(notify.component.moniker, notify.component.url);
  }
}

void Session::DispatchNotifyTestExited(const debug_ipc::NotifyTestExited& notify) {
  for (auto& observer : component_observers_) {
    observer.OnTestExited(notify.url);
  }
}

void Session::DispatchNotifyFilterCreated(const debug_ipc::NotifyFilterCreated& notify) {
  if (notify.originating_filter_id.Decode().originator == debug_ipc::Filter::Originator::kZxdb) {
    system().CreateNewFilter(notify.filter);
  }
}

void Session::DispatchNotification(const debug_ipc::MsgHeader& header, std::vector<char> data) {
  DEBUG_LOG(Session) << "Got notification: " << debug_ipc::MsgHeader::TypeToString(header.type);
  switch (header.type) {
#define FN(msg_type)                                                    \
  case debug_ipc::MsgHeader::Type::k##msg_type: {                       \
    debug_ipc::msg_type notify;                                         \
    if (debug_ipc::Deserialize(std::move(data), &notify, ipc_version_)) \
      Dispatch##msg_type(notify);                                       \
    break;                                                              \
  }
    FOR_EACH_NOTIFICATION_TYPE(FN)
#undef define
    default:
      FX_NOTREACHED();  // Unexpected notification.
  }
}

ThreadImpl* Session::ThreadImplFromKoid(const debug_ipc::ProcessThreadId& id) {
  ProcessImpl* process = system_.ProcessImplFromKoid(id.process);
  if (!process)
    return nullptr;
  return process->GetThreadImplFromKoid(id.thread);
}

Err Session::ResolvePendingConnection(fxl::RefPtr<PendingConnection> pending,
                                      const debug_ipc::HelloReply& reply,
                                      std::unique_ptr<debug::BufferedFD> buffer) {
  if (pending.get() != pending_connection_.get()) {
    // When the connection doesn't match the pending one, that means the pending connection was
    // cancelled and we should drop the one we just got.
    return Err(ErrType::kCanceled, "Connect operation cancelled.");
  }
  pending_connection_ = nullptr;

  if (Err err = HandleHelloReply(reply); err.has_error()) {
    return err;
  }

  // Success, connect up the stream buffers.
  connection_storage_ = std::move(buffer);

  stream_ = &connection_storage_->stream();
  connection_storage_->set_data_available_callback([this]() { OnStreamReadable(); });
  connection_storage_->set_error_callback([this]() { OnStreamError(); });

  // Simple heuristic to tell if we're connected to the local system.
  // TODO As we extend local debugging support, this will need to get more complex and robust.
  System::Where where = pending->connection_info().host == "localhost" ? System::Where::kLocal
                                                                       : System::Where::kRemote;

  // Connection succeeds.
  system_.DidConnect(where);
  SyncAgentStatus();
  return Err();
}

void Session::SendLocalHello(fit::callback<void(const Err&)> cb) {
  // In order to use the RemoteAPI wrappers, we need to manually set the version first. This is
  // OK since we know the connection is to our same build (either to the built-in debug_agent or to
  // the minidump backend) and has the same version.
  ipc_version_ = debug_ipc::kCurrentProtocolVersion;
  remote_api_->SetVersion(debug_ipc::kCurrentProtocolVersion);
  remote_api_->Hello(debug_ipc::HelloRequest{.version = debug_ipc::kCurrentProtocolVersion},
                     [weak_this = GetWeakPtr(), cb = std::move(cb)](
                         const Err& err, debug_ipc::HelloReply reply) mutable {
                       if (weak_this && !err.has_error()) {
                         if (weak_this->HandleHelloReply(reply).ok()) {
                           weak_this->analytics_reporter_.ReportSessionStarted();
                           weak_this->SyncAgentStatus();
                           weak_this->system_.DidConnect(System::Where::kLocal);
                         }
                         cb(err);
                       }
                     });
}

Err Session::HandleHelloReply(const debug_ipc::HelloReply& reply) {
  // Version check.
  if (reply.version > debug_ipc::kCurrentProtocolVersion ||
      reply.version < debug_ipc::kMinimumProtocolVersion) {
    return Err(
        "The IPC version of the debug_agent on the system (v%u) is not in the supported\n"
        "range of the zxdb frontend (v%u to v%u).",
        reply.version, debug_ipc::kMinimumProtocolVersion, debug_ipc::kCurrentProtocolVersion);
  }

  ipc_version_ = reply.version;
  remote_api_->SetVersion(reply.version);

  // Initialize arch-specific stuff.
  return SetArch(reply.arch, reply.platform, reply.page_size);
}

void Session::SyncAgentStatus() {
  remote_api()->Status(
      debug_ipc::StatusRequest{},
      [this, session = GetWeakPtr()](const Err& err, debug_ipc::StatusReply reply) {
        if (!session)
          return;

        if (err.has_error()) {
          LOGS(Error) << "Could not get debug agent status: " << err.msg();
          return;
        }

        // This code path is called by all entrypoints, whether we are connected to a remote or
        // local DebugAgent or using alternative RemoteAPI implementations (e.g. minidumps or other
        // offline debugging). If we've gotten here, we are connected and in a good state.
        analytics_reporter_.ReportSessionConnected(is_minidump_,
                                                   system_.where() == System::Where::kLocal);

        if (!reply.filters.empty()) {
          for (auto& remote_filter : reply.filters) {
            Filter* client_filter = system().CreateNewFilter();
            client_filter->SetType(remote_filter.type);
            client_filter->SetPattern(remote_filter.pattern);
            client_filter->SetJobKoid(remote_filter.job_koid);
            client_filter->SetWeak(remote_filter.config.weak);
          }
        }

        // Notify about previously connected processes.
        if (!reply.processes.empty()) {
          for (auto& observer : observers_) {
            observer.HandlePreviousConnectedProcesses(reply.processes);
          }
        }

        // Notify about processes on limbo.
        if (!reply.limbo.empty()) {
          for (auto& observer : observers_) {
            observer.HandleProcessesInLimbo(reply.limbo);
          }

          if (auto_attach_limbo_) {
            for (auto& process : reply.limbo) {
              AttachToLimboProcessAndNotify(process.process_koid, process.process_name);
            }
          }
        }

        if (!reply.breakpoints.empty()) {
          for (auto& bp : reply.breakpoints) {
            Breakpoint* client_bp = system().CreateNewBreakpoint();

            BreakpointSettings settings = client_bp->GetSettings();
            settings.name = bp.name;
            settings.type = bp.type;
            settings.one_shot = bp.one_shot;
            settings.instructions = bp.instructions;
            settings.scope = ExecutionScope();

            // TODO(http://b/317387036): There is some information that will be lost about the
            // breakpoint if it had been installed by zxdb before (breakpoint conditions, file/line
            // information, etc) which the target never knows about. The best we can do is use the
            // address that DebugAgent knows about when the breakpoint is installed.
            for (const auto& location : bp.locations) {
              settings.locations.emplace_back(location.address);
            }

            client_bp->SetSettings(settings);
          }
        }
      });
}

void Session::OnSettingChanged(const SettingStore& store, const std::string& setting_name) {
  if (setting_name == ClientSettings::System::kAutoAttachLimbo) {
    auto_attach_limbo_ = system().settings().GetBool(ClientSettings::System::kAutoAttachLimbo);
  } else {
    LOGS(Warn) << "Session handling invalid setting " << setting_name;
  }
}

void Session::ListenForSystemSettings() {
  system_.settings().AddObserver(ClientSettings::System::kAutoAttachLimbo, this);
}

void Session::AttachToLimboProcessAndNotify(uint64_t koid, const std::string& process_name) {
  if (koid_seen_in_limbo_.insert(koid).second) {
    system().AttachToPid(koid, debug_ipc::AttachConfig(),
                         [](fxl::WeakPtr<Target> target, const Err&, uint64_t timestamp) {});

  } else {
    // We've already seen this koid in limbo during this session, alert the user and do not
    // automatically attach.
    LOGS(Info) << "Process " << process_name << " (" << koid
               << ") crashed and is waiting to be attached.\n"
                  "Not automatically attached because "
               << koid
               << " has already been seen this session.\n"
                  "Type \"status\" for more information.";
  }
}

}  // namespace zxdb
