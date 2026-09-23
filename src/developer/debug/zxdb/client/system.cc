// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/system.h"

#include <functional>
#include <lib/syslog/cpp/log_settings.h>

#include <algorithm>
#include <filesystem>
#include <iterator>
#include <set>

#include "src/developer/debug/ipc/filter_utils.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/zxdb/client/breakpoint_impl.h"
#include "src/developer/debug/zxdb/client/download_manager.h"
#include "src/developer/debug/zxdb/client/download_observer.h"
#include "src/developer/debug/zxdb/client/exception_settings.h"
#include "src/developer/debug/zxdb/client/filter.h"
#include "src/developer/debug/zxdb/client/process_impl.h"
#include "src/developer/debug/zxdb/client/remote_api.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/client/symbol_server.h"
#include "src/developer/debug/zxdb/client/system_observer.h"
#include "src/developer/debug/zxdb/client/target_impl.h"
#include "src/developer/debug/zxdb/client/thread_impl.h"
#include "src/developer/debug/zxdb/common/join_callbacks.h"
#include "src/developer/debug/zxdb/common/string_util.h"
#include "src/developer/debug/zxdb/expr/vector_register_format.h"
#include "src/developer/debug/zxdb/symbols/loaded_module_symbols.h"
#include "src/developer/debug/zxdb/symbols/module_symbol_status.h"
#include "src/developer/debug/zxdb/symbols/module_symbols.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

// Schema definition -------------------------------------------------------------------------------

const char* ClientSettings::System::kAutoCastToDerived = "auto-cast-to-derived";
static const char* kAutoCastToDerivedDescription =
    R"(  Automatically cast pointers and references to the final derived class when
  possible.

  When a class has virtual members, zxdb can use the vtable information to
  deduce the specific derived class for the object. This affects printing and
  the resolution of class/struct members in expressions.)";

const char* ClientSettings::System::kDebugMode = "debug-mode";
static const char* kDebugModeDescription =
    R"(  Output debug information about zxdb.
  In general should only be useful for people developing zxdb.)";

const char* ClientSettings::System::kAutoAttachLimbo = "auto-attach-limbo";
static const char* kAutoAttachLimboDescription =
    R"(  Automatically attach to processes found in Process Limbo.)";

const char* ClientSettings::System::kShowFilePaths = "show-file-paths";
static const char* kShowFilePathsDescription =
    R"(  Displays full path information when file names are displayed. Otherwise
  file names will be shortened to the shortest unique name in the current
  process.)";

const char* ClientSettings::System::kShowStdout = "show-stdout";
static const char* kShowStdoutDescription =
    R"(  Whether newly debugged process (either launched or attached) should
  output it's stdout/stderr to zxdb. This setting is global but can be overridden
  by each individual process.)";

const char* ClientSettings::System::kConsoleMode = "console-mode";
static const char* kConsoleModeDescription =
    R"(  The style in which zxdb interacts with the console. Valid values are
  "shell", "embedded", "embedded-interactive", and "non-interactive".)";
const char* ClientSettings::System::kConsoleMode_Shell = "shell";
const char* ClientSettings::System::kConsoleMode_Embedded = "embedded";
const char* ClientSettings::System::kConsoleMode_EmbeddedInteractive = "embedded-interactive";
const char* ClientSettings::System::kConsoleMode_NonInteractive = "non-interactive";
const char* ClientSettings::System::kEmbeddedModeContext = "embedded-mode-context";
static const char* kEmbeddedModeContextDescription =
    R"(  A short contextual string shown to the user when zxdb breaks out of
    Embedded mode. This should typically be something that the user would expect
    to need to debug e.g. "test failure" or "crash". If unspecified, the
    exception that caused the debugger to appear will be shown.)";

const char* ClientSettings::System::kLanguage = "language";
static const char* kLanguageDescription =
    R"(  Programming language for expressions given to commands such as print.
  Valid values are "c++", "rust", and "auto". Most of the time you'll want to
  set this to "auto" and let zxdb determine the language of the current unit.)";
const char* ClientSettings::System::kLanguage_Cpp = "c++";
const char* ClientSettings::System::kLanguage_Rust = "rust";
const char* ClientSettings::System::kLanguage_Auto = "auto";

const char* ClientSettings::System::kSecondChanceExceptions = "second-chance-exceptions";
static const char* kSecondChanceExceptionsDescription =
    R"(  List of exception types that should be handled as second-chance by
  default; anything not in this list will be handled as first-chance. For
  brevity two-to-three letter shorthands are used represent the types; valid
  shorthands are:

   • "gen": general
   • "pf": page faults
   • "ui": undefined instruction
   • "ua": unaligned access
   • "pe": policy error)";

