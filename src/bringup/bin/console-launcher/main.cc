// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.virtualconsole/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/namespace.h>
#include <lib/fdio/spawn.h>
#include <lib/fit/defer.h>
#include <lib/stdcompat/string_view.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/debuglog.h>
#include <lib/zx/process.h>
#include <lib/zx/time.h>
#include <zircon/processargs.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <algorithm>
#include <future>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/bringup/bin/console-launcher/console_launcher.h"
#include "src/bringup/bin/console-launcher/console_launcher_config.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/loader_service/loader_service.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/remote_dir.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"
#include "src/sys/lib/stdout-to-debuglog/cpp/stdout-to-debuglog.h"

namespace {

namespace fio = fuchsia_io;

template <typename FOnRepresentation>
class EventHandler final : public fidl::WireSyncEventHandler<fuchsia_io::Directory> {
 public:
  explicit EventHandler(FOnRepresentation on_representation)
      : on_representation_(std::move(on_representation)) {}

  void OnOpen(fidl::WireEvent<fuchsia_io::Directory::OnOpen>* event) override {
    ZX_PANIC("Should not receive an OnOpen event!");
  }

  void OnRepresentation(fidl::WireEvent<fuchsia_io::Directory::OnRepresentation>* event) override {
    on_representation_(event);
  }

  void handle_unknown_event(fidl::UnknownEventMetadata<fuchsia_io::Directory> metadata) override {
    ZX_PANIC("Received unknown event!");
  }

