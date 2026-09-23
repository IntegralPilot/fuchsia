// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace/commands/record.h"

#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/fdio/spawn.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <string.h>
#include <zircon/status.h>

#include <fstream>
#include <memory>
#include <string>
#include <unordered_set>

#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/string_number_conversions.h"
#include "src/performance/lib/trace_converters/chromium_exporter.h"
#include "src/performance/trace/utils.h"

namespace tracing {

namespace {

// Command line options.
const char kCategories[] = "categories";
const char kAppendArgs[] = "append-args";
const char kOutputFile[] = "output-file";
const char kBinary[] = "binary";
const char kDuration[] = "duration";
const char kDetach[] = "detach";
const char kDecouple[] = "decouple";
const char kSpawn[] = "spawn";
const char kEnvironmentName[] = "environment-name";
const char kReturnChildResult[] = "return-child-result";
const char kBufferSize[] = "buffer-size";
const char kProviderBufferSize[] = "provider-buffer-size";
const char kBufferingMode[] = "buffering-mode";
const char kTrigger[] = "trigger";

zx_status_t Spawn(const std::vector<std::string>& args, zx::process* subprocess) {
  FX_DCHECK(args.size() > 0);

  std::vector<const char*> raw_args;
  for (const auto& item : args) {
    raw_args.push_back(item.c_str());
  }
  raw_args.push_back(nullptr);

  return fdio_spawn(ZX_HANDLE_INVALID, FDIO_SPAWN_CLONE_ALL, raw_args[0], raw_args.data(),
                    subprocess->reset_and_get_address());
}

}  // namespace

bool RecordCommand::Options::Setup(const fxl::CommandLine& command_line) {
  const std::unordered_set<std::string> known_options = {
      kCategories, kAppendArgs,         kOutputFile,    kBinary,          kDuration,
      kDetach,     kDecouple,           kSpawn,         kEnvironmentName, kReturnChildResult,
      kBufferSize, kProviderBufferSize, kBufferingMode, kTrigger,
  };

  for (auto& option : command_line.options()) {
    if (known_options.count(option.name) == 0) {
      FX_LOGS(ERROR) << "Unknown option: " << option.name;
      return false;
    }
  }

  size_t index = 0;

  // --categories=<cat1>,<cat2>,...
  if (command_line.HasOption(kCategories, &index)) {
    categories = fxl::SplitStringCopy(command_line.options()[index].value, ",",
                                      fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
  }

  // --append-args=<arg1>,<arg2>,...
  // This option may be repeated, all args are added in order.
  // These arguments are added after the command line positional args.
  std::vector<std::string> append_args;
  if (command_line.HasOption(kAppendArgs, nullptr)) {
    auto all_append_args = command_line.GetOptionValues(kAppendArgs);
    for (const auto& arg : all_append_args) {
      auto args = fxl::SplitStringCopy(arg, ",", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
      std::move(std::begin(args), std::end(args), std::back_inserter(append_args));
    }
  }

  // --binary
  if (ParseBooleanOption(command_line, kBinary, &binary) == OptionStatus::ERROR) {
    return false;
  }
  if (binary) {
    output_file_name = kDefaultBinaryOutputFileName;
  }

  // --output-file=<file>
  if (command_line.HasOption(kOutputFile, &index)) {
    output_file_name = command_line.options()[index].value;
  }

  // --duration=<seconds>
  if (command_line.HasOption(kDuration, &index)) {
    int64_t seconds;
    if (!fxl::StringToNumberWithError(command_line.options()[index].value, &seconds)) {
      FX_LOGS(ERROR) << "Failed to parse command-line option " << kDuration << ": "
                     << command_line.options()[index].value;
      return false;
    }
    duration = zx::sec(seconds);
  }

  // --detach
  if (ParseBooleanOption(command_line, kDetach, &detach) == OptionStatus::ERROR) {
    return false;
  }

  // --decouple
  if (ParseBooleanOption(command_line, kDecouple, &decouple) == OptionStatus::ERROR) {
    return false;
  }

  // --spawn
  {
    bool spawn_value = false;
    OptionStatus spawn_status = ParseBooleanOption(command_line, kSpawn, &spawn_value);
    if (spawn_status == OptionStatus::ERROR) {
      return false;
    }
    bool have_spawn = spawn_status == OptionStatus::PRESENT;
    if (have_spawn) {
      spawn = spawn_value;
    }
  }

  // --environment-name
  if (command_line.HasOption(kEnvironmentName, &index)) {
    environment_name = command_line.options()[index].value;
  }

  // --return-child-result=<flag>
  if (ParseBooleanOption(command_line, kReturnChildResult, &return_child_result) ==
      OptionStatus::ERROR) {
    return false;
  }

  // --buffer-size=<megabytes>
  if (command_line.HasOption(kBufferSize, &index)) {
    if (!ParseBufferSize(command_line.options()[index].value, &buffer_size_megabytes)) {
      return false;
    }
  }

  // --provider-buffer-size=<name:megabytes>
  if (command_line.HasOption(kProviderBufferSize)) {
    std::vector<std::string_view> args = command_line.GetOptionValues(kProviderBufferSize);
    if (!ParseProviderBufferSize(args, &provider_specs)) {
      return false;
    }
  }

  // --buffering-mode=oneshot|circular|streaming
  if (command_line.HasOption(kBufferingMode, &index)) {
    BufferingMode mode;
    if (!ParseBufferingMode(command_line.options()[index].value, &mode)) {
      return false;
    }
    buffering_mode = TranslateBufferingMode(mode);
  }

  // --trigger=<alert>:<action>
  if (command_line.HasOption(kTrigger)) {
    std::vector<std::string_view> args = command_line.GetOptionValues(kTrigger);
    if (!ParseTriggers(args, &trigger_specs)) {
      return false;
    }
  }

  // <command> <args...>
  const auto& positional_args = command_line.positional_args();
  if (!positional_args.empty()) {
    app = positional_args[0];
    args = std::vector<std::string>(positional_args.begin() + 1, positional_args.end());
  }

  // Now that we've processed positional args we can append --append-args args.
  std::move(std::begin(append_args), std::end(append_args), std::back_inserter(args));

  return true;
}

Command::Info RecordCommand::Describe() {
  return Command::Info{
      []() { return std::make_unique<RecordCommand>(); },
      "record",
      "starts tracing and records data",
      {{"output-file=[/tmp/trace.json]", "Trace data is stored in this file."},
       {"binary=[false]",
        "Output the binary trace rather than converting to JSON. "
        "If this is set, then the default output location will be "
        "/tmp/trace.fxt"},

       {"duration=[10]",
        "Trace will be active for this many seconds after the session has been "
        "started. The provided value must be integral."},
       {"categories=["
        "]",
        "Categories that should be enabled for tracing"},
       {"append-args=["
        "]",
        "Additional args for the app being traced. The value is a comma-separated list of "
        "arguments to pass. This option may be repeated, arguments are added in order."},
       {"detach=[false]", "Don't stop the traced program when tracing finished"},
       {"decouple=[false]", "Don't stop tracing when the traced program exits"},
       {"spawn=[false]", "Use fdio_spawn to run a legacy app."},
       {"environment-name=[none]",
        "Create a nested environment with the given name and run the app being traced under it."},
       {"return-child-result=[true]",
        "Return with the same return code as the child. "
        "Only valid when a child program is passed."},
       {"buffer-size=[4]", "Maximum size of trace buffer for each provider in megabytes"},
       {"provider-buffer-size=[provider-name:buffer-size]",
        "Specify the buffer size that \"provider-name\" will use. "
        "May be specified multiple times, once per provider."},
       {"buffering-mode=oneshot|circular|streaming", "The buffering mode to use"},
       {"trigger=<alert>:<action>",
        "Specifies an action to take when an alert with "
        "the specified name is received. Multiple alert/action rules may be "
        "specified using multiple --trigger arguments. The only action currently "
        "supported is 'stop'. This action causes the session to be stopped and "
        "results to be captured"},
       {"[command args]",
        "Run program after starting trace. The program is terminated when "
        "tracing ends unless --detach is specified"}}};
}

RecordCommand::RecordCommand()
    : dispatcher_(async_get_default_dispatcher()),
      wait_spawned_app_(this),
      weak_ptr_factory_(this) {
  wait_spawned_app_.set_trigger(ZX_PROCESS_TERMINATED);
}

void RecordCommand::Start(const fxl::CommandLine& command_line) {
  if (!options_.Setup(command_line)) {
    FX_LOGS(ERROR) << "Error parsing options from command line - aborting";
    Done(EXIT_FAILURE);
    return;
  }

  std::unique_ptr<std::ofstream> out_stream = OpenOutputStream(options_.output_file_name);
  if (!out_stream) {
    FX_LOGS(ERROR) << "Failed to open " << options_.output_file_name << " for writing";
    Done(EXIT_FAILURE);
    return;
  }

  Tracer::BytesConsumer bytes_consumer;
  Tracer::RecordConsumer record_consumer;
  Tracer::ErrorHandler error_handler;
  if (options_.binary) {
    binary_out_ = std::move(out_stream);

    bytes_consumer = [this](const unsigned char* buffer, size_t n_bytes) {
      binary_out_->write(reinterpret_cast<const char*>(buffer), n_bytes);
    };
    record_consumer = [](trace::Record record) {};
    error_handler = [](std::string_view error) {};
  } else {
    exporter_ = std::make_unique<ChromiumExporter>(std::move(options_.output_file_name));

    bytes_consumer = [](const unsigned char* buffer, size_t n_bytes) {};
    record_consumer = [this](trace::Record record) { exporter_->ExportRecord(record); };
    error_handler = [](std::string_view error) { FX_LOGS(ERROR) << error; };
  }

  tracer_ = std::make_unique<Tracer>(take_provisioner());

  tracing_ = true;

  controller::TraceConfig trace_config{
      {.categories = options_.categories,
       .buffer_size_megabytes_hint = options_.buffer_size_megabytes,
       // TODO(dje): start_timeout_milliseconds
       .buffering_mode = options_.buffering_mode,
       .provider_specs = TranslateProviderSpecs(options_.provider_specs)}};

  tracer_->Initialize(
      std::move(trace_config), options_.binary, std::move(bytes_consumer),
      std::move(record_consumer), std::move(error_handler),
      [this] {
        DoneTrace();
      },  // TODO(https://fxbug.dev/42113074): For now preserve existing behaviour.
      [this] { DoneTrace(); }, fit::bind_member(this, &RecordCommand::OnAlert));

  tracer_->Start([this](fidl::Result<controller::Session::StartTracing> result) {
    if (result.is_error()) {
      FX_LOGS(ERROR) << "Unable to start trace: " << result.error_value();
      tracing_ = false;
      Done(EXIT_FAILURE);
      return;
    }
    if (!options_.app.empty()) {
      if (!options_.spawn) {
        FX_LOGS(ERROR) << "`--spawn` must be set";
        tracing_ = false;
        Done(EXIT_FAILURE);
        return;
      }
      LaunchSpawnedApp();
    }
    StartTimer();
  });
}

void RecordCommand::TerminateTrace(int32_t return_code) {
  if (tracing_) {
    out() << "Terminating trace...\n";
    tracing_ = false;
    return_code_ = return_code;
    tracer_->Terminate();
    if (spawned_app_ && !options_.detach) {
      KillSpawnedApp();
    }
  }
}

void RecordCommand::DoneTrace() {
  FX_DCHECK(!tracing_);

  tracer_.reset();
  exporter_.reset();

  out() << "Trace file written to " << options_.output_file_name << '\n';

  Done(return_code_);
}

// Quote elements of |args| as necessary to ensure the result can be correctly
// parsed by readers. But also do so minimally to maintain the S/N ratio.
// This is just a log message so the result doesn't need to be executable,
// this fact to avoid handling various complicated cases like one arg
// containing a mix of spaces, single quotes, and double quotes.
namespace {
std::string JoinArgsForLogging(const std::vector<std::string>& args) {
  std::string result;

  for (const auto& arg : args) {
    if (result.size() > 0) {
      result += " ";
    }
    if (arg.size() == 0) {
      result += "\"\"";
    } else if (arg.find(' ') != std::string::npos) {
      result += "{" + arg + "}";
    } else {
      result += arg;
    }
  }

  return result;
}
}  // namespace

void RecordCommand::LaunchSpawnedApp() {
  std::vector<std::string> all_args = {options_.app};
  all_args.insert(all_args.end(), options_.args.begin(), options_.args.end());

  // Include the arguments here: It's useful to see how the passed command+args ended up after shell
  // processing.
  FX_LOGS(INFO) << "Spawning: " << JoinArgsForLogging(all_args);

  zx::process subprocess;
  zx_status_t status = Spawn(all_args, &subprocess);
  if (status != ZX_OK) {
    TerminateTrace(EXIT_FAILURE);
    FX_LOGS(ERROR) << "Subprocess launch failed: \"" << status
                   << "\" Did you provide the full path to the tool?";
    return;
  }

  spawned_app_ = std::move(subprocess);

  wait_spawned_app_.set_object(spawned_app_.get());
  status = wait_spawned_app_.Begin(dispatcher_);
  FX_CHECK(status == ZX_OK) << "Failed to add handler: status=" << status;
}

void RecordCommand::OnSpawnedAppExit(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                                     zx_status_t status, const zx_packet_signal_t* signal) {
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to wait for spawned app: status=" << status;
    TerminateTrace(EXIT_FAILURE);
    return;
  }

  if (signal->observed & ZX_PROCESS_TERMINATED) {
    zx_info_process_t proc_info;
    [[maybe_unused]] zx_status_t info_status =
        spawned_app_.get_info(ZX_INFO_PROCESS, &proc_info, sizeof(proc_info), nullptr, nullptr);
    FX_DCHECK(info_status == ZX_OK);

    out() << "Application exited with return code " << proc_info.return_code << '\n';
    if (!options_.decouple) {
      if (options_.return_child_result) {
        TerminateTrace(static_cast<int32_t>(proc_info.return_code));
      } else {
        TerminateTrace(EXIT_SUCCESS);
      }
    }
  } else {
    FX_NOTREACHED();
  }
}

void RecordCommand::KillSpawnedApp() {
  FX_DCHECK(spawned_app_);

  // If already dead this is a no-op.
  [[maybe_unused]] zx_status_t status = spawned_app_.kill();
  FX_DCHECK(status == ZX_OK);

  wait_spawned_app_.Cancel();
  wait_spawned_app_.set_object(ZX_HANDLE_INVALID);
}

void RecordCommand::OnAlert(std::string alert_name) {
  auto iter = options_.trigger_specs.find(alert_name);
  if (iter == options_.trigger_specs.end()) {
    // No action specified for alert. This is expected.
    return;
  }

  switch (iter->second) {
    case Action::kStop:
      TerminateTrace(EXIT_SUCCESS);
      break;
  }
}

void RecordCommand::StartTimer() {
  async::PostDelayedTask(
      dispatcher_,
      [weak = weak_ptr_factory_.GetWeakPtr()] {
        if (weak)
          weak->TerminateTrace(EXIT_SUCCESS);
      },
      options_.duration);
  out() << "Starting trace; will stop in "
        << static_cast<double>(options_.duration.to_nsecs()) / 1000000000.0 << " seconds...\n";
}

}  // namespace tracing