const char* ClientSettings::System::kSkipUnsymbolized = "skip-unsymbolized";
static const char* kSkipUnsymbolizedDescription =
    R"(  When true, the "step" command will automatically skip over unsymbolized
  function calls. When false, it will stop.)";

// Symbol lookup.
const char* ClientSettings::System::kSymbolIndexFiles = "symbol-index-files";
static const char* kSymbolIndexFilesDescription =
    R"(  List of symbol-index files for symbol lookup. The content will be used
  to populate the "ids-txts", "build-id-dirs", "public-symbol-servers", and
  "private-symbol-servers" settings. Check the "symbol-index" host tool for more
  information.)";

const char* ClientSettings::System::kSymbolIndexInclude = "symbol-index-include";
static const char* kSymbolIndexIncludeDescription =
    R"(  Strings containing additional symbol index content, encoded as JSON.
  The content will be used to populate the "ids-txts", "build-id-dirs",
  "public-symbol-servers", and "private-symbol-servers" settings. Check the
  "symbol-index" host tool for more information.)";

const char* ClientSettings::System::kSymbolPaths = "symbol-paths";
static const char* kSymbolPathsDescription =
    R"(  List of ELF files or directories for symbol lookup. When a directory
  path is passed, the directory will be enumerated non-recursively to index all
  ELF files within. When a file is passed, it will be loaded as an ELF file.

  To view the currently indexed files run "sym-stat --dump-index".)";

const char* ClientSettings::System::kBuildIdDirs = "build-id-dirs";
static const char* kBuildIdDirsDescription =
    R"(  List of ".build-id" directories for symbol lookup. Each directory is assumed to
  contain a ".build-id"-style index of symbol files, that is, each symbol file
  lives at xx/yyyyyyyy.debug where xx is the first two characters of the build
  ID and yyyyyyyy is the rest. However, the name of the directory doesn't need
  to be .build-id.)";

const char* ClientSettings::System::kIdsTxts = "ids-txts";
static const char* kIdsTxtsDescription =
    R"(  List of "ids.txt" files for symbol lookup. Each file, typically named
  "ids.txt", serves as a mapping from build ID to symbol file path and should
  contain multiple lines in the format of "<build ID> <file path>".

  To view the currently indexed files run "sym-stat --dump-index".)";

const char* ClientSettings::System::kPrivateSymbolServers = "private-symbol-servers";
static const char* kPrivateSymbolServersDescription =
    R"(  List of all private symbol server URLs.)";

const char* ClientSettings::System::kPublicSymbolServers = "public-symbol-servers";
static const char* kPublicSymbolServersDescription = R"(  List of all public symbol server URLs.)";

const char* ClientSettings::System::kSymbolCache = "symbol-cache";
static const char* kSymbolCacheDescription =
    R"(  Directory where we can keep a symbol cache. If a symbol server has been
  specified, downloaded symbols will be stored in this directory. The directory
  structure will be the same as a .build-id directory.)";

const char* ClientSettings::System::kEnableAnalytics = "enable-analytics";
static const char* kEnableAnalyticsDescription = R"(  Whether collection of analytics is enabled.)";

const char* ClientSettings::System::kUiTimeoutMs = "ui-timeout-ms";
static const char* kUiTimeoutMsDescription =
    R"(  Time in milliseconds for how long to wait for a command to complete before
  running it in the background and showing the prompt again. Setting this value
  to 0 makes all commands run asynchronously without blocking the input.)";

const char* ClientSettings::System::kContextLinesAfter = "context-lines-after";
static const char* kContextLinesAfterDescription =
    R"(  Number of lines of source code to show below the current line when
  stopping. This setting has priority over "context-lines" and can be
  set independently.)";

const char* ClientSettings::System::kContextLinesBefore = "context-lines-before";
static const char* kContextLinesBeforeDescription =
    R"(  Number of lines of source code to show above the current line when
  stopping. This setting has priority over "context-lines" and can be
  set independently.)";

const char* ClientSettings::System::kContextLines = "context-lines";
static const char* kContextLinesDescription =
    R"(  Number of lines of source code to show above and below the current
  line when stopping. In other words, this is a convenience setter that sets
  both "context-lines-after" and "context-lines-before", overwriting their
  current values.)";