 private:
  FOnRepresentation on_representation_;
};

std::ostream& operator<<(std::ostream& os, const std::vector<std::string>& args) {
  for (size_t i = 0; i < args.size(); ++i) {
    if (i != 0) {
      os << ' ';
    }
    os << args[i];
  }
  return os;
}

zx::result<fidl::ClientEnd<fuchsia_hardware_pty::Device>> ConnectToPty(
    const console_launcher::Arguments& args) {
  if (args.use_virtio_console) {
    return component::SyncServiceMemberWatcher<fuchsia_hardware_pty::Service::Device>()
        .GetNextInstance(false);
  }

  return component::Connect<fuchsia_hardware_pty::Device>("/svc/console");
}

zx::result<fidl::ClientEnd<fuchsia_hardware_pty::Device>> CreateVirtualConsole(
    const fidl::WireSyncClient<fuchsia_virtualconsole::SessionManager>& session_manager) {
  auto [client, server] = fidl::Endpoints<fuchsia_hardware_pty::Device>::Create();

  const fidl::Status result = session_manager->CreateSession(std::move(server));
  if (!result.ok()) {
    FX_PLOGS(ERROR, result.status()) << "failed to create virtcon session";
    return zx::error(result.status());
  }
  return zx::ok(std::move(client));
}

std::vector<std::thread> LaunchAutorun(const console_launcher::ConsoleLauncher& launcher,
                                       std::shared_ptr<loader::LoaderService> ldsvc,
                                       fs::FuchsiaVfs& vfs, const fbl::RefPtr<fs::Vnode>& root,
                                       std::unordered_map<std::string_view, std::thread>& threads,
                                       const console_launcher::Arguments& args) {
  std::tuple<const char*, const std::string&, std::vector<std::string_view>> map[] = {
      // NB: //tools/emulator/emulator.go expects these to be available in its boot autorun.
      {"autorun:boot", args.autorun_boot, {"/dev"}},
      {"autorun:system", args.autorun_system, {"/system"}},
  };

  std::vector<std::thread> autorun;
  for (const auto& [name, args, paths] : map) {
    if (args.empty()) {
      continue;
    }
    if (!cpp20::starts_with(std::string_view{args}, "/")) {
      FX_LOGS(ERROR) << name << " failed to run '" << args << "' command must be absolute path";
      continue;
    }
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    if (endpoints.is_error()) {
      FX_PLOGS(FATAL, endpoints.status_value()) << "failed to create endpoints";
    }

    if (zx_status_t status = vfs.ServeDirectory(root, std::move(endpoints->server));
        status != ZX_OK) {
      FX_PLOGS(FATAL, status) << "failed to serve root directory";
    }

    zx::result loader = ldsvc->Connect();
    if (loader.is_error()) {
      FX_PLOGS(FATAL, loader.status_value()) << "failed to connect to loader service";
    }

    // Get the full commandline by splitting on '+'.
    std::vector argv = fxl::SplitStringCopy(args, "+", fxl::WhiteSpaceHandling::kTrimWhitespace,
                                            fxl::SplitResult::kSplitWantNonEmpty);
    autorun.emplace_back([paths = paths, &threads, args = std::move(argv), name = name,
                          loader = std::move(*loader), client_end = std::move(endpoints->client),
                          &job = launcher.shell_job()]() mutable {
      for (std::string_view path : paths) {
        if (auto it = threads.find(path); it != threads.end()) {
          it->second.join();
        } else {
          FX_LOGS(ERROR) << "unable to run '" << name << "': could not mount required path '"
                         << path << "'";
          return;
        }
      }

      const char* argv[args.size() + 1];
      argv[args.size()] = nullptr;
      for (size_t i = 0; i < args.size(); ++i) {
        argv[i] = args[i].c_str();
      }

      fdio_spawn_action_t actions[] = {
          {
              .action = FDIO_SPAWN_ACTION_SET_NAME,
              .name =
                  {
                      .data = name,
                  },
          },
          {
              .action = FDIO_SPAWN_ACTION_ADD_NS_ENTRY,
              .ns =
                  {
                      .prefix = "/",
                      .handle = client_end.channel().get(),
                  },
          },
          {
              .action = FDIO_SPAWN_ACTION_ADD_HANDLE,
              .h =
                  {
                      .id = PA_HND(PA_LDSVC_LOADER, 0),
                      .handle = loader.TakeChannel().release(),
                  },
          },
      };

      zx::process process;
      char err_msg[FDIO_SPAWN_ERR_MSG_MAX_LENGTH];
      constexpr uint32_t flags =
          FDIO_SPAWN_CLONE_ALL & ~FDIO_SPAWN_CLONE_NAMESPACE & ~FDIO_SPAWN_DEFAULT_LDSVC;
      FX_LOGS(INFO) << "starting '" << name << "': " << args;
      zx_status_t status =
          fdio_spawn_etc(job.get(), flags, argv[0], argv, nullptr, std::size(actions), actions,
                         process.reset_and_get_address(), err_msg);
      if (status != ZX_OK) {
        FX_PLOGS(ERROR, status) << "failed to start '" << name << "': " << err_msg;
        return;
      }
      if (zx_status_t status =
              process.wait_one(ZX_PROCESS_TERMINATED, zx::time::infinite(), nullptr);
          status != ZX_OK) {
        FX_PLOGS(ERROR, status) << "failed to wait for '" << name << "' termination";
      }
      FX_LOGS(INFO) << "completed '" << name << "': " << args;
    });
  }

  return autorun;
}

[[noreturn]] void RunSerialConsole(const console_launcher::ConsoleLauncher& launcher,
                                   std::shared_ptr<loader::LoaderService> ldsvc,
                                   fs::FuchsiaVfs& vfs, const fbl::RefPtr<fs::Vnode>& root,
                                   fidl::ClientEnd<fuchsia_hardware_pty::Device> stdio,
                                   const std::string& term, const std::optional<std::string>& cmd) {
  zx::duration backoff = zx::sec(1);
  const zx::duration max_backoff = zx::sec(60);
  const zx::duration quick_exit_threshold = zx::sec(1);

  while (true) {
    auto start_time = zx::clock::get_monotonic();

    auto [client, server] = fidl::Endpoints<fuchsia_hardware_pty::Device>::Create();

    const fidl::Status result = fidl::WireCall(stdio)->Clone(
        fidl::ServerEnd<fuchsia_unknown::Cloneable>(server.TakeChannel()));
    if (!result.ok()) {
      FX_PLOGS(FATAL, result.status()) << "failed to clone stdio handle";
    }

    zx::result directory = fidl::CreateEndpoints<fuchsia_io::Directory>();
    if (directory.is_error()) {
      FX_PLOGS(FATAL, directory.status_value()) << "failed to create directory endpoints";
    }
    if (zx_status_t status = vfs.ServeDirectory(root, std::move(directory->server));
        status != ZX_OK) {
      FX_PLOGS(FATAL, status) << "failed to serve root directory";
    }

    zx::result loader = ldsvc->Connect();
    if (loader.is_error()) {
      FX_PLOGS(FATAL, loader.status_value()) << "failed to connect to loader service";
    }

    zx::result process = launcher.LaunchShell(std::move(directory->client), std::move(*loader),
                                              std::move(client), term, cmd);
    if (process.is_error()) {
      FX_PLOGS(FATAL, process.status_value()) << "failed to launch shell";
    }

    if (zx_status_t status = console_launcher::WaitForExit(std::move(process.value()));
        status != ZX_OK) {
      FX_PLOGS(FATAL, status) << "failed to wait for shell exit";
    }

    auto duration = zx::clock::get_monotonic() - start_time;

    if (duration < quick_exit_threshold) {
      zx::nanosleep(zx::deadline_after(backoff));
      backoff = std::min(backoff * 2, max_backoff);
    } else {
      backoff = zx::sec(1);
    }
  }
}

}  // namespace

