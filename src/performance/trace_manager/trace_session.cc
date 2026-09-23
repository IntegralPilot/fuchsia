// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/trace_session.h"

#include <lib/async/cpp/executor.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/fields.h>
#include <lib/zx/vmo.h>

#include <algorithm>
#include <numeric>
#include <unordered_set>

#include "src/performance/trace_manager/traceev2.h"
#include "src/performance/trace_manager/util.h"

namespace {
template <class... Ts>
struct overloaded : Ts... {
  using Ts::operator()...;
};
template <class... Ts>
overloaded(Ts...) -> overloaded<Ts...>;
}  // namespace

namespace tracing {

TraceSession::TraceSession(async::Executor& executor, std::shared_ptr<BufferForwarder> destination,
                           std::vector<std::string> enabled_categories,
                           size_t buffer_size_megabytes,
                           fuchsia_tracing::BufferingMode buffering_mode,
                           TraceProviderSpecMap&& provider_specs, zx::duration start_timeout,
                           zx::duration stop_timeout,
                           fuchsia_tracing_controller::FxtVersion fxt_version,
                           fit::closure abort_handler, AlertCallback alert_callback)
    : executor_(executor),
      buffer_forwarder_(std::move(destination)),
      enabled_categories_(std::move(enabled_categories)),
      buffer_size_megabytes_(buffer_size_megabytes),
      buffering_mode_(buffering_mode),
      provider_specs_(std::move(provider_specs)),
      start_timeout_(start_timeout),
      stop_timeout_(stop_timeout),
      fxt_version_(std::move(fxt_version)),
      abort_handler_(std::move(abort_handler)),
      alert_callback_(std::move(alert_callback)),
      weak_ptr_factory_(this) {}

TraceSession::~TraceSession() {
  session_start_timeout_.Cancel();
  session_stop_timeout_.Cancel();
  session_terminate_timeout_.Cancel();
}

void TraceSession::AddProvider(TraceProviderBundle* provider) {
  if (state_ == State::kTerminating) {
    FX_LOGS(DEBUG) << "Ignoring new provider " << *provider << ", terminating";
    return;
  }

  size_t buffer_size_megabytes = buffer_size_megabytes_;
  // Include at least the umbrella enabled categories.
  std::unordered_set<std::string> provider_specific_categories(enabled_categories_.begin(),
                                                               enabled_categories_.end());
  auto spec_iter = provider_specs_.find(provider->name);
  if (spec_iter != provider_specs_.end()) {
    const TraceProviderSpec* spec = &spec_iter->second;
    if (spec->buffer_size_megabytes) {
      buffer_size_megabytes = spec->buffer_size_megabytes.value();
    }
    provider_specific_categories.insert(spec->categories.begin(), spec->categories.end());
  }
  uint64_t buffer_size = buffer_size_megabytes * 1024 * 1024;

  FX_LOGS(DEBUG) << "Adding provider " << *provider << ", buffer size " << buffer_size_megabytes
                 << "MB";

  tracees_.emplace_back(std::make_unique<Tracee>(executor_, buffer_forwarder_, provider));
  std::vector<std::string> categories_clone(provider_specific_categories.begin(),
                                            provider_specific_categories.end());
  if (!std::get<std::unique_ptr<Tracee>>(tracees_.back())
           ->Initialize(
               std::move(categories_clone), buffer_size, buffering_mode_,
               [weak = weak_ptr_factory_.GetWeakPtr(), provider]() {
                 if (weak) {
                   weak->OnProviderStarted(provider);
                 }
               },
               [weak = weak_ptr_factory_.GetWeakPtr(), provider](bool write_results) {
                 if (weak) {
                   weak->OnProviderStopped(provider, write_results);
                 }
               },
               [weak = weak_ptr_factory_.GetWeakPtr(), provider]() {
                 if (weak) {
                   weak->OnProviderTerminated(provider);
                 }
               },
               [weak = weak_ptr_factory_.GetWeakPtr()](const std::string& alert_name) {
                 if (weak && weak->alert_callback_) {
                   weak->alert_callback_(alert_name);
                 }
               })) {
    tracees_.pop_back();
  } else {
    Tracee* tracee = std::get<std::unique_ptr<Tracee>>(tracees_.back()).get();
    switch (state_) {
      case State::kInitialized:
        // Nothing more to do.
        break;
      case State::kStarting:
      case State::kStarted:
        // This is a new provider, there is nothing in the buffer to retain.
        tracee->Start(fuchsia_tracing::BufferDisposition::kClearEntire, additional_categories_);
        break;
      case State::kStopping:
      case State::kStopped:
        // Mark the tracee as stopped so we don't try to wait for it to do so.
        // This is a new provider, there are no results to write.
        tracee->Stop(/*write_results=*/false);
        break;
      default:
        FX_NOTREACHED();
        break;
    }
  }
}

void TraceSession::AddProvider(ProviderConnection* provider) {
  if (state_ == State::kTerminating) {
    FX_LOGS(DEBUG) << std::format("Ignoring new provider {}, terminating", *provider);
    return;
  }

  size_t buffer_size_megabytes = buffer_size_megabytes_;
  // Include at least the umbrella enabled categories.
  std::unordered_set<std::string> provider_specific_categories(enabled_categories_.begin(),
                                                               enabled_categories_.end());
  auto spec_iter = provider_specs_.find(provider->name);
  if (spec_iter != provider_specs_.end()) {
    const TraceProviderSpec* spec = &spec_iter->second;
    if (spec->buffer_size_megabytes) {
      buffer_size_megabytes = spec->buffer_size_megabytes.value();
    }
    provider_specific_categories.insert(spec->categories.begin(), spec->categories.end());
  }
  uint64_t buffer_size = buffer_size_megabytes * 1024 * 1024;

  FX_LOGS(DEBUG) << std::format("Adding provider {}, buffer size {} MB", *provider,
                                buffer_size_megabytes);

  provider->RegisterForAlerts([weak = weak_ptr_factory_.GetWeakPtr()](std::string_view alert_name) {
    if (weak && weak->alert_callback_) {
      weak->alert_callback_(std::string(alert_name));
    }
  });
  tracees_.emplace_back(std::make_unique<TraceeV2>(executor_, buffer_forwarder_, provider));
  std::unique_ptr<TraceeV2>& tracee = std::get<std::unique_ptr<TraceeV2>>(tracees_.back());

  std::vector<std::string> categories_clone(provider_specific_categories.begin(),
                                            provider_specific_categories.end());
  if (!tracee->Initialize(std::move(categories_clone), buffer_size, buffering_mode_)) {
    tracees_.pop_back();
  } else {
    // Register the tracee to receive
    provider->RegisterForBufferSave([weak_session = weak_ptr_factory_.GetWeakPtr(),
                                     weak_tracee = tracee->GetWeakPtr(),
                                     provider](uint32_t wrapped_count, uint64_t durable_data_end) {
      if (weak_tracee) {
        if (weak_tracee->state() == Tracee::State::kStarted ||
            weak_tracee->state() == Tracee::State::kStopping ||
            weak_tracee->state() == Tracee::State::kTerminating) {
          FX_LOGS(DEBUG) << std::format(
              "Buffer save request from {}, wrapped_count={}, durable_data_end={:x}",
              *weak_tracee->connection(), wrapped_count, durable_data_end);
          if (weak_tracee->TransferBuffer(wrapped_count, durable_data_end).is_error()) {
            if (weak_session) {
              weak_session->OnV2ProviderTerminated(provider);
            }
          }
        } else {
          FX_LOGS(WARNING) << std::format("{}: Received buffer save request in state ", *provider)
                           << weak_tracee->state();
          if (weak_session) {
            weak_session->OnV2ProviderTerminated(provider);
          }
        }
      } else {
        if (weak_session) {
          weak_session->OnV2ProviderTerminated(provider);
        }
      }
    });

    switch (state_) {
      case State::kInitialized:
        // Nothing more to do.
        break;
      case State::kStarting:
      case State::kStarted:
        // This is a new provider, there is nothing in the buffer to retain.
        tracee->Start(fuchsia_tracing::BufferDisposition::kClearEntire, additional_categories_,
                      [weak = weak_ptr_factory_.GetWeakPtr(), provider]() {
                        if (weak) {
                          weak->OnV2ProviderStarted(provider);
                        }
                      });
        break;
      case State::kStopping:
      case State::kStopped:
        // Mark the tracee as stopped so we don't try to wait for it to do so.
        // This is a new provider, there are no results to write.
        tracee->Stop([weak = weak_ptr_factory_.GetWeakPtr(), provider]() {
          if (weak) {
            weak->OnV2ProviderStopped(provider, false);
          }
        });
        break;
      default:
        FX_NOTREACHED();
        break;
    }
  }
}

void TraceSession::Terminate(fit::closure callback) {
  if (state_ == State::kTerminating) {
    return;
  }

  TransitionToState(State::kTerminating);
  terminate_callback_ = std::move(callback);

  for (const auto& tracee_variant : tracees_) {
    std::visit(overloaded{[](const std::unique_ptr<Tracee>& tracee) { tracee->Terminate(); },
                          [this](const std::unique_ptr<TraceeV2>& tracee) {
                            tracee->Terminate([weak = weak_ptr_factory_.GetWeakPtr(),
                                               provider = tracee->connection()]() {
                              if (weak) {
                                weak->OnV2ProviderTerminated(provider);
                              }
                            });
                          }},
               tracee_variant);
  }

  session_terminate_timeout_.PostDelayed(executor_.dispatcher(), stop_timeout_);
  TerminateSessionIfEmpty();
}

void TraceSession::Start(fuchsia_tracing::BufferDisposition buffer_disposition,
                         const std::vector<std::string>& additional_categories,
                         StartTracingCallback callback) {
  FX_DCHECK(state_ == State::kInitialized || state_ == State::kStopped);

  if (force_clear_buffer_contents_) {
    // "force-clear" -> Clear the entire buffer because it was saved.
    buffer_disposition = fuchsia_tracing::BufferDisposition::kClearEntire;
  }
  force_clear_buffer_contents_ = false;

  for (const auto& tracee_variant : tracees_) {
    std::visit(overloaded{[&](const std::unique_ptr<Tracee>& tracee) {
                            tracee->Start(buffer_disposition, additional_categories);
                          },
                          [&](const std::unique_ptr<TraceeV2>& tracee) {
                            tracee->Start(buffer_disposition, additional_categories,
                                          [weak = weak_ptr_factory_.GetWeakPtr(),
                                           provider = tracee->connection()]() {
                                            if (weak) {
                                              weak->OnV2ProviderStarted(provider);
                                            }
                                          });
                          }},
               tracee_variant);
  }

  start_callback_ = std::move(callback);
  session_start_timeout_.PostDelayed(executor_.dispatcher(), start_timeout_);

  // Clear out any old trace stats before starting a new session.
  trace_stats_.clear();

  // We haven't fully started at this point, we still have to wait for each
  // provider to indicate it they've started.
  TransitionToState(State::kStarting);

  // If there are no providers currently registered, then we are started.
  CheckAllProvidersStarted();

  // Save for tracees that come along later.
  additional_categories_ = additional_categories;
}

void TraceSession::Stop(bool write_results, StopTracingCallback callback) {
  FX_DCHECK(state_ == State::kInitialized || state_ == State::kStarting ||
            state_ == State::kStarted);

  TransitionToState(State::kStopping);
  stop_callback_ = std::move(callback);

  for (const auto& tracee_variant : tracees_) {
    std::visit(
        overloaded{[&](const std::unique_ptr<Tracee>& tracee) { tracee->Stop(write_results); },
                   [&](const std::unique_ptr<TraceeV2>& tracee) {
                     tracee->Stop([weak = weak_ptr_factory_.GetWeakPtr(),
                                   provider = tracee->connection(), write_results]() {
                       if (weak) {
                         weak->OnV2ProviderStopped(provider, write_results);
                       }
                     });
                   }},
        tracee_variant);
  }

  // If we're writing results then force-clear the buffer on the next Start.
  if (write_results) {
    force_clear_buffer_contents_ = true;
  }

  session_stop_timeout_.PostDelayed(executor_.dispatcher(), stop_timeout_);
  CheckAllProvidersStopped();

  // Clear out, must be respecified for each Start() request.
  additional_categories_.clear();
}

// Called when a provider reports that it has started.

void TraceSession::OnProviderStarted(TraceProviderBundle* bundle) {
  if (state_ == State::kStarting) {
    CheckAllProvidersStarted();
  } else if (state_ == State::kStarted) {
    // Nothing to do. One example of when this can happen is if we time out
    // waiting for providers to start and then a provider reports starting
    // afterwards.
  } else {
    // Tracing likely stopped or terminated in the interim.
    auto it = std::find_if(tracees_.begin(), tracees_.end(), [bundle](const auto& tracee_variant) {
      if (std::holds_alternative<std::unique_ptr<Tracee>>(tracee_variant)) {
        return *std::get<std::unique_ptr<Tracee>>(tracee_variant) == bundle;
      }
      return false;
    });

    if (it != tracees_.end()) {
      auto& tracee = std::get<std::unique_ptr<Tracee>>(*it);
      if (state_ == State::kInitialized) {
        FX_LOGS(WARNING) << "Provider " << *bundle << " sent a \"started\""
                         << " notification but tracing hasn't started";
        // Misbehaving provider, but it may just be slow.
        tracee->Stop(/*write_results=*/false);
      } else if (state_ == State::kStopping || state_ == State::kStopped) {
        tracee->Stop(/*write_results=*/false);
      } else {
        tracee->Terminate();
      }
    }
  }
}

void TraceSession::OnV2ProviderStarted(ProviderConnection* connection) {
  if (state_ == State::kStarting) {
    CheckAllProvidersStarted();
  } else if (state_ == State::kStarted) {
    // Nothing to do. One example of when this can happen is if we time out
    // waiting for providers to start and then a provider reports starting
    // afterwards.
  } else {
    // Tracing likely stopped or terminated in the interim.
    auto it = std::ranges::find_if(tracees_, [connection](const auto& tracee_variant) {
      if (std::holds_alternative<std::unique_ptr<TraceeV2>>(tracee_variant)) {
        return *std::get<std::unique_ptr<TraceeV2>>(tracee_variant) == connection;
      }
      return false;
    });

    if (it != tracees_.end()) {
      auto& tracee = std::get<std::unique_ptr<TraceeV2>>(*it);
      if (state_ == State::kInitialized) {
        FX_LOGS(WARNING) << std::format(
            "Provider {} sent a \"started\" notification but tracing hasn't started", *connection);
        // Misbehaving provider, but it may just be slow.
        tracee->Stop([weak = weak_ptr_factory_.GetWeakPtr(), provider = connection]() {
          if (weak) {
            weak->OnV2ProviderStopped(provider, false);
          }
        });
      } else if (state_ == State::kStopping || state_ == State::kStopped) {
        tracee->Stop([weak = weak_ptr_factory_.GetWeakPtr(), provider = connection]() {
          if (weak) {
            weak->OnV2ProviderStopped(provider, false);
          }
        });
      } else {
        tracee->Terminate([weak = weak_ptr_factory_.GetWeakPtr(), connection]() {
          if (weak) {
            weak->OnV2ProviderTerminated(connection);
          }
        });
      }
    }
  }
}

// Called when a provider state change is detected.
// This includes "failed" as well as "started".
void TraceSession::CheckAllProvidersStarted() {
  FX_DCHECK(state_ == State::kStarting);

  const bool all_started = std::accumulate(
      tracees_.begin(), tracees_.end(), true, [](bool value, const auto& tracee_variant) {
        bool ready =
            std::visit(overloaded{[](const std::unique_ptr<Tracee>& tracee) {
                                    bool ready = (tracee->state() == Tracee::State::kStarted ||
                                                  // If a provider fails to start continue tracing.
                                                  // We warn which providers failed to start in the
                                                  // timeout handling.
                                                  tracee->state() == Tracee::State::kStopped);
                                    FX_LOGS(DEBUG) << "tracee " << *tracee->bundle()
                                                   << (ready ? "" : " not") << " ready";
                                    return ready;
                                  },
                                  [](const std::unique_ptr<TraceeV2>& tracee) {
                                    bool ready = (tracee->state() == Tracee::State::kStarted ||
                                                  // If a provider fails to start continue tracing.
                                                  // We warn which providers failed to start in the
                                                  // timeout handling.
                                                  tracee->state() == Tracee::State::kStopped);
                                    FX_LOGS(DEBUG) << std::format("v2 tracee {} {} ready",
                                                                  *(tracee->connection()),
                                                                  (ready ? "" : " not"));
                                    return ready;
                                  }},
                       tracee_variant);
        return value && ready;
      });

  if (all_started) {
    FX_LOGS(DEBUG) << "All providers reporting started";
    NotifyStarted();
  }
}

void TraceSession::NotifyStarted() {
  TransitionToState(State::kStarted);
  if (start_callback_) {
    FX_LOGS(DEBUG) << "Marking session as having started";
    session_start_timeout_.Cancel();
    auto callback = std::move(start_callback_);
    callback(fit::ok());
  }
}

void TraceSession::OnProviderStopped(TraceProviderBundle* bundle, bool write_results) {
  auto it = std::find_if(tracees_.begin(), tracees_.end(), [bundle](const auto& tracee_variant) {
    if (std::holds_alternative<std::unique_ptr<Tracee>>(tracee_variant)) {
      return *std::get<std::unique_ptr<Tracee>>(tracee_variant) == bundle;
    }
    return false;
  });

  if (write_results) {
    if (it != tracees_.end()) {
      auto& tracee = std::get<std::unique_ptr<Tracee>>(*it);
      if (!tracee->results_written()) {
        if (!WriteProviderData(tracee.get())) {
          FX_LOGS(INFO) << "Aborting in OnProviderStopped due to WriteProviderData failed for "
                        << *tracee->bundle();
          Abort();
          return;
        }
      }
    }
  }

  if (state_ == State::kStopped) {
    // Late stop notification, nothing more to do.
  } else if (state_ == State::kStopping) {
    CheckAllProvidersStopped();
  } else {
    // Tracing may have terminated in the interim.
    if (it != tracees_.end()) {
      if (state_ == State::kTerminating) {
        std::get<std::unique_ptr<Tracee>>(*it)->Terminate();
      }
    }
  }
}

void TraceSession::OnV2ProviderStopped(ProviderConnection* connection, bool write_results) {
  auto it = std::ranges::find_if(tracees_, [connection](const auto& tracee_variant) {
    if (std::holds_alternative<std::unique_ptr<TraceeV2>>(tracee_variant)) {
      return *std::get<std::unique_ptr<TraceeV2>>(tracee_variant) == connection;
    }
    return false;
  });

  if (write_results) {
    if (it != tracees_.end()) {
      auto& tracee = std::get<std::unique_ptr<TraceeV2>>(*it);
      if (!tracee->results_written()) {
        if (!WriteV2ProviderData(tracee.get())) {
          Abort();
          return;
        }
      }
    }
  }

  if (state_ == State::kStopped) {
    // Late stop notification, nothing more to do.
  } else if (state_ == State::kStopping) {
    CheckAllProvidersStopped();
  } else {
    // Tracing may have terminated in the interim.
    if (it != tracees_.end()) {
      if (state_ == State::kTerminating) {
        std::get<std::unique_ptr<TraceeV2>>(*it)->Terminate(
            [weak = weak_ptr_factory_.GetWeakPtr(), connection]() {
              if (weak) {
                weak->OnV2ProviderTerminated(connection);
              }
            });
      }
    }
  }
}

void TraceSession::CheckAllProvidersStopped() {
  FX_DCHECK(state_ == State::kStopping);

  const bool all_stopped = std::accumulate(
      tracees_.begin(), tracees_.end(), true, [](bool value, const auto& tracee_variant) {
        bool stopped =
            std::visit(overloaded{[](const std::unique_ptr<Tracee>& tracee) {
                                    bool stopped = tracee->state() == Tracee::State::kStopped;
                                    FX_LOGS(DEBUG) << "tracee " << *tracee->bundle()
                                                   << (stopped ? "" : " not") << " stopped";
                                    return stopped;
                                  },
                                  [](const std::unique_ptr<TraceeV2>& tracee) {
                                    bool stopped = tracee->state() == Tracee::State::kStopped;
                                    FX_LOGS(DEBUG) << std::format("v2 tracee {} {} stopped",
                                                                  *(tracee->connection()),
                                                                  (stopped ? "" : " not"));
                                    return stopped;
                                  }},
                       tracee_variant);
        return value && stopped;
      });

  if (all_stopped) {
    FX_LOGS(DEBUG) << "All providers reporting stopped";
    FX_LOGS(INFO) << "Flushing to socket";
    buffer_forwarder_->Flush();

    TransitionToState(State::kStopped);
    NotifyStopped();
  }
}

void TraceSession::NotifyStopped() {
  FX_DCHECK(state_ == State::kStopped);

  if (stop_callback_) {
    FX_LOGS(DEBUG) << "Marking session as having stopped";
    session_stop_timeout_.Cancel();
    for (auto& tracee_variant : tracees_) {
      std::visit(overloaded{[this](const std::unique_ptr<Tracee>& tracee) {
                              if (auto trace_stat = tracee->GetStats(); trace_stat.has_value()) {
                                trace_stats_.push_back(std::move(trace_stat.value()));
                              } else {
                                FX_LOGS(WARNING) << "No stats generated for " << tracee->bundle();
                              }
                            },
                            [this](const std::unique_ptr<TraceeV2>& tracee) {
                              if (auto trace_stat = tracee->GetStats(); trace_stat.has_value()) {
                                trace_stats_.push_back(std::move(trace_stat.value()));
                              } else {
                                FX_LOGS(WARNING) << std::format("No stats generated for {}",
                                                                *(tracee->connection()));
                              }
                            }},
                 tracee_variant);
    }

    FX_LOGS(INFO) << "Writing " << trace_stats_.size() << " trace stats to result.";

    fuchsia_tracing_controller::StopResult result;
    result.provider_stats(std::move(trace_stats_));
    auto callback = std::move(stop_callback_);
    FX_DCHECK(callback);
    callback(fit::ok(std::move(result)));
    FX_LOGS(INFO) << "Stop() callback complete.";
  } else {
    FX_LOGS(WARNING) << "Stop() did not provide a callback??";
  }
}

void TraceSession::OnProviderTerminated(TraceProviderBundle* bundle) {
  auto it = std::find_if(tracees_.begin(), tracees_.end(), [bundle](const auto& tracee_variant) {
    if (std::holds_alternative<std::unique_ptr<Tracee>>(tracee_variant)) {
      return *std::get<std::unique_ptr<Tracee>>(tracee_variant) == bundle;
    }
    return false;
  });

  if (it != tracees_.end()) {
    if (write_results_on_terminate_) {
      auto& tracee = std::get<std::unique_ptr<Tracee>>(*it);
      // If the last Stop request saved the results, don't save them again.
      // But don't write results if the tracee was never started.
      if (tracee->was_started() && !tracee->results_written()) {
        // Since the provider is terminated, ignore any error writing
        // provider data. This is most likely caused by the socket
        // being closed. WriteProviderData will log the details.
        WriteProviderData(tracee.get());
      }
    }
    tracees_.erase(it);
  }

  if (state_ == State::kStarting) {
    // A trace provider may have disconnected without having first successfully
    // started. Check whether all remaining providers have now started so that
    // we can transition to |kStarted|.
    CheckAllProvidersStarted();
  } else if (state_ == State::kStopping) {
    // A trace provider may have disconnected without having been marked as
    // stopped. Check whether all remaining providers have now stopped.
    CheckAllProvidersStopped();
  }

  TerminateSessionIfEmpty();
}

void TraceSession::OnV2ProviderTerminated(ProviderConnection* connection) {
  auto it = std::ranges::find_if(tracees_, [connection](const auto& tracee_variant) {
    if (std::holds_alternative<std::unique_ptr<TraceeV2>>(tracee_variant)) {
      return *std::get<std::unique_ptr<TraceeV2>>(tracee_variant) == connection;
    }
    return false;
  });

  if (it != tracees_.end()) {
    if (write_results_on_terminate_) {
      auto& tracee = std::get<std::unique_ptr<TraceeV2>>(*it);
      // If the last Stop request saved the results, don't save them again.
      // But don't write results if the tracee was never started.
      if (tracee->was_started() && !tracee->results_written()) {
        // Since the provider is terminated, ignore any error writing
        // provider data. This is most likely caused by the socket
        // being closed. WriteProviderData will log the details.
        WriteV2ProviderData(tracee.get());
      }
    }
    tracees_.erase(it);
  }

  if (state_ == State::kStarting) {
    // A trace provider may have disconnected without having first successfully
    // started. Check whether all remaining providers have now started so that
    // we can transition to |kStarted|.
    CheckAllProvidersStarted();
  } else if (state_ == State::kStopping) {
    // A trace provider may have disconnected without having been marked as
    // stopped. Check whether all remaining providers have now stopped.
    CheckAllProvidersStopped();
  }

  TerminateSessionIfEmpty();
}

void TraceSession::TerminateSessionIfEmpty() {
  if (state_ == State::kTerminating && tracees_.empty()) {
    FX_LOGS(DEBUG) << "Marking session as terminated, no more tracees";

    session_terminate_timeout_.Cancel();
    auto callback = std::move(terminate_callback_);
    FX_DCHECK(callback);
    callback();
  }
}

void TraceSession::SessionStartTimeout(async_dispatcher_t* dispatcher, async::TaskBase* task,
                                       zx_status_t status) {
  FX_LOGS(WARNING) << "Timed out waiting for one or more providers to ack the start request";
  for (auto& tracee_variant : tracees_) {
    std::visit(overloaded{[](const std::unique_ptr<Tracee>& tracee) {
                            if (tracee->state() != Tracee::State::kStarted) {
                              FX_LOGS(WARNING) << "Timed out waiting for trace provider "
                                               << *tracee->bundle() << " to start";
                            }
                          },
                          [](const std::unique_ptr<TraceeV2>& tracee) {
                            if (tracee->state() != Tracee::State::kStarted) {
                              FX_LOGS(WARNING)
                                  << std::format("Timed out waiting for trace provider {} to start",
                                                 *tracee->connection());
                            }
                          }},
               tracee_variant);
  }
  NotifyStarted();
}

void TraceSession::SessionStopTimeout(async_dispatcher_t* dispatcher, async::TaskBase* task,
                                      zx_status_t status) {
  FX_LOGS(WARNING) << "Timed out waiting for one or more providers to ack the stop request";

  if (state_ == State::kStopping) {
    FX_LOGS(DEBUG) << "Marking session as stopped, timed out waiting for tracee(s)";
    TransitionToState(State::kStopped);
    for (auto& tracee_variant : tracees_) {
      std::visit(overloaded{[](const std::unique_ptr<Tracee>& tracee) {
                              if (tracee->state() != Tracee::State::kStopped) {
                                FX_LOGS(WARNING) << "Timed out waiting for trace provider "
                                                 << *tracee->bundle() << " to stop";
                              }
                            },
                            [](const std::unique_ptr<TraceeV2>& tracee) {
                              if (tracee->state() != Tracee::State::kStopped) {
                                FX_LOGS(WARNING) << std::format(
                                    "Timed out waiting for trace provider {} to stop",
                                    *(tracee->connection()));
                              }
                            }},
                 tracee_variant);
    }

    FX_LOGS(INFO) << "Flushing to socket";
    buffer_forwarder_->Flush();
    NotifyStopped();
  }
}

void TraceSession::SessionTerminateTimeout(async_dispatcher_t* dispatcher, async::TaskBase* task,
                                           zx_status_t status) {
  FX_LOGS(WARNING) << "Timed out waiting for one or more providers to ack the terminate request";

  // We do not consider pending_start_tracees_ here as we only
  // terminate them as a best effort.
  if (state_ == State::kTerminating && !tracees_.empty()) {
    FX_LOGS(DEBUG) << "Marking session as terminated, timed out waiting for tracee(s)";

    for (auto& tracee_variant : tracees_) {
      std::visit(overloaded{[](const std::unique_ptr<Tracee>& tracee) {
                              if (tracee->state() != Tracee::State::kTerminated) {
                                FX_LOGS(WARNING) << "Timed out waiting for trace provider "
                                                 << tracee->bundle() << " to terminate";
                              }
                            },
                            [](const std::unique_ptr<TraceeV2>& tracee) {
                              if (tracee->state() != Tracee::State::kTerminated) {
                                FX_LOGS(WARNING) << "Timed out waiting for trace provider "
                                                 << tracee->connection() << " to terminate";
                              }
                            }},
                 tracee_variant);
    }
    auto callback = std::move(terminate_callback_);
    FX_DCHECK(callback);
    callback();
  }
}

void TraceSession::RemoveDeadProvider(TraceProviderBundle* bundle) { OnProviderTerminated(bundle); }

void TraceSession::RemoveDeadV2Provider(ProviderConnection* connection) {
  OnV2ProviderTerminated(connection);
}

void TraceSession::FlushProviders() {
  if (!(state_ == State::kStarted || state_ == State::kStarting)) {
    return;
  }
  for (auto& tracee : tracees_) {
    std::visit(overloaded{[](const std::unique_ptr<Tracee>& tracee) {},
                          [](const std::unique_ptr<TraceeV2>& tracee) {
                            if (zx::result res = tracee->RequestFlush(); res.is_error()) {
                              FX_PLOGS(ERROR, res.status_value()) << std::format(
                                  "Failed to flush provider: {}", *tracee->connection());
                            }
                          }},
               tracee);
  }
}

bool TraceSession::WriteProviderData(Tracee* tracee) {
  FX_DCHECK(!tracee->results_written());

  switch (tracee->TransferRecords()) {
    case TransferStatus::kComplete:
      break;
    case TransferStatus::kProviderError:
      FX_LOGS(ERROR) << "Problem reading provider socket output, skipping";
      break;
    case TransferStatus::kWriteError:
      FX_LOGS(ERROR) << "Encountered unrecoverable error writing socket";
      return false;
    case TransferStatus::kReceiverDead:
      FX_LOGS(ERROR) << "Consumer socket peer is closed";
      return false;
    default:
      __UNREACHABLE;
      break;
  }

  return true;
}

bool TraceSession::WriteV2ProviderData(TraceeV2* tracee) {
  FX_DCHECK(!tracee->results_written());

  switch (tracee->TransferRecords()) {
    case TransferStatus::kComplete:
      break;
    case TransferStatus::kProviderError:
      FX_LOGS(ERROR) << "Problem reading provider socket output, skipping";
      break;
    case TransferStatus::kWriteError:
      FX_LOGS(ERROR) << "Encountered unrecoverable error writing socket";
      return false;
    case TransferStatus::kReceiverDead:
      FX_LOGS(ERROR) << "Consumer socket peer is closed";
      return false;
    default:
      __UNREACHABLE;
      break;
  }

  return true;
}

void TraceSession::Abort() {
  FX_LOGS(DEBUG) << "Fatal error occurred, aborting session";
  write_results_on_terminate_ = false;
  if (stop_callback_) {
    TransitionToState(State::kStopped);
    session_stop_timeout_.Cancel();
    auto callback = std::move(stop_callback_);
    FX_DCHECK(callback);
    callback(fit::error(fuchsia_tracing_controller::StopError::kAborted));
  }
  abort_handler_();
}

void TraceSession::WriteTraceInfo() {
  // This won't block as we're only called after the consumer connects, and
  // this is the first record written.
  if (auto status = buffer_forwarder_->WriteMagicNumberRecord();
      status != TransferStatus::kComplete) {
    FX_LOGS(ERROR) << "Failed to write magic number record: " << status;
  }
}

void TraceSession::TransitionToState(State new_state) {
  FX_LOGS(DEBUG) << "Transitioning from " << state_ << " to " << new_state;
  state_ = new_state;
}

std::ostream& operator<<(std::ostream& out, TraceSession::State state) {
  switch (state) {
    case TraceSession::State::kInitialized:
      out << "initialized";
      break;
    case TraceSession::State::kStarting:
      out << "starting";
      break;
    case TraceSession::State::kStarted:
      out << "started";
      break;
    case TraceSession::State::kStopping:
      out << "stopping";
      break;
    case TraceSession::State::kStopped:
      out << "stopped";
      break;
    case TraceSession::State::kTerminating:
      out << "terminating";
      break;
  }

  return out;
}

}  // namespace tracing