namespace {

fxl::RefPtr<SettingSchema> CreateSchema() {
  auto schema = fxl::MakeRefCounted<SettingSchema>();

  schema->AddBool(ClientSettings::System::kAutoCastToDerived, kAutoCastToDerivedDescription, true);
  schema->AddBool(ClientSettings::System::kDebugMode, kDebugModeDescription, false);
  schema->AddBool(ClientSettings::System::kAutoAttachLimbo, kAutoAttachLimboDescription, true);
  schema->AddBool(ClientSettings::System::kShowFilePaths, kShowFilePathsDescription, false);
  schema->AddBool(ClientSettings::System::kShowStdout, kShowStdoutDescription, true);
  schema->AddString(
      ClientSettings::System::kConsoleMode, kConsoleModeDescription,
      ClientSettings::System::kConsoleMode_Shell,
      {ClientSettings::System::kConsoleMode_Shell, ClientSettings::System::kConsoleMode_Embedded,
       ClientSettings::System::kConsoleMode_EmbeddedInteractive,
       ClientSettings::System::kConsoleMode_NonInteractive});
  schema->AddString(ClientSettings::System::kEmbeddedModeContext, kEmbeddedModeContextDescription,
                    "");
  schema->AddString(ClientSettings::System::kLanguage, kLanguageDescription, "auto",
                    {"rust", "c++", "auto"});
  schema->AddList(ClientSettings::System::kSecondChanceExceptions,
                  kSecondChanceExceptionsDescription,
                  {kPageFaultExcpTypeShorthand, kPolicyErrorExcpTypeShorthand},
                  {
                      kGeneralExcpTypeShorthand,
                      kPageFaultExcpTypeShorthand,
                      kUndefinedInstructionExcpTypeShorthand,
                      kUnalignedAccessExcpTypeShorthand,
                      kPolicyErrorExcpTypeShorthand,
                  });
  schema->AddBool(ClientSettings::System::kSkipUnsymbolized, kSkipUnsymbolizedDescription, true);

  // Symbol lookup.
  schema->AddList(ClientSettings::System::kSymbolIndexFiles, kSymbolIndexFilesDescription, {});
  schema->AddList(ClientSettings::System::kSymbolIndexInclude, kSymbolIndexIncludeDescription, {});
  schema->AddList(ClientSettings::System::kSymbolPaths, kSymbolPathsDescription, {});
  schema->AddList(ClientSettings::System::kBuildIdDirs, kBuildIdDirsDescription, {});
  schema->AddList(ClientSettings::System::kIdsTxts, kIdsTxtsDescription, {});
  schema->AddList(ClientSettings::System::kPrivateSymbolServers, kPrivateSymbolServersDescription,
                  {});
  schema->AddList(ClientSettings::System::kPublicSymbolServers, kPublicSymbolServersDescription,
                  {});
  schema->AddString(ClientSettings::System::kSymbolCache, kSymbolCacheDescription, "");
  schema->AddList(ClientSettings::Target::kSourceMap,
                  ClientSettings::Target::kSourceMapDescription);

  // Target ones.
  schema->AddString(
      ClientSettings::Target::kVectorFormat, ClientSettings::Target::kVectorFormatDescription,
      kVectorRegisterFormatStr_Double, ClientSettings::Target::GetVectorFormatOptions());
  schema->AddBool(ClientSettings::Target::kAutoContinueWhenStepping,
                  ClientSettings::Target::kAutoContinueWhenSteppingDescription, true);

  // Thread ones.
  schema->AddBool(ClientSettings::Thread::kDebugStepping,
                  ClientSettings::Thread::kDebugSteppingDescription, false);
  schema->AddList(ClientSettings::Thread::kDisplay, ClientSettings::Thread::kDisplayDescription);

  // The code that handles opt-in/out will set this explicitly.
  schema->AddBool(ClientSettings::System::kEnableAnalytics, kEnableAnalyticsDescription, false);

  // Set the UI timeout default to 1 second.
  schema->AddInt(ClientSettings::System::kUiTimeoutMs, kUiTimeoutMsDescription, 1000);

  // Set the number of source code lines to show when we stop.
  schema->AddInt(ClientSettings::System::kContextLinesAfter, kContextLinesAfterDescription, 2);
  schema->AddInt(ClientSettings::System::kContextLinesBefore, kContextLinesBeforeDescription, 2);
  schema->AddInt(ClientSettings::System::kContextLines, kContextLinesDescription, 2);

  return schema;
}

}  // namespace

// System Implementation ---------------------------------------------------------------------------