int main(int argv, char** argc) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"console-launcher"}).WithDispatcher(loop.dispatcher()).BuildAndInitialize();

  if (zx_status_t status = StdoutToDebuglog::Init(); status != ZX_OK) {
    FX_PLOGS(ERROR, status)
        << "failed to redirect stdout to debuglog, assuming test environment and continuing";
  }

  FX_LOGS(INFO) << "running";

  auto config = console_launcher_config::Config::TakeFromStartupHandle();

  zx::result get_args = console_launcher::GetArguments(config);
  if (get_args.is_error()) {
    FX_PLOGS(FATAL, get_args.status_value()) << "failed to get arguments";
  }
  console_launcher::Arguments args = get_args.value();

  async_dispatcher_t* dispatcher = loop.dispatcher();
  fbl::RefPtr root = fbl::MakeRefCounted<fs::PseudoDir>();

  std::unordered_map<std::string_view, std::thread> threads;
  fdio_flat_namespace_t* flat;
  if (zx_status_t status = fdio_ns_export_root(&flat); status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "failed to get namespace root";
  }
  auto free_flat = fit::defer([&flat]() { fdio_ns_free_flat_ns(flat); });

  // Our incoming namespace contains directories provided by fshost that may not
  // yet be responding to requests. This is ordinarily fine, but can cause
  // indefinite hangs in an interactive shell when storage devices fail to
  // start.
  //
  // Rather than expose these directly to the shell, indirect through a local
  // VFS to which entries are added only once they are seen to be servicing
  // requests. This causes the shell to initially observe an empty root
  // directory to which entries are added once they are ready for blocking
  // operations.
  for (size_t i = 0; i < flat->count; ++i) {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    std::string_view path = flat->path[i];
    constexpr fuchsia_io::Flags kFlags = fio::kPermReadable | fio::Flags::kPermInheritExecute |
                                         fio::Flags::kPermInheritWrite |
                                         fio::Flags::kFlagSendRepresentation;
    const fidl::Status result =
        fidl::WireCall(fidl::UnownedClientEnd<fuchsia_io::Directory>(flat->handle[i]))
            ->Open(".", kFlags, {}, server_end.TakeChannel());
    if (!result.ok()) {
      FX_PLOGS(ERROR, result.status()) << "failed to reopen '" << path << "'";
      continue;
    }

    // TODO(https://fxbug.dev/42147799): Replace the use of threads with async clients when it is
    // possible to extract the channel from the client.
    auto [thread, inserted] = threads.emplace(path, [&root, client_end = std::move(client_end),
                                                     dispatcher, path]() mutable {
      EventHandler handler([&](fidl::WireEvent<fuchsia_io::Directory::OnRepresentation>* event) {
        // Must run on the dispatcher thread to avoid racing with VFS dispatch.
        libsync::Completion completion;
        async::PostTask(
            dispatcher, [&completion, &root, path, client_end = std::move(client_end)]() mutable {
              const std::vector components = fxl::SplitString(path, "/", fxl::kKeepWhitespace,
                                                              fxl::SplitResult::kSplitWantNonEmpty);
              fbl::RefPtr<fs::Vnode> current = root;
              for (size_t i = 0; i < components.size(); i++) {
                const std::string_view& component = components[i];
                const std::string_view fragment = [&]() {
                  const ssize_t fragment_len = std::distance(path.begin(), component.end());
                  if (fragment_len < 0) {
                    const void* path_ptr = path.data();
                    const void* component_ptr = component.data();
                    FX_LOGS(FATAL) << "expected overlapping memory:"
                                   << " path@" << path_ptr << "=" << path << " component@"
                                   << component_ptr << "=" << component;
                  }
                  return path.substr(0, static_cast<size_t>(fragment_len));
                }();
                fbl::RefPtr<fs::Vnode> next;
                if (i == components.size() - 1) {
                  next = fbl::MakeRefCounted<fs::RemoteDir>(std::move(client_end));
                } else {
                  switch (zx_status_t status = current->Lookup(component, &current); status) {
                    case ZX_OK:
                      continue;
                    case ZX_ERR_NOT_FOUND:
                      next = fbl::MakeRefCounted<fs::PseudoDir>();
                      break;
                    default:
                      FX_PLOGS(FATAL, status) << "Lookup(" << fragment << ")";
                  }
                }
                if (zx_status_t status =
                        fbl::RefPtr<fs::PseudoDir>::Downcast(current)->AddEntry(component, next);
                    status != ZX_OK) {
                  FX_PLOGS(FATAL, status) << "failed to add entry for '" << fragment << "'";
                }
                current = next;
              }
              FX_LOGS(INFO) << "mounted '" << path << "'";
              completion.Signal();
            });
        completion.Wait();
      });

      if (fidl::Status status = handler.HandleOneEvent(client_end); !status.ok()) {
        FX_PLOGS(DEBUG, status.status()) << "failed to handle event for '" << path << "'";
      }
    });
    if (!inserted) {
      FX_LOGS(FATAL) << "duplicate namespace entry: " << path;
    }
  }

  std::thread thread([&loop]() {
    if (zx_status_t status = loop.Run(); status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "VFS loop exited";
    }
  });

  fs::ManagedVfs vfs(dispatcher);

  fbl::unique_fd lib_fd;
  if (zx_status_t status = fdio_open3_fd(
          "/boot/lib/",
          static_cast<uint64_t>(fio::wire::Flags::kProtocolDirectory | fio::wire::kPermReadable |
                                fio::wire::kPermExecutable),
          lib_fd.reset_and_get_address());
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "VFS loop exited";
  }
  auto ldsvc = loader::LoaderService::Create(dispatcher, std::move(lib_fd), "console-launcher");

  zx::result result = console_launcher::ConsoleLauncher::Create();
  if (result.is_error()) {
    FX_PLOGS(FATAL, result.status_value()) << "failed to create console launcher";
  }
  const auto& launcher = result.value();

  std::vector<std::thread> workers;

  if (!args.virtcon_disabled) {
    zx_status_t status = [&]() {
      zx::result virtcon = component::Connect<fuchsia_virtualconsole::SessionManager>();
      if (virtcon.is_error()) {
        FX_PLOGS(ERROR, virtcon.status_value())
            << "failed to connect to "
            << fidl::DiscoverableProtocolName<fuchsia_virtualconsole::SessionManager>;
        return virtcon.status_value();
      }
      fidl::WireSyncClient client{std::move(virtcon.value())};

      if (args.virtual_console_need_debuglog) {
        zx::result session = CreateVirtualConsole(client);
        if (session.is_error()) {
          return session.status_value();
        }

        workers.emplace_back([&, stdio = std::move(session.value())]() mutable {
          RunSerialConsole(launcher, ldsvc, vfs, root, std::move(stdio), args.term, "dlog -f -t");
        });
      }

      zx::result session = CreateVirtualConsole(client);
      if (session.is_error()) {
        return session.status_value();
      }
      workers.emplace_back([&, stdio = std::move(session.value())]() mutable {
        RunSerialConsole(launcher, ldsvc, vfs, root, std::move(stdio), "TERM=xterm-256color", {});
      });
      return ZX_OK;
    }();
    if (status != ZX_OK) {
      // If launching virtcon fails, we still should continue so that the autorun programs
      // and serial console are launched.
      FX_PLOGS(ERROR, status) << "failed to set up virtcon";
    }
  }

  if (args.run_shell) {
    FX_LOGS(INFO) << "console.shell: enabled";

    {
      std::vector<std::thread> autorun = LaunchAutorun(launcher, ldsvc, vfs, root, threads, args);
      workers.insert(workers.end(), std::make_move_iterator(autorun.begin()),
                     std::make_move_iterator(autorun.end()));
    }

    zx::result pty_result = ConnectToPty(args);
    if (pty_result.is_error()) {
      FX_PLOGS(FATAL, pty_result.error_value()) << "Failed to connect to PTY";
    }

    workers.emplace_back([&, stdio = std::move(pty_result.value())]() mutable {
      RunSerialConsole(launcher, ldsvc, vfs, root, std::move(stdio), args.term, {});
    });
  } else {
    if (!args.autorun_boot.empty()) {
      FX_LOGS(ERROR) << "cannot launch autorun command '" << args.autorun_boot << "'";
    }
    FX_LOGS(INFO) << "console.shell: disabled";

    for (auto& [_, thread] : threads) {
      thread.join();
    }
    thread.join();
  }
  for (auto& thread : workers) {
    thread.join();
  }
  // TODO(https://fxbug.dev/42179909): Hang around. If we exit before archivist has started, our
  // logs will be lost, and this log is load bearing in shell_disabled_test.
  std::promise<void>().get_future().wait();
}
