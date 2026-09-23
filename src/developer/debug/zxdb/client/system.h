// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SYSTEM_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SYSTEM_H_

#include <memory>
#include <vector>

#include "lib/fit/function.h"
#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/zxdb/client/client_object.h"
#include "src/developer/debug/zxdb/client/download_manager.h"
#include "src/developer/debug/zxdb/client/download_observer.h"
#include "src/developer/debug/zxdb/client/map_setting_store.h"
#include "src/developer/debug/zxdb/client/session_observer.h"
#include "src/developer/debug/zxdb/client/setting_store_observer.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/symbols/debug_symbol_file_type.h"
#include "src/developer/debug/zxdb/symbols/system_symbols.h"
#include "src/lib/fxl/macros.h"
#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/lib/fxl/observer_list.h"

namespace zxdb {

class Breakpoint;
class BreakpointImpl;
class Err;
class Filter;
class ProcessImpl;
class SymbolServer;
class SystemObserver;
class TargetImpl;

// Represents the client's view of the system-wide state on the debugged
// computer.
class System : public ClientObject,
               public DownloadObserver,
               public SessionObserver,
               public SettingStoreObserver {
 public:
  enum class Where {
    kNone,    // No connection.
    kLocal,   // Connection is to the local system, file paths can be used directly.
    kRemote,  // Remote connection to another computer.
  };

  // Callback for requesting the process tree.
  using ProcessTreeCallback = fit::callback<void(const Err&, debug_ipc::ProcessTreeReply)>;

  explicit System(Session* session);
  ~System() override;

  fxl::WeakPtr<System> GetWeakPtr() { return weak_factory_.GetWeakPtr(); }

  // Returns whether this connection is local or remote.
  Where where() const { return where_; }

  void AddObserver(SystemObserver* observer) { observers_.AddObserver(observer); }
  void RemoveObserver(SystemObserver* observer) { observers_.RemoveObserver(observer); }

  MapSettingStore& settings() { return settings_; }

  // Provides the setting schema for this object.
  static fxl::RefPtr<SettingSchema> GetSchema();

  ProcessImpl* ProcessImplFromKoid(uint64_t koid) const;

  std::vector<TargetImpl*> GetTargetImpls() const;

  // Like CreateNewTarget but returns the implementation.
  TargetImpl* CreateNewTargetImpl(TargetImpl* clone);

  SystemSymbols* GetSymbols() { return &symbols_; }

  DownloadManager* GetDownloadManager() { return &download_manager_; }

  // Returns all targets currently in this System instance. The returned pointers are managed by the
  // System object and should not be cached once you return to the message loop.  There is a single
  // default Target, which is not initially attached to anything.
  std::vector<Target*> GetTargets() const;

  // Returns all [non-]internal breakpoints currently in this System instance. The returned pointers
  // are managed by the System object and should not be cached once you return to the message loop.
  std::vector<Breakpoint*> GetBreakpoints() const;
  std::vector<Breakpoint*> GetInternalBreakpoints() const;

  // Returns all filters currently in this System instance. The returned pointers are managed by the
  // System object and should not be cached once you return to the message loop.
  std::vector<Filter*> GetFilters() const;

  // Returns the filter matching the given id or nullptr if not found. Similarly to |GetFilters|
  // only weak pointers should be cached from the returned pointer and checked for validity before
  // use.
  Filter* GetFilterForId(const debug_ipc::Filter::Identifier& id) const;

  // Returns all symbol servers registered with this symbol instance. The returned pointers are
  // managed by the System object and should not be cached once you return to the message loop.
  std::vector<SymbolServer*> GetSymbolServers() const;

  // Returns the process (and hence Target) associated with the given live koid. Returns 0 if not
  // found.
  Process* ProcessFromKoid(uint64_t koid) const;

  // Schedules a request for the system process tree.
  void GetProcessTree(ProcessTreeCallback callback);

  // Creates/Deletes a target in this System instance. If "clone" is given, the settings from that
  // target will be cloned into the new one. If clone is null, an empty Target will be allocated.
  //
  // Targets can't be deleted if they're running, and there must always be at least one target
  // in the system. Delete will fail otherwise.
  Target* CreateNewTarget(Target* clone);
  Err DeleteTarget(Target* t);

  // Creates a new breakpoint. It will have no associated process or location and will be disabled.
  Breakpoint* CreateNewBreakpoint();

  // Creates an internal breakpoint. Internal breakpoints are not reported by GetBreakpoints() and
  // are used to implement internal stepping functions.
  Breakpoint* CreateNewInternalBreakpoint();

  // Deletes the given breakpoint. The passed-in pointer will be invalid after this call. Used for
  // both internal and external breakpoints.
  void DeleteBreakpoint(Breakpoint* breakpoint);
  // Delete all internal and external breakpoints.
  void DeleteAllBreakpoints();

  // Creates a new filter. It will have no associated pattern.
  Filter* CreateNewFilter(std::optional<debug_ipc::Filter> maybe_filter = std::nullopt);

  // Delete a filter. The passed-in pointer will be invalid after this call.
  void DeleteFilter(Filter* filter);
  // Delete all filters in the system.
  void DeleteAllFilters();

  // Pauses (suspends in Zircon terms) all threads of all attached processes.
  //
  // The backend will try to ensure the threads are actually paused before issuing the on_paused
  // callback. But this is best effort and not guaranteed: both because there's a timeout for the
  // synchronous suspending and because a different continue message could race with the reply.
  void Pause(fit::callback<void()> on_paused);

  // Applies to all threads of all debugged processes.
  void Continue(bool forward);

  // Stops all thread controllers which may be doing automatic stepping for all threads in all
  // processes. See Thread::CancelAllThreadControllers() for more.
  void CancelAllThreadControllers();

  // Whether there's a download pending for the given build ID.
  bool HasDownload(const std::string& build_id);

  // Notification that a connection has been made/terminated to a target system.
  //
  // The is_local flag will be set when the connection is just a loopback to the local computer.
  void DidConnect(Where where);
  void DidDisconnect();

  // Returns the breakpoint implementation for the given ID, or null if the ID was not found in the
  // map. This will include both internal and regular breakpoints (it is used for notification
  // dispatch).
  BreakpointImpl* BreakpointImplForId(uint32_t id);

  // SettingStoreObserver implementation.
  void OnSettingChanged(const SettingStore&, const std::string& setting_name) override;

  // Called when all downloads for |build_id| fail for all active symbol servers.
  void NotifyFailedToFindDebugSymbols(const Err& err, const std::string& build_id,
                                      DebugSymbolFileType file_type);

  // Add a symbol server for testing purposes.
  void InjectSymbolServerForTesting(std::unique_ptr<SymbolServer> server);

  // Returns the next available Target for a test, which might be created on the fly. This is
  // effectively exposing the private method |GetNextTarget| to tests, which is helpful for creating
  // multiple processes.
  TargetImpl* GetNextTargetForTesting();

  // Sync filters to debug_agent.
  void SyncFilters();

  // Will attach to any process we are not already attached to.
  void OnFilterMatches(const std::vector<debug_ipc::FilterMatch>& matches);

  // Searches through for an open slot (Target without an attached process) or creates another one
  // if none is found. Calls attach on that target, passing |callback| into it.
  void AttachToPid(uint64_t pid, debug_ipc::AttachConfig config,
                   Target::CallbackWithTimestamp callback);

  // Detach from all targets, and delete the now unused Target objects. This can be called by user
  // commands (e.g. "detach *", or "quit" when in embedded mode).
  void DetachFromAllTargets(fit::callback<void(int)> cb);

  // Adds a task to be run once any pending downloads are completed. If no downloads are currently
  // in progress, the callback is issued immediately.
  void AddPostDownloadTask(fit::callback<void()> cb);

 private:
  void AddNewTarget(std::unique_ptr<TargetImpl> target);
  void AddSymbolServer(std::unique_ptr<SymbolServer> server);

  // Returns an unused Target or allocates a new one, if there are none available.
  Target* GetNextTarget();

  // SessionObserver implementation.
  void HandlePreviousConnectedProcesses(
      const std::vector<debug_ipc::ProcessRecord>& procs) override;

  // DownloadObserver implementation.
  void OnDownloadsStopped(size_t num_succeeded, size_t num_failed) override;

  std::vector<fit::callback<void()>> post_download_tasks_;

  Where where_ = Where::kNone;

  std::vector<std::unique_ptr<SymbolServer>> symbol_servers_;
  std::vector<std::unique_ptr<TargetImpl>> targets_;

  // The breakpoints are indexed by their unique backend ID. This is separate from the index
  // generated by the console frontend to describe the breakpoint noun.
  std::map<uint32_t, std::unique_ptr<BreakpointImpl>> breakpoints_;

  std::vector<std::unique_ptr<Filter>> filters_;
  bool filter_sync_pending_ = false;  // Used to throttle consecutive OnFilterChanges.

  fxl::ObserverList<SystemObserver> observers_;

  DownloadManager download_manager_;
  SystemSymbols symbols_;

  MapSettingStore settings_;

  fxl::WeakPtrFactory<System> weak_factory_;

  FXL_DISALLOW_COPY_AND_ASSIGN(System);
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SYSTEM_H_