System::System(Session* session)
    : ClientObject(session),
      download_manager_(this),
      symbols_(std::bind_front(&DownloadManager::RequestDownload, &download_manager_)),
      settings_(GetSchema(), nullptr),
      weak_factory_(this) {
  // Create the default target.
  AddNewTarget(std::make_unique<TargetImpl>(this));

  session->AddObserver(this);
  session->AddDownloadObserver(this);

  // The system is the one holding the system symbols and is the one who will be updating the
  // symbols once we get a symbol change, so the System will be listening to its own options. We
  // don't use SystemSymbols because they live in the symbols library and we don't want it to have a
  // client dependency.
  settings_.AddObserver(ClientSettings::System::kDebugMode, this);
  settings_.AddObserver(ClientSettings::System::kSymbolIndexFiles, this);
  settings_.AddObserver(ClientSettings::System::kSymbolCache, this);
  settings_.AddObserver(ClientSettings::System::kSymbolPaths, this);
  settings_.AddObserver(ClientSettings::System::kBuildIdDirs, this);
  settings_.AddObserver(ClientSettings::System::kIdsTxts, this);
  settings_.AddObserver(ClientSettings::System::kPrivateSymbolServers, this);
  settings_.AddObserver(ClientSettings::System::kPublicSymbolServers, this);
  settings_.AddObserver(ClientSettings::System::kSecondChanceExceptions, this);
  settings_.AddObserver(ClientSettings::System::kContextLines, this);
}

System::~System() {
  // Target destruction may depend on the symbol system. Ensure the targets get cleaned up first.
  for (auto& target : targets_) {
    // It's better if process destruction notifications are sent before target ones because the
    // target owns the process. Because this class sends the target notifications, force the process
    // destruction before doing anything.
    target->ImplicitlyDetach();
    for (auto& observer : session()->target_observers())
      observer.WillDestroyTarget(target.get());
  }

  targets_.clear();

  session()->RemoveDownloadObserver(this);
  session()->RemoveObserver(this);
}

fxl::RefPtr<SettingSchema> System::GetSchema() {
  // Will only run initialization once.
  InitializeSchemas();
  static fxl::RefPtr<SettingSchema> schema = CreateSchema();
  return schema;
}

ProcessImpl* System::ProcessImplFromKoid(uint64_t koid) const {
  for (const auto& target : targets_) {
    ProcessImpl* process = target->process();
    if (process && process->GetKoid() == koid)
      return process;
  }
  return nullptr;
}

std::vector<TargetImpl*> System::GetTargetImpls() const {
  std::vector<TargetImpl*> result;
  for (const auto& t : targets_)
    result.push_back(t.get());
  return result;
}

TargetImpl* System::CreateNewTargetImpl(TargetImpl* clone) {
  auto target = clone ? clone->Clone(this) : std::make_unique<TargetImpl>(this);
  TargetImpl* to_return = target.get();
  AddNewTarget(std::move(target));
  return to_return;
}

std::vector<Target*> System::GetTargets() const {
  std::vector<Target*> result;
  result.reserve(targets_.size());
  for (const auto& t : targets_)
    result.push_back(t.get());
  return result;
}

std::vector<Breakpoint*> System::GetBreakpoints() const {
  std::vector<Breakpoint*> result;
  result.reserve(breakpoints_.size());
  for (const auto& pair : breakpoints_) {
    if (!pair.second->is_internal())
      result.push_back(pair.second.get());
  }
  return result;
}

std::vector<Breakpoint*> System::GetInternalBreakpoints() const {
  std::vector<Breakpoint*> result;
  result.reserve(breakpoints_.size());
  for (const auto& pair : breakpoints_) {
    if (pair.second->is_internal())
      result.push_back(pair.second.get());
  }
  return result;
}

std::vector<Filter*> System::GetFilters() const {
  std::vector<Filter*> result;
  result.reserve(filters_.size());
  for (const auto& filter : filters_) {
    result.push_back(filter.get());
  }
  return result;
}

std::vector<SymbolServer*> System::GetSymbolServers() const {
  std::vector<SymbolServer*> result;
  result.reserve(symbol_servers_.size());
  for (const auto& item : symbol_servers_) {
    result.push_back(item.get());
  }
  return result;
}

void System::NotifyFailedToFindDebugSymbols(const Err& err, const std::string& build_id,
                                            DebugSymbolFileType file_type) {
  for (const auto& target : targets_) {
    // Notify only those targets which are processes and which have attempted and failed to load
    // symbols for this build ID previously.
    auto process = target->process();
    if (!process)
      continue;

    for (const auto& status : process->GetSymbols()->GetStatus()) {
      if (status.build_id != build_id) {
        continue;
      }

      if (!err.has_error()) {
        if (file_type == DebugSymbolFileType::kDebugInfo) {
          process->OnSymbolLoadFailure(Err(
              fxl::StringPrintf("Could not load symbols for \"%s\" because there was no mapping "
                                "for build ID \"%s\".",
                                status.name.c_str(), status.build_id.c_str())));
        } else {
          process->OnSymbolLoadFailure(
              Err(fxl::StringPrintf("Could not load binary for \"%s\" because there was no mapping "
                                    "for build ID \"%s\".",
                                    status.name.c_str(), status.build_id.c_str())));
        }
      } else {
        process->OnSymbolLoadFailure(err);
      }
    }
  }
}

