// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/cmdline/args_parser.h>
#include <lib/fit/defer.h>
#include <signal.h>
#include <sys/wait.h>
#include <unistd.h>

#include <cstdlib>
#include <memory>

#include "src/developer/debug/debug_agent/posix/eintr_wrapper.h"
#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/shared/buffered_bidi_pipe.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/message_loop_poll.h"
#include "src/developer/debug/zxdb/client/analytics.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/common/curl.h"
#include "src/developer/debug/zxdb/console/command_line_options.h"
#include "src/developer/debug/zxdb/console/command_sequence.h"
#include "src/developer/debug/zxdb/console/console_context.h"
#include "src/developer/debug/zxdb/console/console_impl.h"
#include "src/developer/debug/zxdb/console/console_noninteractive.h"
#include "src/developer/debug/zxdb/console/fd_streamer.h"
#include "src/developer/debug/zxdb/console/script_runner.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/developer/debug/zxdb/debug_adapter/server.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/developer/debug/zxdb/local_agent.h"
#include "src/lib/fxl/strings/string_printf.h"

#if defined(__linux__) && defined(__x86_64__)
// Indicates that we can support the local built-in debug agent on this platform.
#define SUPPORTS_LOCAL_AGENT
#endif

namespace zxdb {

namespace {

// Loads any actions specified on the command line into the vector.
Err SetupActions(const CommandLineOptions& options, std::vector<std::string>* actions) {
  if (options.core) {
    if (options.connect) {
      return Err("--core can't be used with commands to connect.");
    }
    actions->push_back(VerbToString(Verb::kOpenDump) + " " + *options.core);
  }

  if (options.connect)
    actions->push_back(VerbToString(Verb::kConnect) + " " + *options.connect);

  if (options.unix_connect)
    actions->push_back(VerbToString(Verb::kConnect) + " -q -u " + *options.unix_connect);

  for (const auto& script_file : options.config_files) {
    ErrOr<std::vector<std::string>> cmds_or = ReadCommandsFromFile(script_file);
    if (cmds_or.has_error())
      return cmds_or.err();
    actions->insert(actions->end(), cmds_or.value().begin(), cmds_or.value().end());
  }

  for (const auto& attach : options.attach) {
    actions->push_back(VerbToString(Verb::kAttach) + " " + attach);
  }

  actions->insert(actions->end(), options.execute_commands.begin(), options.execute_commands.end());

  return Err();
}

void InitConsole(zxdb::Console& console) {
  // We disabled input during startup. Balance that by enabling input now.
  console.EnableInput();

  // Help text.
  OutputBuffer help;
  help.Append(Syntax::kWarning, "👉 ");
  help.Append(Syntax::kComment, "To get started, try \"status\" or \"help\".");
  console.Output(help);
}

void SetupCommandLineOptions(const CommandLineOptions& options, Session* session) {
  auto& system_settings = session->system().settings();

  if (options.debug_mode)
    system_settings.SetBool(ClientSettings::System::kDebugMode, true);
  if (options.console_mode)
    system_settings.SetString(ClientSettings::System::kConsoleMode, *options.console_mode);
  if (options.embedded_mode_context)
    system_settings.SetString(ClientSettings::System::kEmbeddedModeContext,
                              *options.embedded_mode_context);
  if (options.auto_attach_limbo)
    system_settings.SetBool(ClientSettings::System::kAutoAttachLimbo, true);
  if (options.symbol_cache)
    system_settings.SetString(ClientSettings::System::kSymbolCache, *options.symbol_cache);
  if (!options.symbol_index_files.empty())
    system_settings.SetList(ClientSettings::System::kSymbolIndexFiles, options.symbol_index_files);
  if (!options.private_symbol_servers.empty())
    system_settings.SetList(ClientSettings::System::kPrivateSymbolServers,
                            options.private_symbol_servers);
  if (!options.public_symbol_servers.empty())
    system_settings.SetList(ClientSettings::System::kPublicSymbolServers,
                            options.public_symbol_servers);
  if (!options.symbol_paths.empty())
    system_settings.SetList(ClientSettings::System::kSymbolPaths, options.symbol_paths);
  if (!options.build_id_dirs.empty())
    system_settings.SetList(ClientSettings::System::kBuildIdDirs, options.build_id_dirs);
  if (!options.ids_txts.empty())
    system_settings.SetList(ClientSettings::System::kIdsTxts, options.ids_txts);
}

}  // namespace

int ConsoleMain(int argc, const char* argv[]) {
  signal(SIGPIPE, SIG_IGN);

  // Declare the loop at the top because it must outlive other objects that
  // might call loop methods during their destruction.
  //
  // Declaring `defer_loop_cleanup` before those objects ensures that
  // `loop.Cleanup()` runs after they have been fully cleaned up.
  debug::MessageLoopPoll loop;
  bool loop_is_initialized = false;
  auto defer_loop_cleanup = fit::defer([&loop, &loop_is_initialized] {
    if (loop_is_initialized) {
      loop.Cleanup();
    }
  });

  using ::analytics::core_dev_tools::EarlyProcessAnalyticsOptions;

  CommandLineOptions options;
  std::vector<std::string> params;
  cmdline::Status status = ParseCommandLine(argc, argv, &options, &params);
  if (status.has_error()) {
    fprintf(stderr, "%s", status.error_message().c_str());
    return 1;
  }

  if (options.requested_version) {
    printf("Version: %d\n", debug_ipc::kCurrentProtocolVersion);
    return 0;
  }

  Curl::GlobalInit();
  auto deferred_cleanup_curl = fit::defer(Curl::GlobalCleanup);
  auto deferred_cleanup_analytics = fit::defer(Analytics::CleanUp);

  if (EarlyProcessAnalyticsOptions<Analytics>(options.analytics, options.analytics_show)) {
    return 0;
  }

#ifdef SUPPORTS_LOCAL_AGENT
  // Handle a local connection to the built-in debug agent when supported. We want to fork as early
  // as possible to avoid accumulating state that isn't relevant to the debug_agent process
  // (especially message loops which will conflict). To accommodate early returns later in this
  // function, we set up a deferred callback which kills the agent on return. This is removed
  // when we "commit" the connection at which point the client sents IPC messages to manage the
  // agent lifetime.
  //
  // To prevent a zombie, every process should wait on the PID for every subprocess it forks.
  std::optional<LocalAgentResult> local_agent;
  fit::deferred_callback cleanup_local_agent;
  if (options.local) {
    local_agent = ForkLocalAgent();
    if (local_agent->status == LocalAgentResult::kSuccess) {
      cleanup_local_agent = fit::defer_callback([pid = local_agent->agent_pid]() {
        // The early returns in the code below will leave the forked agent running and the wait
        // will hang, so kill it before waiting. This also catches mistakes if we forget to
        // exit the agent via IPC once things are running normally.
        if (kill(pid, SIGKILL) == 0) {
          waitpid(pid, 0, 0);
        }
      });
    } else {
      return local_agent->exit_code;
    }
  }
#else  // No local agent
  if (options.local) {
    fprintf(stderr, "No local debugging supported on this platform.");
    return 1;
  }
#endif

  std::vector<std::string> actions;
  Err err = SetupActions(options, &actions);
  if (err.has_error()) {
    fprintf(stderr, "%s\n", err.msg().c_str());
    return 1;
  }

  std::string error_message;
  if (!loop.Init(&error_message)) {
    fprintf(stderr, "%s", error_message.c_str());
    return 1;
  }
  loop_is_initialized = true;

  // This scope forces all the objects to be destroyed before the Cleanup() call which will mark the
  // message loop as not-current.
  int ret_code = 0;
  {
    // Hold ownership of any local pipe in our scope to ensure it gets cleaned up before the
    // message loop exits (it will attach to the loop when its Start() function is called and
    // will disconnect from the loop in its destructor).
    std::unique_ptr<debug::BufferedBidiPipe> local_pipe;

    std::unique_ptr<Session> session;
    if (options.local) {
#ifdef SUPPORTS_LOCAL_AGENT
      // The local debug_agent will have been started above.
      local_pipe = std::move(local_agent->pipe);
      session = std::make_unique<Session>(&local_pipe->stream());

      local_pipe->set_data_available_callback([&session]() { session->OnStreamReadable(); });
      if (!local_pipe->Start()) {
        fprintf(stderr, "Failed to connect to the pipe.");
        return 1;
      }

      // If a binary is supplied on the command line, set it as the default target's process name.
      if (!params.empty()) {
        auto targets = session->system().GetTargets();
        FX_CHECK(!targets.empty());  // Should always have a default target.
        targets[0]->SetArgs(params);
      }
#endif  // SUPPORTS_LOCAL_AGENT
    } else {
      // Normal remote connections don't start with a connected session.
      session = std::make_unique<Session>();
    }

    Analytics::Init(*session, options.analytics);
    session->analytics().Init(session.get());
    session->analytics().ReportInvoked();

    debug::SetLogCategories({debug::LogCategory::kAll});
    SetupCommandLineOptions(options, session.get());

    std::unique_ptr<Console> console;
    std::unique_ptr<DebugAdapterServer> debug_adapter;

    if (options.enable_debug_adapter) {
      int port = options.debug_adapter_port;
      console = std::make_unique<ConsoleNoninteractive>(session.get());
      debug_adapter = std::make_unique<DebugAdapterServer>(console->get(), port);
      err = debug_adapter->Init();
      if (err.has_error()) {
        fprintf(stderr, "Failed to initialize debug adapter: %s\n", err.msg().c_str());

        return EXIT_FAILURE;
      }
    } else {
      if (isatty(STDIN_FILENO) && tcgetpgrp(STDIN_FILENO) != getpgrp()) {
        fprintf(stderr, "Error: This program cannot be run in the background.\n");
        return EXIT_FAILURE;
      }
      console = std::make_unique<ConsoleImpl>(session.get());
    }

    // Suppress input during startup.
    // Balanced in InitConsole.
    console->DisableInput();

    // Enable streaming as soon as possible. If we're not in embedded mode, it will be disabled
    // during |InitConsoleMode| below. We do this before registering the streamers with the console
    // so that there's no flickering between here and initializing the console mode below. If the
    // program attempting to stream output through zxdb is sensitive to timing issues on the target,
    // they should be waiting for |signal_when_ready| below to start sending data through the FD.
    console->EnableStreaming();

    // This will enable output from the console if we're not in embedded mode, so we can reliably
    // use the logging macros now.
    console->Init();

    std::vector<std::unique_ptr<debug::BufferedFD>> file_streamers;
    for (auto& path : options.stream_files) {
      fbl::unique_fd fd(HANDLE_EINTR(open(path.c_str(), O_RDONLY | O_NONBLOCK)));
      if (!fd.is_valid()) {
        LOGS(Error) << "Failed to open file for streaming: " << path;
        return EXIT_FAILURE;
      }
      file_streamers.emplace_back(StreamFDToConsole(std::move(fd), console.get()));
    }

    auto on_ready = [&]() {
      // Display the first prompt to the screen
      InitConsole(*console);

      // The console is now ready for interactive input.
      if (options.signal_when_ready) {
        int rv = kill(options.signal_when_ready, SIGUSR1);
        if (rv != 0) {
          LOGS(Error) << "Failed to send SIGUSR1: " << strerror(errno);
        }
      }
    };

    // Run the actions and then initialize the console to enter interactive mode. Errors in
    // running actions should be fatal and quit the debugger.
    std::unique_ptr<ScriptRunner> script_runner;
    RunCommandSequence(
        console.get(), std::move(actions),
        fxl::MakeRefCounted<ConsoleCommandContext>(console.get(), [&](const Err& err) {
          if (err.has_error()) {
            ret_code = 1;
            loop.QuitNow();
          } else {
            if (options.script_file) {
              script_runner = std::make_unique<ScriptRunner>(session.get(), console.get());
              script_runner->Run(*options.script_file, [&](bool success) {
                if (!success) {
                  ret_code = 1;
                  loop.QuitNow();
                } else {
                  // Successful script completion.
                  on_ready();
                }
              });
            } else {
              on_ready();
            }
          }
        }));

    loop.Run();
  }

  return ret_code;
}

}  // namespace zxdb

int main(int argc, char* argv[]) { return zxdb::ConsoleMain(argc, const_cast<const char**>(argv)); }
