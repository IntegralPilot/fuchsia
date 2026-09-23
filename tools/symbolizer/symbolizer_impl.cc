// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/symbolizer/symbolizer_impl.h"

#include <algorithm>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <memory>
#include <string>
#include <utility>

#include <rapidjson/ostreamwrapper.h>
#include <rapidjson/prettywriter.h>
#include <rapidjson/rapidjson.h>

#include "lib/fit/defer.h"
#include "src/developer/debug/ipc/automation_instruction.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/zxdb/client/client_object.h"
#include "src/developer/debug/zxdb/client/frame.h"
#include "src/developer/debug/zxdb/client/pretty_stack_manager.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/client/source_file_provider_impl.h"
#include "src/developer/debug/zxdb/client/stack.h"
#include "src/developer/debug/zxdb/client/symbol_server.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/client/target_impl.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/common/file_util.h"
#include "src/developer/debug/zxdb/format/format_name.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/loaded_module_symbols.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/developer/debug/zxdb/symbols/process_symbols.h"
#include "src/developer/debug/zxdb/symbols/system_symbols.h"
#include "src/developer/debug/zxdb/symbols/target_symbols.h"
#include "src/lib/fxl/memory/ref_ptr.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "tools/symbolizer/symbolizer.h"

namespace symbolizer {

namespace {

void SetupCommandLineOptions(const CommandLineOptions& options, zxdb::MapSettingStore& settings) {
  using Settings = zxdb::ClientSettings;

  const char* symbol_index_from_env = getenv("SYMBOL_INDEX_INCLUDE");
  if (symbol_index_from_env) {
    FX_LOGS(DEBUG) << "Setting symbol index contents from environment variable.";
    settings.SetList(Settings::System::kSymbolIndexInclude, {symbol_index_from_env});
  }

  if (options.symbol_cache) {
    FX_LOGS(DEBUG) << "Setting symbol cache to " << *options.symbol_cache;
    settings.SetString(Settings::System::kSymbolCache, *options.symbol_cache);
  }

  if (!options.symbol_index_files.empty()) {
    for (const auto& file : options.symbol_index_files) {
      FX_LOGS(DEBUG) << "Adding symbol index file " << file;
    }
    settings.SetList(Settings::System::kSymbolIndexFiles, options.symbol_index_files);
  }

  if (!options.private_symbol_servers.empty()) {
    for (const auto& server : options.private_symbol_servers) {
      FX_LOGS(DEBUG) << "Adding private symbol server " << server;
    }
    settings.SetList(Settings::System::kPrivateSymbolServers, options.private_symbol_servers);
  }

  if (!options.public_symbol_servers.empty()) {
    for (const auto& server : options.public_symbol_servers) {
      FX_LOGS(DEBUG) << "Adding public symbol server " << server;
    }
    settings.SetList(Settings::System::kPublicSymbolServers, options.public_symbol_servers);
  }

  if (!options.symbol_paths.empty()) {
    for (const auto& path : options.symbol_paths) {
      FX_LOGS(DEBUG) << "Adding symbol path " << path;
    }
    settings.SetList(Settings::System::kSymbolPaths, options.symbol_paths);
  }

  if (!options.build_id_dirs.empty()) {
    for (const auto& dir : options.build_id_dirs) {
      FX_LOGS(DEBUG) << "Adding build-id dir " << dir;
    }
    settings.SetList(Settings::System::kBuildIdDirs, options.build_id_dirs);
  }

  if (!options.ids_txts.empty()) {
    for (const auto& file : options.ids_txts) {
      FX_LOGS(DEBUG) << "Adding idx.txt " << file;
    }
    settings.SetList(Settings::System::kIdsTxts, options.ids_txts);
  }
}

std::string FormatFrameIdAndAddress(uint64_t frame_id, uint64_t inline_index, uint64_t address) {
  // Frame number.
  std::string out = "   #" + std::to_string(frame_id);

  // Append a sequence for inline frames, i.e. not the last frame.
  if (inline_index) {
    out += "." + std::to_string(inline_index);
  }

  // Pad to 9.
  constexpr int index_width = 9;
  if (out.length() < index_width) {
    out += std::string(index_width - out.length(), ' ');
  }

  // Print the absolute address first.
  out += fxl::StringPrintf("0x%016" PRIx64, address);

  return out;
}

}  // namespace

SymbolizerImpl::SymbolizerImpl(const CommandLineOptions& options)
    : prettify_enabled_(options.prettify_backtrace), omit_module_lines_(options.omit_module_lines) {
  // Hook observers.
  session_.system().AddObserver(this);
  session_.AddDownloadObserver(this);
  session_.AddProcessObserver(this);

  // Disable indexing on ModuleSymbols to accelerate the loading time.
  session_.system().GetSymbols()->set_create_index(false);
  target_ = session_.system().GetTargets()[0];

  FX_LOGS(DEBUG) << "Initializing async loop...";
  loop_.Init(nullptr);
  // Setting symbol servers will trigger an asynchronous network request.
  SetupCommandLineOptions(options, session_.system().settings());
  FX_LOGS(DEBUG) << "CLI options set up.";
  if (waiting_auth_) {
    FX_LOGS(DEBUG) << "Checking for auth...";
    remote_symbol_lookup_enabled_ = true;
    loop_.Run();
    FX_LOGS(DEBUG) << "Auth state updated.";
  }

  // Check and prompt authentication message.
  auto symbol_servers = session_.system().GetSymbolServers();
  if (std::any_of(symbol_servers.begin(), symbol_servers.end(), [](zxdb::SymbolServer* server) {
        return server->state() == zxdb::SymbolServer::State::kAuth;
      })) {
    std::cerr << "WARN: missing authentication for symbol servers. You might want to run "
                 "`ffx debug symbolize --auth`.\n";
  }

  if (options.dumpfile_output) {
    dumpfile_output_ = options.dumpfile_output.value();
    dumpfile_document_.SetArray();
    ResetDumpfileCurrentObject();
  }

  FX_LOGS(DEBUG) << "Making stack manager and loading matchers.";
  pretty_stack_manager_ = fxl::MakeRefCounted<zxdb::PrettyStackManager>();
  pretty_stack_manager_->LoadDefaultMatchers();

  FX_LOGS(DEBUG) << "Creating source file provider.";
  source_file_provider_ = std::make_unique<zxdb::SourceFileProviderImpl>(target_->settings());
}

SymbolizerImpl::~SymbolizerImpl() {
  loop_.Cleanup();

  // Support for dumpfile
  if (!dumpfile_output_.empty()) {
    std::ofstream ofs(dumpfile_output_);
    rapidjson::OStreamWrapper osw(ofs);
    rapidjson::PrettyWriter<rapidjson::OStreamWrapper> writer(osw);
    writer.SetIndent(' ', 2);
    dumpfile_document_.Accept(writer);
  }
}

void SymbolizerImpl::Reset(bool symbolizing_dart, ResetType type) {
  // We got a reset:begin before a previous begin tag had a corresponding end tag. For instance,
  // this can happen when many rust components crash at the same time, with each panic handler
  // outputting the respective process' backtrace. If they become interleaved in the log, then
  // symbolizer cannot disambiguate between the stack traces, and must abandon the old one.
  if (in_batch_mode_ && type == ResetType::kBegin) {
    FlushBufferedFramesWithContext("got a reset:begin tag while processing another backtrace");
  }

  // Try to output upon reset first.
  if (!frames_in_batch_mode_.empty()) {
    OutputBatchedBacktrace();
  }

  symbolizing_dart_ = symbolizing_dart;

  modules_.clear();
  address_to_module_id_.clear();
  if (target_->GetState() == zxdb::Target::State::kRunning) {
    // OnProcessExiting() will destroy the Process, ProcessSymbols.
    // Retain references to loaded TargetSymbols in |previous_modules_| so that they can be
    // potentially reused for the subsequent stack trace.
    previous_modules_ = target_->GetProcess()->GetSymbols()->target_symbols()->TakeModules();
    target_->OnProcessExiting(/*return_code=*/0, /*timestamp=*/0);
  }

  if (analytics_builder_.valid()) {
    analytics_builder_.SetRemoteSymbolLookupEnabledBit(remote_symbol_lookup_enabled_);
    analytics_builder_.SendAnalytics();
    analytics_builder_ = {};
  }

  // Support for dumpfile
  if (!dumpfile_output_.empty()) {
    ResetDumpfileCurrentObject();
  }

  // Enable batch mode only on {{{reset:begin}}}.
  in_batch_mode_ = type == Symbolizer::ResetType::kBegin;
}

void SymbolizerImpl::Module(uint64_t id, std::string_view name, std::string_view build_id) {
  if (!frames_in_batch_mode_.empty()) {
    FlushBufferedFramesWithContext("got a module tag, expected bt");
  }

  modules_[id].name = name;
  modules_[id].build_id = build_id;

  // Support for dumpfile
  if (!dumpfile_output_.empty()) {
    rapidjson::Value module(rapidjson::kObjectType);
    module.AddMember("name", ToJSONString(name), dumpfile_document_.GetAllocator());
    module.AddMember("build", ToJSONString(build_id), dumpfile_document_.GetAllocator());
    module.AddMember("id", id, dumpfile_document_.GetAllocator());
    dumpfile_current_object_["modules"].PushBack(module, dumpfile_document_.GetAllocator());
  }
}

SymbolizerImpl::MMapStatus SymbolizerImpl::MMap(uint64_t address, uint64_t size, uint64_t module_id,
                                                std::string_view flags, uint64_t module_offset) {
  if (!frames_in_batch_mode_.empty()) {
    FlushBufferedFramesWithContext("got a mmap tag, expected bt");
  }

  if (!modules_.contains(module_id)) {
    analytics_builder_.SetAtLeastOneInvalidInput();
    return MMapStatus::kInvalidModuleId;
  }

  ModuleInfo& module = modules_[module_id];
  uint64_t base = address - module_offset;

  bool inconsistent_base_address = false;
  if (address < module_offset) {
    // Negative load address. This happens for zircon on x64.
    if (module.printed) {
      if (module.base != 0 || module.negative_base != module_offset - address) {
        inconsistent_base_address = true;
      }
    } else {
      base = address;  // for printing only
      module.base = 0;
      module.negative_base = module_offset - address;
    }
    module.size = std::max(module.size, address + size);
  } else {
    if (module.printed) {
      if (module.base != base) {
        inconsistent_base_address = true;
      }
    } else {
      module.base = base;
    }
    module.size = std::max(module.size, size + module_offset);
  }

  // Support for dumpfile
  if (!dumpfile_output_.empty()) {
    rapidjson::Value segment(rapidjson::kObjectType);
    segment.AddMember("mod", module_id, dumpfile_document_.GetAllocator());
    segment.AddMember("vaddr", address, dumpfile_document_.GetAllocator());
    segment.AddMember("size", size, dumpfile_document_.GetAllocator());
    segment.AddMember("flags", ToJSONString(flags), dumpfile_document_.GetAllocator());
    segment.AddMember("mod_rel_addr", module_offset, dumpfile_document_.GetAllocator());
    dumpfile_current_object_["segments"].PushBack(segment, dumpfile_document_.GetAllocator());
  }

  if (inconsistent_base_address) {
    analytics_builder_.SetAtLeastOneInvalidInput();
    return MMapStatus::kInconsistentBaseAddress;
  }
  return MMapStatus::kOk;
}

void SymbolizerImpl::MMap(uint64_t address, uint64_t size, uint64_t module_id,
                          std::string_view flags, uint64_t module_offset, StringOutputFn output) {
  if (!frames_in_batch_mode_.empty()) {
    FlushBufferedFramesWithContext("got a mmap tag, expected bt");
  }

  auto status = MMap(address, size, module_id, flags, module_offset);
  switch (status) {
    case MMapStatus::kOk:
      break;
    case MMapStatus::kInconsistentBaseAddress:
      output("symbolizer: Inconsistent base address.");
      break;
    case MMapStatus::kInvalidModuleId:
      output("symbolizer: Invalid module id.");
      return;
    default:
      output("symbolizer: Unknown error state.");
      return;
  }

  // We only continue from the above switch/case block if module_id is valid.
  ModuleInfo& module = modules_[module_id];

  if (!omit_module_lines_ && !symbolizing_dart_ && !module.printed) {
    output(fxl::StringPrintf("[[[ELF module #0x%" PRIx64 " \"%s\" BuildID=%s 0x%" PRIx64 "]]]",
                             module_id, module.name.c_str(), module.build_id.c_str(), module.base));
    module.printed = true;
  }
}

SymbolizerImpl::BacktraceStatus SymbolizerImpl::Backtrace(uint64_t address,
                                                          debug_ipc::StackFrame::AddressType type,
                                                          LocationOutputFn output) {
  FX_LOGS(TRACE) << "Symbolizing backtrace for address 0x" << std::hex << address;
  InitProcess();
  auto timer_stop = fit::defer([this] { analytics_builder_.TotalTimerStop(); });

  analytics_builder_.IncreaseNumberOfFrames();

  // Find the module to see if the stack might be corrupt.
  const ModuleInfo* module = nullptr;
  if (auto next = address_to_module_id_.upper_bound(address);
      next != address_to_module_id_.begin()) {
    next--;
    const auto& module_id = next->second;
    const auto& prev = modules_[module_id];
    if (address - prev.base <= prev.size) {
      module = &prev;
    }
  }
  // Also check for negative loads which can happen on zircon on x64.
  // When this happens, the module will have a base address of 0.
  if (!module && address_to_module_id_.contains(0)) {
    uint64_t module_id = address_to_module_id_[0];
    const auto& mod = modules_[module_id];
    if (address - mod.base <= mod.size) {
      module = &mod;
    }
  }

  if (!module) {
    FX_LOGS(TRACE) << "Could not find matching module.";
    analytics_builder_.IncreaseNumberOfFramesInvalid();
    return BacktraceStatus::kNoOverlappingModule;
  }

  uint64_t call_address = address;

  if (module->negative_base) {
    call_address += module->negative_base;
  }
  // Substracts 1 from the address if it's a return address or unknown. It shouldn't be an issue
  // for unknown addresses as most instructions are more than 1 byte.
  if (type != debug_ipc::StackFrame::AddressType::kExact) {
    call_address -= 1;
  }

  debug_ipc::StackFrame frame{call_address, 0};
  zxdb::Stack& stack = target_->GetProcess()->GetThreads()[0]->GetStack();
  stack.SetFrames(debug_ipc::ThreadRecord::StackAmount::kFull, {frame});

  // All modules for this stack trace have been loaded by this point, so we can discard retained
  // data from previously handled stack traces (if any).
  previous_modules_.clear();

  bool symbolized = false;
  FX_LOGS(TRACE) << "Have " << stack.size() << " frames for address 0x" << std::hex << address;
  for (size_t i = 0; i < stack.size(); i++) {
    // Function name.
    const zxdb::Location location = stack[i]->GetLocation();
    if (location.symbol().is_valid()) {
      symbolized = true;
    } else {
      FX_LOGS(WARNING) << "Invalid symbol for stack frame " << i << " of address 0x" << std::hex
                       << address;
    }

    output(stack.size() - i - 1, location, *module);
  }

  // If no symbols were produced, it might be because the symbol file is unavailable.
  if (!symbolized) {
    // TODO(fdurso): Can this check be done faster like in ProcessSymbols::GetModuleForAddress?
    bool has_symbol_file = false;
    for (auto module_symbol : target_->GetSymbols()->GetModuleSymbols()) {
      if (module_symbol->GetStatus().build_id == module->build_id) {
        has_symbol_file = true;
        break;
      }
    }
    if (!has_symbol_file) {
      return BacktraceStatus::kSymbolFileUnavailable;
    }
  }

  // One physical frame could be symbolized to multiple inlined frames. We're only counting the
  // number of physical frames symbolized.
  if (symbolized) {
    FX_LOGS(TRACE) << "Successfully symbolized address 0x" << std::hex << address;
    analytics_builder_.IncreaseNumberOfFramesSymbolized();
  } else {
    FX_LOGS(WARNING) << "No stack frames symbolized for address 0x" << std::hex << address;
  }
  return BacktraceStatus::kOk;
}

void SymbolizerImpl::Backtrace(uint64_t frame_id, uint64_t address,
                               debug_ipc::StackFrame::AddressType type, std::string_view message,
                               StringOutputFn output) {
  if (prettify_enabled_ && in_batch_mode_) {
    if (frame_id < frames_in_batch_mode_.size()) {
      OutputBatchedBacktrace();
    }
    frames_in_batch_mode_.push_back(Frame{address, type, std::move(output)});
    return;
  }

  bool module_found = false;
  Backtrace(address, type,
            [this, frame_id, address, message, &output, &module_found](
                auto inline_index, auto& location, auto& module) {
              module_found = true;
              std::string out = FormatFrameIdAndAddress(frame_id, inline_index, address);
              out += " in";

              if (location.symbol().is_valid()) {
                auto symbol = location.symbol().Get();
                auto function = symbol->template As<zxdb::Function>();
                if (function && !symbolizing_dart_) {
                  out += " " + zxdb::FormatFunctionName(function, {}).AsString();
                } else {
                  out += " " + symbol->GetFullName();
                }
              }

              // FileLine info.
              if (location.file_line().is_valid()) {
                out += " " + location.file_line().file() + ":" +
                       std::to_string(location.file_line().line());
              }

              // Module offset.
              out += fxl::StringPrintf(" <%s>+0x%" PRIx64, module.name.c_str(),
                                       address - module.base + module.negative_base);

              // Extra message.
              if (!message.empty()) {
                out += " " + std::string(message);
              }

              output(out);
            });

  if (!module_found) {
    std::string out =
        FormatFrameIdAndAddress(frame_id, 0, address) + " is not covered by any module";
    if (!message.empty()) {
      out += " " + std::string(message);
    }
    output(out);
  }
}

// Consume frames_in_batch_mode_ and output.
void SymbolizerImpl::OutputBatchedBacktrace() {
  InitProcess();

  std::vector<debug_ipc::StackFrame> input_frames;
  input_frames.reserve(frames_in_batch_mode_.size());
  for (auto& frame : frames_in_batch_mode_) {
    input_frames.emplace_back(frame.address,
                              /*sp*/ 0,
                              /*cfa*/ 0, debug_ipc::StackFrame::Trust::kUnknown,
                              /*pc_is_return_address*/ frame.type,
                              std::vector<debug::RegisterValue>{});
  }
  zxdb::Stack& stack = target_->GetProcess()->GetThreads()[0]->GetStack();
  stack.SetFrames(debug_ipc::ThreadRecord::StackAmount::kFull, input_frames);

  for (auto& entry : pretty_stack_manager_->ProcessStack(stack)) {
    std::string out = "  #" + std::to_string(entry.begin_index);
    if (entry.match) {
      out += "…" + std::to_string(entry.begin_index + entry.frames.size()) + " «" +
             entry.match.description + "»";
    } else {
      bool symbolized = false;

      // Function name.
      auto& frame = entry.frames[0];
      auto& location = frame->GetLocation();
      if (location.symbol().is_valid()) {
        symbolized = true;
        auto symbol = location.symbol().Get();
        auto function = symbol->As<zxdb::Function>();
        if (function) {
          zxdb::FormatFunctionNameOptions options;
          options.params = zxdb::FormatFunctionNameOptions::kElideParams;
          out += " " + zxdb::FormatFunctionName(function, options).AsString();
        } else {
          out += " " + symbol->GetFullName();
        }
      }

      // FileLine info.
      if (location.file_line().is_valid()) {
        symbolized = true;
        std::string file = location.file_line().file();
        auto file_data = source_file_provider_->GetFileData(file, location.file_line().comp_dir());
        if (file_data.ok()) {
          // Convert the path to a relative one from cwd for better readability.
          file = std::filesystem::proximate(file_data.value().full_path);
        }
        out += " • " + file + ":" + std::to_string(location.file_line().line());
      }

      // If non of them exists, print the PC, relative PC, and module build id.
      if (!symbolized) {
        const ModuleInfo* module = nullptr;
        uint64_t address = frame->GetAddress();
        if (auto next = address_to_module_id_.upper_bound(address);
            next != address_to_module_id_.begin()) {
          next--;
          const auto& module_id = next->second;
          const auto& prev = modules_[module_id];
          if (address - prev.base <= prev.size) {
            module = &prev;
          }
        }

        out += fxl::StringPrintf(" 0x%012" PRIx64, address);
        if (!module) {
          out += " is not covered by any module";
        } else {
          out += fxl::StringPrintf(" %s(%s)+0x%" PRIx64, module->name.c_str(),
                                   module->build_id.c_str(),
                                   address - module->base + module->negative_base);
        }
      }
    }
    frames_in_batch_mode_.front().output(out);
    for (auto frame : entry.frames) {
      if (!frame->IsInline()) {
        frames_in_batch_mode_.pop_front();
      }
    }
  }
  FX_CHECK(frames_in_batch_mode_.empty());
}

void SymbolizerImpl::DumpFile(std::string_view type, std::string_view name) {
  if (!dumpfile_output_.empty()) {
    dumpfile_current_object_.AddMember("type", ToJSONString(type),
                                       dumpfile_document_.GetAllocator());
    dumpfile_current_object_.AddMember("name", ToJSONString(name),
                                       dumpfile_document_.GetAllocator());
    dumpfile_document_.PushBack(dumpfile_current_object_, dumpfile_document_.GetAllocator());
    ResetDumpfileCurrentObject();
  }
}

void SymbolizerImpl::OnDownloadsStarted() {
  FX_LOGS(DEBUG) << "Starting downloads";
  if (remote_symbol_lookup_enabled_) {
    analytics_builder_.DownloadTimerStart();
  }
  is_downloading_ = true;
}

void SymbolizerImpl::OnDownloadsStopped(size_t num_succeeded, size_t num_failed) {
  FX_LOGS(DEBUG) << "Downloads stopped";
  // Even if no symbol server is configured, this function could still be invoked but all
  // downloadings will be failed.
  if (remote_symbol_lookup_enabled_) {
    analytics_builder_.SetNumberOfModulesWithDownloadedSymbols(num_succeeded);
    analytics_builder_.SetNumberOfModulesWithDownloadingFailure(num_failed);
    analytics_builder_.DownloadTimerStop();
  }
  is_downloading_ = false;
  loop_.QuitNow();
}

void SymbolizerImpl::DidCreateSymbolServer(zxdb::SymbolServer* server) {
  FX_LOGS(DEBUG) << "Created symbol server " << server->name();
  if (server->state() == zxdb::SymbolServer::State::kInitializing ||
      server->state() == zxdb::SymbolServer::State::kBusy) {
    waiting_auth_ = true;
  }
}

void SymbolizerImpl::OnSymbolServerStatusChanged(zxdb::SymbolServer* server) {
  FX_LOGS(DEBUG) << "Symbol server " << server->name() << " status changed to "
                 << zxdb::ServerStateToString(server->state()) << ".";
  if (!waiting_auth_) {
    return;
  }

  for (auto& server : session_.system().GetSymbolServers()) {
    if (server->state() == zxdb::SymbolServer::State::kInitializing ||
        server->state() == zxdb::SymbolServer::State::kBusy) {
      return;
    }
  }

  waiting_auth_ = false;
  loop_.QuitNow();
}

void SymbolizerImpl::DidCreateProcess(zxdb::Process* process, uint64_t timestamp) {
  FX_LOGS(DEBUG) << "Created " << *process << "\n";
}

void SymbolizerImpl::WillDestroyProcess(zxdb::Process* process, DestroyReason reason, int exit_code,
                                        uint64_t timestamp) {
  FX_LOGS(DEBUG) << "Destroying " << *process << " with exit code " << exit_code << ": "
                 << DestroyReasonToString(reason) << "\n";
}

void SymbolizerImpl::WillLoadModuleSymbols(zxdb::Process* process, int num_modules) {
  FX_LOGS(DEBUG) << "About to load " << num_modules << " module symbols for " << *process << "\n";
}

void SymbolizerImpl::DidLoadModuleSymbols(zxdb::Process* process,
                                          zxdb::LoadedModuleSymbols* module) {
  FX_LOGS(DEBUG) << "Loaded module symbols for " << *process << ", build id " << module->build_id()
                 << "\n";
}

void SymbolizerImpl::DidLoadAllModuleSymbols(zxdb::Process* process) {
  FX_LOGS(DEBUG) << "Loaded all module symbols for " << *process << "\n";
}

void SymbolizerImpl::WillUnloadModuleSymbols(zxdb::Process* process,
                                             zxdb::LoadedModuleSymbols* module) {
  FX_LOGS(DEBUG) << "Unloading module symbols for " << *process << ", build id "
                 << module->build_id() << "\n";
}

void SymbolizerImpl::OnSymbolLoadFailure(zxdb::Process* process, const zxdb::Err& err) {
  FX_LOGS(WARNING) << "Symbols failed to load for " << *process << ": " << err.ToString() << "\n";
}

void SymbolizerImpl::InitProcess() {
  // Only initialize once, i.e. on the first frame of the backtrace.
  // DispatchProcessStarting will set the state to kRunning.
  if (target_->GetState() != zxdb::Target::State::kNone) {
    return;
  }

  analytics_builder_.TotalTimerStart();

  // Manually create our own target and process. The normal notification machinery expects a backend
  // to be connected and a filter to cause a notification, but there is no actual process to connect
  // to, which makes those notifications inappropriate to use here. Since we provide all of the
  // module information ourselves we need to manually set up the client objects to operate on.
  zxdb::TargetImpl* target_impl = session_.system().CreateNewTargetImpl(nullptr);
  target_impl->CreateProcess(zxdb::Process::StartType::kAttach, 0, "", 0, {}, std::nullopt);
  target_ = static_cast<zxdb::Target*>(target_impl);

  // This will match against process koid 0 we gave to the CreateProcess call above. We don't need
  // to do this manually since the thread state is being reconstructed from the symbolizer markup,
  // and all we're inspecting are the frames. Since we don't need any of the additional complicated
  // machinery that are involved with the client thread objects, we can simply have a default
  // constructed thread injected into the process we created above.
  session_.DispatchNotifyThreadStarting({});

  std::vector<debug_ipc::Module> modules;
  modules.reserve(modules_.size());
  for (const auto& pair : modules_) {
    modules.push_back({pair.second.name, pair.second.base, 0, pair.second.build_id});
    address_to_module_id_[pair.second.base] = pair.first;
  }
  target_->GetProcess()->GetSymbols()->SetModules(modules, false);

  // Collect module info for analytics.
  size_t num_modules_with_cached_symbols = 0;
  size_t num_modules_with_local_symbols = 0;
  auto cache_dir = session_.system().GetSymbols()->build_id_index().GetCacheDir();
  // GetModuleSymbols() will only return loaded modules.
  for (auto module_symbol : target_->GetSymbols()->GetModuleSymbols()) {
    if (!cache_dir.empty() &&
        zxdb::PathStartsWith(module_symbol->GetStatus().symbol_file, cache_dir)) {
      num_modules_with_cached_symbols++;
    } else {
      num_modules_with_local_symbols++;
    }
  }
  analytics_builder_.SetNumberOfModules(modules_.size());
  analytics_builder_.SetNumberOfModulesWithCachedSymbols(num_modules_with_cached_symbols);
  analytics_builder_.SetNumberOfModulesWithLocalSymbols(num_modules_with_local_symbols);

  // Wait until downloading completes.
  if (is_downloading_) {
    FX_LOGS(DEBUG) << "Waiting for download to complete...";
    loop_.Run();
    FX_LOGS(DEBUG) << "Done downloading symbols for process.";
  }
}

void SymbolizerImpl::ResetDumpfileCurrentObject() {
  dumpfile_current_object_.SetObject();
  dumpfile_current_object_.AddMember("modules", rapidjson::kArrayType,
                                     dumpfile_document_.GetAllocator());
  dumpfile_current_object_.AddMember("segments", rapidjson::kArrayType,
                                     dumpfile_document_.GetAllocator());
}

rapidjson::Value SymbolizerImpl::ToJSONString(std::string_view str) {
  rapidjson::Value string;
  string.SetString(str.data(), static_cast<uint32_t>(str.size()),
                   dumpfile_document_.GetAllocator());
  return string;
}

void SymbolizerImpl::FlushBufferedFramesWithContext(const std::string& context) {
  std::cerr
      << "WARN: " << context
      << " symbolization is incomplete due to interleaved stack traces! Logging what we have!\n";

  // Consume all the frames we were buffering.
  OutputBatchedBacktrace();
}

}  // namespace symbolizer