Process* System::ProcessFromKoid(uint64_t koid) const { return ProcessImplFromKoid(koid); }

void System::GetProcessTree(ProcessTreeCallback callback) {
  session()->remote_api()->ProcessTree(debug_ipc::ProcessTreeRequest(), std::move(callback));
}

Target* System::CreateNewTarget(Target* clone) {
  return CreateNewTargetImpl(static_cast<TargetImpl*>(clone));
}

Err System::DeleteTarget(Target* t) {
  if (targets_.size() == 1)
    return Err("Can't delete the last target.");
  if (t->GetState() != Target::kNone)
    return Err("Can't delete a process that's currently attached, detached, or starting.");

  for (auto& observer : session()->target_observers())
    observer.WillDestroyTarget(t);

  auto found =
      std::find_if(targets_.begin(), targets_.end(), [t](const auto& a) { return t == a.get(); });
  FX_DCHECK(found != targets_.end());
  targets_.erase(found);

  return Err();
}

TargetImpl* System::GetNextTargetForTesting() { return static_cast<TargetImpl*>(GetNextTarget()); }

Breakpoint* System::CreateNewBreakpoint() {
  auto owning = std::make_unique<BreakpointImpl>(session(), false);
  uint32_t id = owning->backend_id();
  Breakpoint* to_return = owning.get();

  breakpoints_[id] = std::move(owning);

  // Notify observers (may mutate breakpoint list).
  for (auto& observer : observers_)
    observer.DidCreateBreakpoint(to_return);

  return to_return;
}

Breakpoint* System::CreateNewInternalBreakpoint() {
  auto owning = std::make_unique<BreakpointImpl>(session(), true);
  uint32_t id = owning->backend_id();
  Breakpoint* to_return = owning.get();

  breakpoints_[id] = std::move(owning);
  return to_return;
}

void System::DeleteBreakpoint(Breakpoint* breakpoint) {
  BreakpointImpl* impl = static_cast<BreakpointImpl*>(breakpoint);
  auto found = breakpoints_.find(impl->backend_id());
  if (found == breakpoints_.end()) {
    // Should always have found the breakpoint.
    FX_NOTREACHED();
    return;
  }

  // Only notify observers for non-internal breakpoints.
  if (!found->second->is_internal()) {
    for (auto& observer : observers_)
      observer.WillDestroyBreakpoint(breakpoint);
  }
  breakpoints_.erase(found);
}

void System::DeleteAllBreakpoints() {
  for (auto& bp : breakpoints_) {
    // Only notify for non-internal breakpoints.
    if (!bp.second->is_internal()) {
      for (auto& observer : observers_)
        observer.WillDestroyBreakpoint(bp.second.get());
    }
  }

  breakpoints_.clear();
}

Filter* System::CreateNewFilter(std::optional<debug_ipc::Filter> maybe_filter) {
  Filter* new_filter =
      filters_.emplace_back(std::make_unique<Filter>(session(), maybe_filter)).get();

  FX_DCHECK(new_filter);

  // Notify observers (may mutate filter list).
  for (auto& observer : observers_)
    observer.DidCreateFilter(new_filter);

  return new_filter;
}

void System::DeleteFilter(Filter* filter) {
  auto found = filters_.begin();
  for (; found != filters_.end(); ++found) {
    if (found->get() == filter) {
      break;
    }
  }

  if (found == filters_.end()) {
    // Should always have found the filter.
    FX_NOTREACHED();
    return;
  }

  for (auto& observer : observers_)
    observer.WillDestroyFilter(filter);

  filters_.erase(found);

  SyncFilters();
}

void System::DeleteAllFilters() {
  for (auto& filter : filters_) {
    for (auto& observer : observers_) {
      observer.WillDestroyFilter(filter.get());
    }
  }

  filters_.clear();

  SyncFilters();
}

void System::Pause(fit::callback<void()> on_paused) {
  debug_ipc::PauseRequest request;  // Unset process/thread means everything.
  session()->remote_api()->Pause(
      request, [weak_system = weak_factory_.GetWeakPtr(), on_paused = std::move(on_paused)](
                   const Err&, debug_ipc::PauseReply reply) mutable {
        if (weak_system) {
          // Save the newly paused thread metadata. This may need to be
          // generalized if we add other messages that update thread metadata.
          for (const auto& record : reply.threads) {
            if (auto* process = weak_system->ProcessImplFromKoid(record.id.process)) {
              if (auto* thread = process->GetThreadImplFromKoid(record.id.thread))
                thread->SetMetadata(record);
            }
          }
        }
        on_paused();
      });
}

void System::Continue(bool forward) {
  // Tell each process to continue as it desires.
  //
  // It would be more efficient to tell the backend to resume all threads in all processes but the
  // Thread client objects have state which needs to be updated (like the current stack) and the
  // thread could have a controller that wants to continue in a specific way (like single-step or
  // step in a range).
  for (const auto& target : targets_) {
    if (Process* process = target->GetProcess())
      process->Continue(forward);
  }
}

void System::CancelAllThreadControllers() {
  for (const auto& target : targets_) {
    if (Process* process = target->GetProcess())
      process->CancelAllThreadControllers();
  }
}

void System::DidConnect(Where where) {
  where_ = where;

  // Force reload the symbol mappings after connection. This needs to be done for every connection
  // since a new image could have been compiled and launched which will have a different build ID
  // file.
  symbols_.build_id_index().ClearCache();

  // Force the debug agent to reload its second-chance exception handling policy.
  OnSettingChanged(settings(), ClientSettings::System::kSecondChanceExceptions);

  // When debugging locally, fall back to using symbols from the local modules themselves if not
  // found in the normal symbol locations.
  GetSymbols()->set_enable_local_fallback(where == Where::kLocal);

#if defined(__linux__)
  if (where == Where::kLocal) {
    std::string kLocalSystemSymbolPath("/usr/lib/debug/.build-id");

    // Add the default location for local system symbols if it's not already there.
    auto build_id_dirs = settings_.GetList(ClientSettings::System::kBuildIdDirs);
    if (std::find(build_id_dirs.begin(), build_id_dirs.end(), kLocalSystemSymbolPath) ==
        build_id_dirs.end()) {
      build_id_dirs.push_back(kLocalSystemSymbolPath);
      settings_.SetList(ClientSettings::System::kBuildIdDirs, build_id_dirs);
    }
  }
#endif
}

void System::DidDisconnect() {
  // The logic here should be consistent with debug_agent::DebugAgent::Disconnect().
  for (auto& target : targets_)
    target->ImplicitlyDetach();
}

BreakpointImpl* System::BreakpointImplForId(uint32_t id) {
  auto found = breakpoints_.find(id);
  if (found == breakpoints_.end())
    return nullptr;
  return found->second.get();
}

void System::AddNewTarget(std::unique_ptr<TargetImpl> target) {
  Target* for_observers = target.get();

  targets_.push_back(std::move(target));
  for (auto& observer : session()->target_observers())
    observer.DidCreateTarget(for_observers);
}

void System::OnSettingChanged(const SettingStore& store, const std::string& setting_name) {
  // If any of them change, we have to reinitialize the build_id_index.
  if (setting_name == ClientSettings::System::kSymbolIndexFiles ||
      setting_name == ClientSettings::System::kSymbolPaths ||
      setting_name == ClientSettings::System::kBuildIdDirs ||
      setting_name == ClientSettings::System::kIdsTxts ||
      setting_name == ClientSettings::System::kSymbolCache ||
      setting_name == ClientSettings::System::kPrivateSymbolServers ||
      setting_name == ClientSettings::System::kPublicSymbolServers) {
    // Clear the symbol sources and add them back to sync the index with the setting.
    BuildIDIndex& build_id_index = GetSymbols()->build_id_index();
    build_id_index.ClearAll();

    // Add symbol-index files first. Because they might encode extra information, e.g., build_dir
    // and require_authentication.
    for (const std::string& contents : store.GetList(ClientSettings::System::kSymbolIndexInclude)) {
      // In practice, symbol indices from raw strings that have relative paths will be
      // rather brittle and probably should not be used. Treat them as root-relative to force
      // use of absolute paths.
      build_id_index.AddSymbolIndex(contents, "/");
    }
    for (const std::string& path : store.GetList(ClientSettings::System::kSymbolIndexFiles)) {
      build_id_index.AddSymbolIndexFile(path);
    }
    for (const std::string& path : store.GetList(ClientSettings::System::kSymbolPaths)) {
      build_id_index.AddPlainFileOrDir(path);
    }
    for (const std::string& path : store.GetList(ClientSettings::System::kBuildIdDirs)) {
      build_id_index.AddBuildIdDir(path);
    }
    for (const std::string& path : store.GetList(ClientSettings::System::kIdsTxts)) {
      build_id_index.AddIdsTxt(path);
    }
    for (const std::string& url : store.GetList(ClientSettings::System::kPrivateSymbolServers)) {
      build_id_index.AddSymbolServer(url, true);
    }
    for (const std::string& url : store.GetList(ClientSettings::System::kPublicSymbolServers)) {
      build_id_index.AddSymbolServer(url, false);
    }

    // Cache directory.
    auto symbol_cache = store.GetString(ClientSettings::System::kSymbolCache);
    if (!symbol_cache.empty()) {
      std::error_code ec;
      std::filesystem::create_directories(std::filesystem::path(symbol_cache), ec);
      build_id_index.SetCacheDir(symbol_cache);
    }

    // Symbol servers.
    // TODO(dangyi): We don't support the removal of an existing symbol server yet.
    std::set<std::string> existing;
    for (const auto& symbol_server : symbol_servers_) {
      existing.insert(symbol_server->name());
    }

    // TODO(dangyi): Separate symbol downloading from System into a new DownloadManager which is to
    // be owned by BuildIDIndex.
    for (const auto& server : build_id_index.symbol_servers()) {
      if (existing.find(server.url) == existing.end()) {
        if (auto symbol_server =
                SymbolServer::FromURL(session(), server.url, server.require_authentication)) {
          AddSymbolServer(std::move(symbol_server));
        }
      }
    }
  } else if (setting_name == ClientSettings::System::kDebugMode) {
    debug::SetDebugLogging(store.GetBool(setting_name));
  } else if (setting_name == ClientSettings::System::kSecondChanceExceptions) {
    debug_ipc::UpdateGlobalSettingsRequest request;
    auto updates = ParseExceptionStrategyUpdates(store.GetList(setting_name));
    if (updates.has_error()) {
      // TODO: handle me.
      return;
    }
    request.exception_strategies = updates.value();
    session()->remote_api()->UpdateGlobalSettings(
        request, [](const Err& err, debug_ipc::UpdateGlobalSettingsReply reply) {
          if (reply.status.has_error()) {
            // TODO: handle me.
          }
        });
  } else if (setting_name == ClientSettings::System::kContextLines) {
    const int64_t lines = store.GetInt(ClientSettings::System::kContextLines);
    settings().SetInt(ClientSettings::System::kContextLinesAfter, lines);
    settings().SetInt(ClientSettings::System::kContextLinesBefore, lines);
  } else {
    LOGS(Warn) << "Unhandled setting change: " << setting_name;
  }
}

void System::InjectSymbolServerForTesting(std::unique_ptr<SymbolServer> server) {
  AddSymbolServer(std::move(server));
}

void System::SyncFilters() {
  filter_sync_pending_ = true;
  debug::MessageLoop::Current()->PostTask(FROM_HERE, [weak_this = weak_factory_.GetWeakPtr()]() {
    if (!weak_this || !weak_this->filter_sync_pending_)
      return;
    weak_this->filter_sync_pending_ = false;

    debug_ipc::UpdateFilterRequest request;
    for (const auto& filter : weak_this->filters_) {
      if (filter->is_valid()) {
        request.filters.push_back(filter->filter());
      }
    }
    weak_this->session()->remote_api()->UpdateFilter(
        request, [weak_this](const Err& err, debug_ipc::UpdateFilterReply reply) {
          if (weak_this && !reply.matched_processes_for_filter.empty()) {
            weak_this->OnFilterMatches(reply.matched_processes_for_filter);
          }
        });
  });
}

void System::OnFilterMatches(const std::vector<debug_ipc::FilterMatch>& matches) {
  std::vector<const debug_ipc::Filter*> ipc_filters;
  ipc_filters.reserve(GetFilters().size());
  std::ranges::transform(
      GetFilters(), std::back_inserter(ipc_filters),
      [](const Filter* filter) -> const debug_ipc::Filter* { return &filter->filter(); });

  // A collection of pids that we are going to attach to. The corresponding config will determine
  // the details of the attach. If a pid is matched by multiple filters, they must ALL be configured
  // as weak filters for a weak attach to occur. Job only filters don't have this problem because
  // they won't collide.
  auto pids_to_attach = debug_ipc::GetAttachConfigsForFilterMatches(matches, ipc_filters);

  // Check that we don't accidentally attach to too many processes.
  if (pids_to_attach.size() > 50) {
    LOGS(Error) << "Filter matches too many (" << pids_to_attach.size()
                << ") processes. No attach is performed.";
    return;
  }

  // Now we can attach to all of the matched pids.
  for (const auto& [pid, config] : pids_to_attach) {
    // If we found an already attached process, we don't care about this match.
    if (ProcessFromKoid(pid)) {
      continue;
    }
    AttachToPid(pid, config,
                [pid](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) {
                  if (err.has_error()) {
                    LOGS(Error) << "Could not attach to process " << pid << ": " << err.msg();
                    return;
                  }
                });
  }
}

Filter* System::GetFilterForId(const debug_ipc::Filter::Identifier& id) const {
  if (filters_.empty())
    return nullptr;

  const auto& filter = std::find_if(
      filters_.begin(), filters_.end(),
      [id](const std::unique_ptr<Filter>& filter) { return id == filter->filter().id; });
  return filter != filters_.end() ? filter->get() : nullptr;
}

Target* System::GetNextTarget() {
  Target* open_slot = nullptr;

  // See if there is a target that is not attached.
  for (auto& target : targets_) {
    if (target->GetState() == zxdb::Target::State::kNone) {
      open_slot = target.get();
      break;
    }
  }

  // If no slot was found, we create a new target.
  if (!open_slot)
    open_slot = CreateNewTarget(nullptr);

  return open_slot;
}

void System::AttachToPid(uint64_t pid, debug_ipc::AttachConfig config,
                         Target::CallbackWithTimestamp callback) {
  // Don't allow attaching to a process more than once.
  if (Process* process = ProcessFromKoid(pid)) {
    debug::MessageLoop::Current()->PostTask(
        FROM_HERE, [callback = std::move(callback),
                    weak_target = process->GetTarget()->GetWeakPtr(), pid]() mutable {
          callback(weak_target,
                   Err("Process " + std::to_string(pid) + " is already being debugged."), 0);
        });
    return;
  }

  GetNextTarget()->Attach(pid, config, std::move(callback));
}

void System::HandlePreviousConnectedProcesses(const std::vector<debug_ipc::ProcessRecord>& procs) {
  for (const auto& proc : procs) {
    GetNextTarget()->AssignPreviousConnectedProcess(proc);
  }
}

void System::OnDownloadsStopped(size_t num_succeeded, size_t num_failed) {
  for (auto& task : post_download_tasks_) {
    debug::MessageLoop::Current()->PostTask(FROM_HERE, [cb = std::move(task)]() mutable { cb(); });
  }
}

void System::AddSymbolServer(std::unique_ptr<SymbolServer> unique_server) {
  SymbolServer* server = unique_server.get();
  symbol_servers_.push_back(std::move(unique_server));

  for (auto& observer : observers_) {
    observer.DidCreateSymbolServer(server);
  }

  server->set_state_change_callback([weak_this = weak_factory_.GetWeakPtr()](
                                        SymbolServer* server, SymbolServer::State state) mutable {
    if (!weak_this) {
      return;
    }

    for (auto& observer : weak_this->observers_)
      observer.OnSymbolServerStatusChanged(server);
  });
}

void System::DetachFromAllTargets(fit::callback<void(int)> cb) {
  struct PackedJoinType {
    fxl::WeakPtr<Target> target;
    Err err;
  };

  auto joiner = fxl::MakeRefCounted<JoinCallbacks<PackedJoinType>>();

  for (auto target : GetTargets()) {
    if (target->GetState() != Target::State::kNone) {
      target->Detach(
          [cb = joiner->AddCallback()](fxl::WeakPtr<Target> target, const Err& err) mutable {
            PackedJoinType pack{std::move(target), err};
            cb(pack);
          });
    }
  }

  joiner->Ready([weak_this = GetWeakPtr(),
                 cb = std::move(cb)](const std::vector<PackedJoinType>& results) mutable {
    if (!weak_this)
      return;

    for (const auto& result : results) {
      FX_DCHECK(result.err.ok());
      // Ignore this return value. The last one will always have a warning since one target must
      // always be present. |DeleteTarget| handles that logic for us.
      weak_this->DeleteTarget(result.target.get());
    }

    cb(results.size());
  });
}

bool System::HasDownload(const std::string& build_id) {
  return download_manager_.HasDownload(build_id);
}

void System::AddPostDownloadTask(fit::callback<void()> cb) {
  if (!download_manager_.DownloadsInProgress()) {
    return cb();
  }

  post_download_tasks_.emplace_back(std::move(cb));
}

}  // namespace zxdb
