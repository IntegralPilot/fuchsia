// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/media/cpp/fidl.h>
#include <fuchsia/mediacodec/cpp/fidl.h>
#include <inttypes.h>
#include <lib/async/cpp/task.h>
#include <lib/closure-queue/closure_queue.h>
#include <lib/fidl/cpp/clone.h>
#include <lib/fit/defer.h>
#include <lib/media/codec_impl/codec_impl.h>
#include <lib/media/codec_impl/codec_vmo_range.h>
#include <lib/media/codec_impl/log.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <stdarg.h>
#include <threads.h>
#include <zircon/compiler.h>
#include <zircon/threads.h>

#include <mutex>
#include <optional>
#include <variant>

#include <bind/fuchsia/sysmem/heap/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/macros.h>

#include "lib/media/codec_impl/codec_port.h"
#include "src/media/lib/codec_impl/dispatcher.h"
#include "src/media/lib/codec_impl/utils.h"

#include <src/media/lib/metrics/metrics.cb.h>

// "is_bound_checks" - In several places that send a message, we check is_bound() first, only
// because of ZX_POL_BAD_HANDLE ZX_POL_ACTION_EXCEPTION which typically only applies in a driver
// context. If it weren't for that, we wouldn't care about passing ZX_HANDLE_INVALID to
// zx_channel_write(), since the channel error handling is async (we Unbind(), sweep the in-proc
// send queue, and only then delete the Binding). This is only about ZX_HANDLE_INVALID specifically,
// not arbitrary incorrect zx_handle_t values (for those, ZX_POL_BAD_HANDLE ZX_POL_ACTION_EXCEPTION
// is great).

namespace {

constexpr bool kLogTimestampDelay = false;

// The protocol does not permit an unbounded number of in-flight streams, as
// channel with no valid circuit-breaker value for the incoming channel data.
// that would potentially result in unbounded data queued in the incoming
constexpr size_t kMaxInFlightStreams = 10;

// We cap the number of buffer_lifetime_ordinal values that can still have any
// buffers active from the CodecAdapter's point of view. This is set large
// enough to permit a pathological test stream that changes dimensions every
// frame while keeping every frame in a DPB of size 32. This is more than
// actually required by any of h.264 (16 frames or, in theory, 32 fields if we
// supported interlaced decode), HEVC (16), VP9 (8), AV1 (8).
constexpr size_t kMaxActiveBufferLifetimeOrdinals = 48;

// The intent is for this to be low enough to avoid using too much time in a
// single dispatch due to actions of an adversarial client. Along with this
// limit, a CodecAdapter is allowed to have a lower limit if necessary due to
// FW/HW limitations (beyond just needing the driver to queue before letting the
// FW know about a non-empty input buffer or free output buffer).
//
// Increasing this value is easier than decreasing it, so we'll start this on
// the lower end of what might be considered a plausibly-sufficient range, and
// we can increase this as actually needed.
//
// Clients shouldn't interpret this number as a challenge; using more buffers
// than necessary can result in lower performance (due to stuff like IOMMU TLB
// hit rate).
constexpr uint32_t kMaxDynamicBuffersPerPort = 64;

// At least for now, input never changes its buffer_constraints_version_ordinal.
constexpr uint64_t kInputBufferConstraintsVersionOrdinal = 1;

constexpr CodecPort kPorts[] = {kInputPort, kOutputPort};

bool IsStreamErrorRecoverable(fuchsia::media::StreamError e) {
  using StreamError = fuchsia::media::StreamError;
  switch (e) {
    case StreamError::DECRYPTOR_NO_KEY:
      return true;
    default:
      return false;
  }
}

const char* ToString(fuchsia::media::StreamError e) {
  using StreamError = fuchsia::media::StreamError;
  switch (e) {
    case StreamError::UNKNOWN:
      return "UNKNOWN";
    case StreamError::INVALID_INPUT_FORMAT_DETAILS:
      return "INVALID_INPUT_FORMAT_DETAILS";
    case StreamError::INCOMPATIBLE_BUFFERS_PROVIDED:
      return "INCOMPATIBLE_BUFFERS_PROVIDED";
    case StreamError::EOS_PROCESSING:
      return "EOS_PROCESSING";
    case StreamError::DECODER_UNKNOWN:
      return "DECODER_UNKNOWN";
    case StreamError::DECODER_DATA_PARSING:
      return "DECODER_DATA_PARSING";
    case StreamError::ENCODER_UNKNOWN:
      return "ENCODER_UNKNOWN";
    case StreamError::DECRYPTOR_UNKNOWN:
      return "DECRYPTOR_UNKNOWN";
    case StreamError::DECRYPTOR_NO_KEY:
      return "DECRYPTOR_NO_KEY";
  }
}

const char* GetStreamErrorAdditionalHelpText(fuchsia::media::StreamError e) {
  using StreamError = fuchsia::media::StreamError;
  switch (e) {
    case StreamError::DECRYPTOR_NO_KEY:
      return "Retry after keys arrive.";
    default:
      return "";
  }
}

std::optional<CodecPort> CodecPortFromFidlPort(fuchsia::media::Port port) {
  switch (port) {
    case fuchsia::media::Port::INPUT:
      return CodecPort::kInputPort;
    case fuchsia::media::Port::OUTPUT:
      return CodecPort::kOutputPort;
    default:
      return std::nullopt;
  }
}

}  // namespace

// std::unique_lock<> doesn't have thread annotations, but the "owns_lock()" call is useful, and
// condition variables require std::unique_lock<>, so we wrap std::unique_lock<> here.
class __TA_SCOPED_CAPABILITY ScopedLock {
 public:
  ScopedLock(std::mutex& lock) __TA_ACQUIRE(lock) __TA_ACQUIRE(mutex_)
      : mutex_(lock), held_lock_(lock) {}
  ~ScopedLock() __TA_RELEASE() {}
  void AssertHeld(const std::mutex& lock_to_check) const __TA_ASSERT(lock_to_check)
      __TA_ASSERT(mutex_) {
    ZX_ASSERT(&lock_to_check == &mutex_);
    ZX_ASSERT(held_lock_.has_value());
    ZX_ASSERT(held_lock_->mutex() == &lock_to_check);
    ZX_ASSERT(held_lock_->owns_lock());
  }
  void lock() __TA_ACQUIRE(mutex_) {
    ZX_DEBUG_ASSERT(!held_lock_.has_value());
    mutex_.lock();
    held_lock_.emplace(mutex_, std::adopt_lock_t{});
  }
  void unlock() __TA_RELEASE(mutex_) {
    ZX_DEBUG_ASSERT(held_lock_.has_value());
    held_lock_->release();
    mutex_.unlock();
  }
  // Full TA annotation for this one is tricky (possibly infeasible without changing now TA
  // annotations work), but we really only use this for condition variable waits, so it's not
  // critical that the TA annotations are perfect for this call since nothing else relevant happens
  // during a condition variable wait.
  std::unique_lock<std::mutex>& unique_lock() __TA_REQUIRES(mutex_) {
    ZX_DEBUG_ASSERT(held_lock_.has_value());
    return *held_lock_;
  }

 private:
  friend class CodecImpl;

  std::mutex& mutex_;
  std::optional<std::unique_lock<std::mutex>> held_lock_;

  DISALLOW_COPY_ASSIGN_AND_MOVE(ScopedLock);
};

CodecImpl::CodecImpl(fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem,
                     std::unique_ptr<CodecAdmission> codec_admission,
                     async_dispatcher_t* shared_fidl_dispatcher, thrd_t shared_fidl_thread,
                     StreamProcessorParams params,
                     fidl::InterfaceRequest<fuchsia::media::StreamProcessor> request)
    // The parameters to CodecAdapter constructor here aren't important.
    : CodecAdapter(lock_, this),
      codec_admission_(std::move(codec_admission)),
      shared_fidl_dispatcher_(shared_fidl_dispatcher),
      shared_fidl_queue_(shared_fidl_dispatcher),
      checker_fidl_(shared_fidl_dispatcher),
      params_(std::move(params)),
      tmp_sysmem_(std::move(sysmem)),
      tmp_interface_request_(std::move(request)),
      binding_(this) {
  ZX_DEBUG_ASSERT(tmp_sysmem_);
  ZX_DEBUG_ASSERT(tmp_interface_request_);

  if (codec_admission_) {
    codec_admission_->SetChannelToWaitOn(tmp_interface_request_.channel());
  }

  // This is the binding_'s error handler, not the owner_error_handler_ which
  // is related but separate.
  //
  // If client code instead does ~CodecImpl, this error handler is prevented from running by
  // synchronously unbinding in ~CodecImpl.
  binding_.set_error_handler([this](zx_status_t status) {
    // This handler can't run until after binding_ is bound.
    ZX_DEBUG_ASSERT(was_logically_bound_);
    Unbind();
  });

  initial_input_format_details_ = IsDecoder()   ? &decoder_params().input_details()
                                  : IsEncoder() ? &encoder_params().input_details()
                                                : &decryptor_params().input_details();
}

CodecImpl::~CodecImpl() {
  // We need ~binding_ to run on fidl_thread() else it's not safe to
  // un-bind unilaterally.  We could potentially relax this if BindAsync() was
  // never called, but for now we just require this always.
  ZX_DEBUG_ASSERT(IsFidl());

  // See UnbindAsync() and the BindAsync() error handler. The non-legacy way
  // never allows ~CodecImpl until after the client error handler has run.
  ZX_DEBUG_ASSERT(is_client_error_handler_called_ || IsLegacyUnbind());

  // This enforces that we won't end up waiting on StreamControl which may in turn wait on this
  // thread, which would be a deadlock. If this fires, make sure the AsyncUnbind() callback has at
  // least started running before calling ~CodecImpl (not just the call to AsyncUnbind()).
  ZX_ASSERT(was_unbind_completed_ || !is_sharing_fidl_domain_for_core_codec_);

  if (was_logically_bound_) {
    {  // scope lock
      ScopedLock lock(lock_);
      // Ensure that StreamControl is told to stop, which also stops InputData by calling
      // EnsureStreamClosed() as needed.
      //
      // This does almost nothing if the current stack is running in response to CodecImpl calling
      // the BindAsync error handler (sync on same stack or async asap afterward).
      UnbindLocked();

      // Wait for StreamControl to be done.
      //
      // Normally the fidl_thread() waiting for the StreamControl thread to do anything would be
      // bad, because the fidl_thread() is non-blocking and the StreamControl thread can block on
      // stuff, but StreamControl thread behavior after was_unbind_started_ = true and
      // wake_stream_control_condition_.notify_all() does not block and does not wait on
      // fidl_thread().  So in this case it's ok to wait here.
      //
      // If the BindAsync error handler is/was called, this condition will already be set/true.
      while (!is_stream_control_done_) {
        stream_control_done_condition_.wait(lock.unique_lock());
      }
    }  // ~lock

    // This does almost nothing if the current stack is running in response to CodecImpl calling
    // the BindAsync error handler (sync on same stack or async asap afterward).
    EnsureUnbindCompleted();
  }

  // Ensure the CodecAdmission is deleted entirely after ~this, including after any relevant base
  // class destructors have run.  This posted work may only get deleted, not run, since some
  // environments will Quit() their async::Loop shortly after ~CodecImpl.  So to avoid depending on
  // the destruction order of captures of a lambda, we use a fit::defer which will run it's lambda
  // when deleted.  In this lambda we can force ~CodecAdmission before ~zx::channel, and we know
  // this lambda will run, whether the lambda further down runs or is just deleted.
  auto run_when_deleted = fit::defer([codec_admission = std::move(codec_admission_),
                                      codec_to_close = std::move(codec_to_close_)]() mutable {
    // Ensure codec_to_close is destructed only after the codec_admission is destructed.  We have
    // to be fairly explicit about this since the order of lambda members is explicitly
    // unspecified in C++, so their destruction order is also unspecified.
    //
    // We care about the order because a client is fairly likely to immediately retry on seeing
    // the channel close, and we don't want that to ever bounce off the CodecAdmission for the
    // instance associated with that same channel.
    codec_admission = nullptr;

    // ~codec_to_close (after ~CodecAdmission above).
  });
  // We intentionally don't use shared_fidl_queue_ here.
  PostSerial(shared_fidl_dispatcher_, [run_when_deleted = std::move(run_when_deleted)] {
    // ~run_when_deleted will run the lambda above, whether run at the end of this
    // lambda, or when this lambda is deleted without ever having run during ~async::Loop
    // or async::Loop::Shutdown() (or fdf::Dispatcher ShutdownAsync() actions).
  });
}

std::mutex& CodecImpl::lock() { return lock_; }

void CodecImpl::SetLifetimeTracking(std::vector<zx::eventpair> lifetime_tracking_eventpair) {
  ZX_DEBUG_ASSERT(lifetime_tracking_eventpair.size() <=
                  fuchsia::mediacodec::CODEC_FACTORY_LIFETIME_TRACKING_EVENTPAIR_PER_CREATE_MAX);
  lifetime_tracking_ = std::move(lifetime_tracking_eventpair);
}

void CodecImpl::SetCodecMetrics(CodecMetrics* codec_metrics) {
  ZX_DEBUG_ASSERT(codec_metrics);
  codec_metrics_ = codec_metrics;
}

void CodecImpl::SetCoreCodecAdapter(std::unique_ptr<CodecAdapter> codec_adapter) {
  ZX_DEBUG_ASSERT(codec_adapter);
  ZX_DEBUG_ASSERT(!codec_adapter_);
  codec_adapter_ = std::move(codec_adapter);
  is_supports_dynamic_buffers_ = codec_adapter_->IsSupportsDynamicBuffers();
  is_supports_mid_stream_output_constraints_change_ =
      codec_adapter_->IsSupportsMidStreamOutputConstraintsChange();
  // If this assert fails, it means the CodecAdapter supports dynamic buffers but
  // !kEnableDynamicBuffers in this build. To support a CodecAdapter that supports dynamic buffers
  // use a CodecImpl build target with kEnableDynamicBuffers true.
  ZX_ASSERT(is_supports_dynamic_buffers_ == is_supports_dynamic_buffers());
  codec_metrics_implementation_dimension_ = codec_adapter_->CoreCodecMetricsImplementation();
}

void CodecImpl::SetCodecDiagnostics(CodecDiagnostics* codec_diagnostics) {
  ZX_DEBUG_ASSERT(codec_adapter_);
  codec_adapter_->SetCodecDiagnostics(codec_diagnostics);
}

zx_koid_t GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

std::string GetObjectName(zx_handle_t handle) {
  char name[ZX_MAX_NAME_LEN];
  zx_status_t status = zx_object_get_property(handle, ZX_PROP_NAME, name, sizeof(name));
  return status == ZX_OK ? std::string(name) : std::string();
}

void CodecImpl::BindAsync(fit::closure error_handler) {
  // While it would potentially be safe to call Bind() from a thread other than
  // fidl_thread(), we have no reason to permit that.
  ZX_DEBUG_ASSERT(IsFidl());
  // Up to once only.  No reuse.
  ZX_DEBUG_ASSERT(!was_bind_async_called_);
  ZX_DEBUG_ASSERT(!binding_.is_bound());
  ZX_DEBUG_ASSERT(tmp_interface_request_);
  was_bind_async_called_ = true;

  // Give CodecAdapter a chance to set a scheduler profile. For historical
  // reasons this has to fall back to CoreCodecSetStreamControlProfile (below)
  // if CoreCodecGetSchedulerProfileName(OrderingDomain::StreamControl) returns
  // an empty string.
  std::string stream_control_scheduler_role =
      CoreCodecGetSchedulerProfileName(OrderingDomain::StreamControl);

  auto stream_control_dispatcher =
      codec_impl::DispatcherFactory::Create("StreamControl", stream_control_scheduler_role);
  if (!stream_control_dispatcher) {
    // Handle the error async, to be consistent with later errors that must
    // occur async anyway.  Inability to start StreamControl is the only case
    // where we just allow the owner to "delete this" without using
    // UnbindLocked(), since UnbindLocked() relies on StreamControl.
    PostToSharedFidl([this, error_handler = std::move(error_handler)] {
      is_client_error_handler_called_ = true;
      std::move(error_handler)();
    });
    return;
  }
  stream_control_dispatcher_ = std::move(stream_control_dispatcher);

  if (stream_control_scheduler_role.empty()) {
    std::optional<thrd_t> maybe_stream_control_thrd = stream_control_dispatcher_->maybe_thrd();
    if (maybe_stream_control_thrd.has_value()) {
      auto& stream_control_thrd = *maybe_stream_control_thrd;
      CoreCodecSetStreamControlProfile(zx::unowned_thread(thrd_get_zx_handle(stream_control_thrd)));
    }
  }

  libsync::Completion set_dispatcher_done;
  zx_status_t post_status =
      async::PostTask(stream_control_dispatcher_->dispatcher(), [this, &set_dispatcher_done] {
        // must be called on stream_control_dispatcher_
        checker_stream_control_.emplace(stream_control_dispatcher_->dispatcher(), "StreamControl");
        stream_control_queue_.SetDispatcher(stream_control_dispatcher_->dispatcher());
        set_dispatcher_done.Signal();
      });
  ZX_ASSERT(post_status == ZX_OK);
  set_dispatcher_done.Wait();

  // From here on, we'll only fail the CodecImpl via UnbindLocked(), or by
  // just calling ~CodecImpl on the FIDL thread.
  was_logically_bound_ = true;

  // This doesn't really need to be set until the start of the posted lambda
  // below, but here is also fine.
  owner_error_handler_ = std::move(error_handler);

  // Do most of the bind work on StreamControl async, since CoreCodecInit()
  // might potentially take a little while longer than makes sense to run on
  // fidl_thread().  Potential examples: if CoreCodecInit() ends up
  // essentially evicting some other CodecImpl, or if setting up HW can take a
  // while, or if getting a scheduling slot on decode HW can require some
  // waiting, or similar.
  PostToStreamControl([this] {
    // This is allowed to take a little while if necessary, using the current
    // StreamControl thread, which is not shared with any other CodecImpl.
    CoreCodecInit(*initial_input_format_details_);
    is_core_codec_init_called_ = true;

    CoreCodecSetSecureMemoryMode(kOutputPort, PortSecureMemoryMode(kOutputPort));
    CoreCodecSetSecureMemoryMode(kInputPort, PortSecureMemoryMode(kInputPort));

    bool force_new_buffers_for_new_dimensions = false;
    if (IsDecoder() && decoder_params().has_force_new_buffers_for_new_dimensions() &&
        decoder_params().force_new_buffers_for_new_dimensions()) {
      if (!is_supports_dynamic_buffers()) {
        Fail("force_new_buffers_for_new_dimensions true requires supports_dynamic_buffers");
        return;
      }
      force_new_buffers_for_new_dimensions = true;
    }
    if (force_new_buffers_for_new_dimensions) {
      CoreCodecSetForceNewBuffersOnNewDimensions(true);
    }

    if (is_supports_dynamic_buffers()) {
      for (auto port : kPorts) {
        dynamic_buffers_max_[port] = codec_adapter_->GetDynamicBuffersMax(port);
        ZX_DEBUG_ASSERT(dynamic_buffers_max_[port] != 0);
        dynamic_buffers_max_[port] =
            std::min(dynamic_buffers_max_[port], kMaxDynamicBuffersPerPort);
      }
    }

    if (IsCoreCodecHwBased(kInputPort) || IsCoreCodecHwBased(kOutputPort)) {
      core_codec_bti_ = CoreCodecBti();
    }

    auto input_constraints = std::make_unique<fuchsia::media::StreamBufferConstraints>();
    input_constraints->set_buffer_constraints_version_ordinal(
        kInputBufferConstraintsVersionOrdinal);
    TryFillDynamicStreamBufferConstraintsFields(kInputPort, *input_constraints);
    input_constraints_ = std::move(input_constraints);

    sent_buffer_constraints_version_ordinal_[kInputPort] =
        input_constraints_->buffer_constraints_version_ordinal();

    // We touch FIDL stuff only from the fidl_thread().
    //
    // Once this is posted, we can be dispatching incoming FIDL messages, concurrent with the rest
    // of the current lambda. Aside from Sync(), most of that dispatching would tend to land in
    // FailLocked(). The concurrency is just worth keeping in mind for the rest of the current
    // lambda is all.
    //
    // The client is not required to wait for OnInputConstraints before specifying input
    // buffer_constraints_version_ordinal 1, so it's important that
    // sent_buffer_constraints_version_ordinal_[kInputPort] is set to 1 above before binding.
    PostToSharedFidl([this] {
      // If the fuchsia::sysmem2::Allocator connection dies, so does this CodecImpl.
      //
      // In case of client code calling ~CodecImpl, this error handler is prevented from running by
      // synchronously unbinding in ~CodecImpl.
      sysmem_.Bind(
          std::move(tmp_sysmem_), shared_fidl_dispatcher_, [this](fidl::UnbindInfo unbind_info) {
            // This handler can't run until after sysmem_ is bound.
            ZX_DEBUG_ASSERT(was_logically_bound_);
            LogEvent(media_metrics::
                         StreamProcessorEvents2MigratedMetricDimensionEvent_SysmemChannelClosed);
            Fail("CodecImpl sysmem_ channel failed: %s", unbind_info.status_string());
          });
      ZX_DEBUG_ASSERT(!tmp_sysmem_);
      fuchsia_sysmem2::AllocatorSetDebugClientInfoRequest request;
      request.name() = GetObjectName(zx_process_self());
      request.id() = GetKoid(zx_process_self());
      auto set_debug_info_result = sysmem_->SetDebugClientInfo(std::move(request));
      if (!set_debug_info_result.is_ok()) {
        LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_FidlError);
        Fail("(*sysmem_)->SetDebugClientInfo() failed: %s",
             set_debug_info_result.error_value().status_string());
        return;
      }

      zx_status_t status =
          binding_.Bind(std::move(tmp_interface_request_), shared_fidl_dispatcher_);
      if (status != ZX_OK) {
        LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_FidlError);
        Fail("binding_.Bind() failed");
        return;
      }
      ZX_DEBUG_ASSERT(!tmp_interface_request_);

      // See "is_bound_checks" comment up top.
      if (binding_.is_bound()) {
        binding_.events().OnInputConstraints(fidl::Clone(*input_constraints_));
      }
    });
  });
}

void CodecImpl::EnableOnStreamFailed() {
  ZX_DEBUG_ASSERT(IsFidl());
  std::lock_guard<std::mutex> lock(lock_);
  is_on_stream_failed_enabled_ = true;
}

void CodecImpl::AddInputBuffer_StreamControl(CodecBuffer::Info buffer_info,
                                             CodecVmoRange vmo_range) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  if (IsStopping()) {
    return;
  }
  // We must check, because __WARN_UNUSED_RESULT, and it's worth it for the
  // enforcement and consistency.
  if (!AddNonDynamicBufferCommon(std::move(buffer_info), std::move(vmo_range))) {
    return;
  }
}

void CodecImpl::SetInputBufferPartialSettings(
    fuchsia::media::StreamBufferPartialSettings input_settings) {
  ZX_DEBUG_ASSERT(IsFidl());
  is_force_output_buffers_fixed_image_size_message_permitted_ = false;
  LogEvent(media_metrics::
               StreamProcessorEvents2MigratedMetricDimensionEvent_InputBufferAllocationStarted);
  PostToStreamControl([this, input_settings = std::move(input_settings)]() mutable {
    SetInputBufferPartialSettings_StreamControl(std::move(input_settings));
  });
}

void CodecImpl::SetInputBufferPartialSettings_StreamControl(
    fuchsia::media::StreamBufferPartialSettings input_partial_settings) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  {  // scope lock
    ScopedLock lock(lock_);
    ZX_DEBUG_ASSERT(sysmem_.is_valid());
    SetInputBufferSettingsCommon(lock, &input_partial_settings);
  }  // ~lock
}

void CodecImpl::SetInputBufferSettingsCommon(
    ScopedLock& lock, fuchsia::media::StreamBufferPartialSettings* input_partial_settings) {
  lock.AssertHeld(lock_);
  if (IsStoppingLocked()) {
    return;
  }
  if (IsStreamActiveLocked()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("client sent SetInputBuffer*Settings() with stream active");
    return;
  }
  SetBufferSettingsCommon(lock, kInputPort, input_partial_settings, *input_constraints_);
}

void CodecImpl::SetOutputBufferSettingsCommon(
    ScopedLock& lock, fuchsia::media::StreamBufferPartialSettings* output_partial_settings) {
  lock.AssertHeld(lock_);
  if (!output_constraints_) {
    // invalid client behavior
    //
    // client must have received at least the initial OnOutputConstraints()
    // first before sending SetOutputBufferSettings().
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked(
        "client sent SetOutputBufferSettings()/SetOutputBufferPartialSettings()"
        " when no output_constraints_");
    return;
  }

  // For a mid-stream output format change, this also enforces that the client
  // can only catch up to the mid-stream format change once.  In other words,
  // if the client has already caught up to the mid-stream config change, the
  // client no longer has an excuse to re-configure again with a stream
  // active.
  //
  // There's a check in SetBufferSettingsCommonLocked() that ignores this
  // message if the client's buffer_constraints_version_ordinal is behind
  // last_required_buffer_constraints_version_ordinal_, which gets updated
  // under the same lock hold interval as the server's de-configuring of
  // output buffers.
  //
  // There's a check in SetBufferSettingsCommonLocked() that closes the
  // channel if the client is sending a buffer_constraints_version_ordinal
  // that's newer than the last sent_buffer_constraints_version_ordinal_.
  if (IsStreamActiveLocked() && IsOutputConfiguredLocked()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked(
        "client sent SetOutputBufferSettings()/SetOutputBufferPartialSettings()"
        " with IsStreamActiveLocked() + already-fully-configured output");
    return;
  }

  SetBufferSettingsCommon(lock, kOutputPort, output_partial_settings,
                          output_constraints_->buffer_constraints());
}

void CodecImpl::AddOutputBufferInternal(CodecBuffer::Info buffer_info, CodecVmoRange vmo_range) {
  ZX_DEBUG_ASSERT(IsFidl());

  bool output_buffers_done_configuring =
      AddNonDynamicBufferCommon(std::move(buffer_info), std::move(vmo_range));
  if (output_buffers_done_configuring) {
    // The StreamControl domain _might_ be waiting for output to be configured.
    wake_stream_control_condition_.notify_all();
  }
}

void CodecImpl::SetOutputBufferPartialSettings(
    fuchsia::media::StreamBufferPartialSettings output_partial_settings) {
  ZX_DEBUG_ASSERT(IsFidl());
  is_force_output_buffers_fixed_image_size_message_permitted_ = false;
  VLOGF("CodecImpl::SetOutputBufferPartialSettings");
  {  // scope lock
    ScopedLock lock(lock_);
    if (!sysmem_.is_valid()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "client sent SetOutputBufferPartialSettings() to a CodecImpl "
          "that lacks a sysmem_");
      return;
    }
    SetOutputBufferSettingsCommon(lock, &output_partial_settings);
  }  // ~lock
}

void CodecImpl::CompleteOutputBufferPartialSettings(uint64_t buffer_lifetime_ordinal) {
  ZX_DEBUG_ASSERT(IsFidl());
  {  // scope lock
    ScopedLock lock(lock_);

    if (buffer_lifetime_ordinal % 2 == 0) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "CompleteOutputBufferPartialSettings client sent even "
          "buffer_lifetime_ordinal, but must be odd");
      return;
    }

    if (buffer_lifetime_ordinal != protocol_buffer_lifetime_ordinal_[kOutputPort]) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked("CompleteOutputBufferPartialSettings bad buffer_lifetime_ordinal");
      return;
    }

    // If the server is not interested in the client's buffer_lifetime_ordinal,
    // the client's buffer_lifetime_ordinal won't match the server's
    // buffer_lifetime_ordinal_.  The client will probably later catch up.
    if (buffer_lifetime_ordinal != buffer_lifetime_ordinal_[kOutputPort]) {
      // The case that ends up here is when a client's output configuration
      // (whole or last part) is being ignored because it's not yet caught up
      // with last_required_buffer_constraints_version_ordinal_.

      // Ignore the client's message.  The client will probably catch up later.
      return;
    }

    if (!IsPortAtLeastPartiallyConfiguredLocked(kOutputPort)) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "CompleteOutputBufferPartialSettings seen without prior "
          "SetOutputBufferPartialSettings");
      return;
    }

    if (port_settings_[kOutputPort]->is_complete_seen_output()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "CompleteOutputBufferPartialSettings permitted exactly once "
          "after each SetOutputBufferPartialSettings");
      return;
    }

    // This will cause IsOutputConfiguredLocked() to start returning true.
    port_settings_[kOutputPort]->SetCompleteSeenOutput();
  }  // ~lock
  wake_stream_control_condition_.notify_all();
}

void CodecImpl::FlushEndOfStreamAndCloseStream(uint64_t stream_lifetime_ordinal) {
  ZX_DEBUG_ASSERT(IsFidl());
  LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_StreamFlushed);
  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);
    if (!EnsureFutureStreamFlushSeenLocked(stream_lifetime_ordinal)) {
      return;
    }
  }
  // A mid-stream constraints change and output buffers reallocation can happen while waiting for
  // output EOS on StreamControl, and is processed as such. The main benefit of this is a
  // StreamControl stack crawl that indicates what's currently happening, with the calls on the
  // stack matching the logical nesting of the operations (wait for EOS is larger scope than stream
  // driven output buffer re-config, and the former calls the latter when the latter happens while
  // waiting for EOS). More than one output buffer re-config can happen while waiting for EOS if
  // that's what the stream specifies.
  PostToStreamControl([this, stream_lifetime_ordinal] {
    FlushEndOfStreamAndCloseStream_StreamControl(stream_lifetime_ordinal);
  });
}

void CodecImpl::FlushEndOfStreamAndCloseStream_StreamControl(uint64_t stream_lifetime_ordinal) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  {  // scope lock
    ScopedLock lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }

    // We re-check some things which were already future-verified a different
    // way, to allow for flexibility in the future-tracking stuff to permit less
    // checking in the Output ordering domain (fidl_thread()) without
    // breaking overall verification of a flush.  Any checking in the Output
    // ordering domain is for the future-tracking's own convenience only. The
    // checking here is the real checking.

    if (!CheckStreamLifetimeOrdinalLocked(stream_lifetime_ordinal)) {
      return;
    }
    ZX_DEBUG_ASSERT(stream_lifetime_ordinal >= stream_lifetime_ordinal_);
    if (!IsStreamActiveLocked() || stream_lifetime_ordinal != stream_lifetime_ordinal_) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "FlushEndOfStreamAndCloseStream() only valid on an active current "
          "stream (flush does not auto-create a new stream)");
      return;
    }
    // At this point we know that the stream is not discarded, and not already
    // flushed previously (because flush will discard the stream as there's
    // nothing more that the stream is permitted to do).
    ZX_DEBUG_ASSERT(IsStreamActiveLocked());
    ZX_DEBUG_ASSERT(stream_->stream_lifetime_ordinal() == stream_lifetime_ordinal);
    if (stream_->failure_seen()) {
      // Already reported to client.
      return;
    }
    if (!stream_->input_end_of_stream()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "FlushEndOfStreamAndCloseStream() is only permitted after "
          "QueueInputEndOfStream()");
      return;
    }
    auto ensure_closed = fit::defer([this, &lock] {
      // Now that flush is done (or we noticed a stream failure), we close the current stream
      // because there is not any subsequent message for the current stream that's valid.
      lock.AssertHeld(lock_);
      EnsureStreamClosed(lock);
    });
    zx::time wait_for_output_eos_start = zx::clock::get_monotonic();
    zx::time last_warn_time = wait_for_output_eos_start;
    while (!stream_->output_end_of_stream()) {
      if (stream_->failure_seen()) {
        return;
      }
      // While waiting, we'll continue to send OnOutputPacket(),
      // OnOutputConstraints(), and continue to process RecycleOutputPacket(),
      // until the client catches up to the latest config (as needed) and we've
      // started the send of output end_of_stream packet to the client.
      //
      // There is no way for the client to cancel a
      // FlushEndOfStreamAndCloseStream() short of closing the Codec channel.
      // Before long, the server will either send the OnOutputEndOfStream(), or
      // will send OnStreamFailed(), or will close the Codec channel.  The
      // server must do one of those things before long (not allowed to get
      // stuck while flushing).
      //
      // Some core codecs have no way to report mid-stream input data corruption
      // errors or similar without it being a stream failure, so if there's any
      // stream error it turns into OnStreamFailed(). It's also permitted for a
      // server to set error_detected_ bool(s) on output packets and send
      // OnOutputEndOfStream() despite detected errors, but this is only a
      // reasonable behavior for the server if the server normally would detect
      // and report mid-stream input corruption errors without an
      // OnStreamFailed().

      // CodecAdapter(s) are nominally required to ensure output EOS will happen
      // after input EOS, or to fail the stream, or to fail the codec, without
      // getting stuck. All of those options will signal this condition. In
      // addition, PostToStreamControlForOutput will also signal this condition.
      // However, we do require the output EOS to be seen within 30 seconds or
      // we time out and fail the codec instead. These timeouts should only be
      // used to notice the CodecAdapter taking too long to indicate output EOS,
      // not leaned on for anything else to be able to proceed.
      std::ignore =
          wake_stream_control_condition_.wait_for(lock.unique_lock(), std::chrono::seconds(5));
      zx::time now = zx::clock::get_monotonic();
      zx::duration duration_so_far = now - wait_for_output_eos_start;
      if (now - last_warn_time >= zx::sec(5)) {
        LOG(WARN, "output EOS taking too long - so far have waited %" PRId64 "s",
            duration_so_far.to_secs());
        last_warn_time = now;
      }
      if (duration_so_far > zx::sec(30)) {
        // Ideally this failure path wouldn't exist, but historically we've seen
        // video decoders get stuck at FW/HW layer when input data is fuzzed.
        //
        // Newly written codec drivers should not depend on this failure path
        // existing, and this may be removed once we've fixed up the remaining
        // codec drivers that need this.
        //
        // TODO(b/42119800): Put mitigation of getting stuck at FW/HW layer when
        // input is fuzzed in each impacted video decoder driver (and remove
        // this error path). The reasoning is that FW/HW that can get stuck
        // instead of indicating failure is best mitigated in the driver that's
        // specific to that FW/HW (or fixed with a new FW version).
        LogEvent(media_metrics::
                     StreamProcessorEvents2MigratedMetricDimensionEvent_EndOfStreamTimeoutError);
        FailLocked("Timeout waiting for output end of stream - waited %" PRId64 "s",
                   duration_so_far.to_secs());
        return;
      }
      // We call ProcessStreamControlForOutputQueue here because output EOS can
      // have a dependency on stream-driven output buffer re-config finishing,
      // possibly more than once. This call can in turn run
      // MidStreamOutputConstraintsChange, which will itself wait until output
      // buffers are re-configured (or failure) before returning from this call.
      //
      // Running MidStreamOutputConstraintsChange here should (in practice given
      // current codecs) be mutually exclusive with the CodecAdapter getting
      // stuck at FW/HW layer when input data is fuzzed.
      {  // scope unlock
        ScopedUnlock unlock(*this);
        ProcessStreamControlForOutputQueue();
      }
      lock.AssertHeld(lock_);
    }
  }  // ~lock
  LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_CoreFlushed);
}

void CodecImpl::CloseCurrentStream(uint64_t stream_lifetime_ordinal, bool release_input_buffers,
                                   bool release_output_buffers) {
  ZX_DEBUG_ASSERT(IsFidl());
  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }
    if (!EnsureFutureStreamCloseSeenLocked(stream_lifetime_ordinal)) {
      return;
    }
  }  // ~lock
  PostToStreamControl(
      [this, stream_lifetime_ordinal, release_input_buffers, release_output_buffers] {
        CloseCurrentStream_StreamControl(stream_lifetime_ordinal, release_input_buffers,
                                         release_output_buffers);
      });
}

void CodecImpl::CloseCurrentStream_StreamControl(uint64_t stream_lifetime_ordinal,
                                                 bool release_input_buffers,
                                                 bool release_output_buffers) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  ScopedLock lock(lock_);
  if (IsStoppingLocked()) {
    return;
  }
  EnsureStreamClosed(lock);
  if (release_input_buffers) {
    EnsureBuffersNotConfigured(lock, kInputPort, false);
  }
  if (release_output_buffers) {
    auto is_deconfig_done = std::make_shared<bool>(false);
    PostToSharedFidl([this, is_deconfig_done] {
      ScopedLock lock(lock_);
      if (IsStoppingLocked()) {
        return;
      }
      // When !is_dynamic_buffers_supported_, this prevents RecycleOutputPacket() on output domain
      // running concurrently with EnsureBuffersNotConfigured(kOutputPort) on StreamControl.
      //
      // Regardless of is_dynamic_buffers_supported_, this call is what moves
      // last_required_buffer_constraints_version_ordinal_ to a new value.
      StartIgnoringClientOldOutputConfig(lock);
      EnsureBuffersNotConfigured(lock, kOutputPort, false);
      *is_deconfig_done = true;
      wake_stream_control_condition_.notify_all();
    });
    // Wait for the posted task on the SharedFidl thread to complete the deconfiguration. This
    // thread (StreamControl) needs to wait for the FIDL thread to finish updating the buffer
    // states. We also check IsStoppingLocked() to ensure we don't get stuck if the CodecImpl is
    // being torn down.
    while (!IsStoppingLocked() && !*is_deconfig_done) {
      wake_stream_control_condition_.wait(lock.unique_lock());
    }
  }
}

void CodecImpl::Sync(SyncCallback callback) {
  ZX_DEBUG_ASSERT(IsFidl());
  if (IsStopping()) {
    return;
  }
  // By posting to StreamControl ordering domain, we sync both Output ordering
  // domain (on fidl_thread()) and the StreamControl ordering domain.
  //
  // If the posted task doesn't run because stream_control_queue_.StopAndClear()
  // happened/happens, it doesn't matter because the whole channel will be
  // closing before long.
  //
  // The callback has affinity with fidl_thread(), including the destructor.
  // This is problematic with respect to the
  // stream_control_queue_.StopAndClear() called on StreamControl domain during
  // unbind. Without special handling, that StopAndClear() would try to delete
  // callback on the StreamControl domain instead of on the fidl_thread().  To
  // prevent that, we ensure that deletion of the lambda without running the
  // lambda will still post destruction of callback to fidl_thread(), and this
  // posting will queue before the lamda that runs
  // shared_fidl_queue_.StopAndClear().
  PostToStreamControl([this, callback_holder = ThreadSafeDeleter<SyncCallback>(
                                 &shared_fidl_queue_, std::move(callback))]() mutable {
    Sync_StreamControl(std::move(callback_holder));
  });
}

void CodecImpl::Sync_StreamControl(ThreadSafeDeleter<SyncCallback> callback_holder) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  if (IsStopping()) {
    // In this case, we rely on ThreadSafeDeleter to delete callback on fidl_thread().
    //
    // The response won't be sent, which is appropriate - the channel is getting closed soon
    // instead, and the client has to tolerate that.
    //
    // ~callback_holder
    return;
  }
  // We post back to FIDL thread to respond to ensure we're not racing with
  // channel close which could lead to attempting to send to handle value 0
  // which can cause process termination.  Also, because this fences
  // BufferAllocation clean close which itself is done async from StreamControl
  // to FIDL in some cases.
  PostToSharedFidl([this, callback_holder = std::move(callback_holder)]() mutable {
    ZX_DEBUG_ASSERT(IsFidl());
    // call the held callback
    callback_holder.held()();
  });
}

void CodecImpl::RecycleOutputPacket(fuchsia::media::PacketHeader available_output_packet) {
  ZX_DEBUG_ASSERT(IsFidl());
  if (IsStopping()) {
    return;
  }
  if (kLogTimestampDelay) {
    LOG(INFO, "RecycleOutputPacket");
  }
  CodecPacket* packet = nullptr;
  {  // scope lock
    ScopedLock lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }
    if (!available_output_packet.has_buffer_lifetime_ordinal()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked("output packet is missing buffer lifetime ordinal");
      return;
    }
    uint64_t buffer_lifetime_ordinal = available_output_packet.buffer_lifetime_ordinal();
    if (!CheckPlausibleBufferLifetimeOrdinalLocked(kOutputPort, buffer_lifetime_ordinal)) {
      return;
    }
    if (!is_enable_old_output_buffers_ &&
        (buffer_lifetime_ordinal < buffer_lifetime_ordinal_[kOutputPort])) {
      // ignore arbitrarily-stale required by protocol when !is_enable_old_output_buffers_. In this
      // case the server will auto-recycle packets of old buffer_lifetime_ordinal when a new
      // buffer_lifetime_ordinal is created, and the server will auto-recycle any further output
      // packets from CodecAdapter with old buffer_lifetime_ordinal.
      //
      // Ignoring here is necessary to avoid the CodecAdapter seeing duplicate recycles, because
      // we've already recycled any non-current-buffer_lifetime_ordinal packets (or more precisely,
      // have at least committed to doing so outside lock_ (elsewhere), if we haven't already done
      // so).
      //
      // Thanks to even values from the client being prohibited, this also covers mid-stream output
      // config change where the server has already de-configured output buffers but the client
      // doesn't know about that yet. We include that case here by setting
      // buffer_lifetime_ordinal_[kOutputPort] to the next even value when de-configuring output
      // server-side until the client has re-configured output.
      return;
    }
    ZX_DEBUG_ASSERT(is_enable_old_output_buffers_ ||
                    (buffer_lifetime_ordinal == buffer_lifetime_ordinal_[kOutputPort]));
    if ((buffer_lifetime_ordinal == buffer_lifetime_ordinal_[kOutputPort]) &&
        !IsOutputConfiguredLocked()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "client sent RecycleOutputPacket() for buffer_lifetime_ordinal that "
          "wasn't configured yet - bad client behavior");
      return;
    }
    if (!available_output_packet.has_packet_index()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked("output packet is missing packet index");
      return;
    }

    uint32_t protocol_packet_index = available_output_packet.packet_index();
    uint32_t allocated_packet_index;
    if (*is_dynamic_buffers_[kOutputPort]) {
      auto& protocol_packets_by_ordinal = protocol_packets_by_protocol_packet_index_[kOutputPort];
      auto protocol_packets_by_ordinal_iter =
          protocol_packets_by_ordinal.find(buffer_lifetime_ordinal);
      if (protocol_packets_by_ordinal_iter == protocol_packets_by_ordinal.end()) {
        // The server is done tracking the old buffer_lifetime_ordinal, so we can ignore this
        // RecycleOutputPacket message. We won't be trying to send another output packet with the
        // old buffer_lifetime_ordinal, so ignoring this message doesn't risk getting stuck waiting
        // on a free packet.
        return;
      }
      auto& protocol_packets_by_index = protocol_packets_by_ordinal_iter->second;
      auto protocol_packets_by_index_iter = protocol_packets_by_index.find(protocol_packet_index);
      if (protocol_packets_by_index_iter == protocol_packets_by_index.end()) {
        // Ignoring the RecycleOutputPacket isn't an option here since we might still need to emit
        // another packet with this buffer_lifetime_ordinal, so ignoring an incorrectly-recycled
        // packet could lead to getting stuck waiting for a free packet. The client needs to only
        // recycle packet_index values that the server previously sent (and set the
        // buffer_lifetime_ordinal correctly, etc).
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
        FailLocked("RecycleOutputPacket unrecognized packet_index");
        return;
      }
      allocated_packet_index = protocol_packets_by_index_iter->second->allocated_packet_index();
      protocol_packets_by_index.erase(protocol_packet_index);
    } else {
      allocated_packet_index = protocol_packet_index;
    }

    auto& port_packets = active_packets_[kOutputPort];
    auto packets_at_ordinal_iter = port_packets.find(buffer_lifetime_ordinal);
    if (packets_at_ordinal_iter == port_packets.end()) {
      // The server has already closed out the buffer_lifetime_ordinal. This is
      // not an error. No action needed; ignore the message.
      return;
    }
    auto& packets_at_ordinal = packets_at_ordinal_iter->second;
    if (allocated_packet_index >= packets_at_ordinal.size()) {
      // We don't remove any packets from a buffer_lifetime_ordinal until the entire
      // buffer_lifetime_ordinal is closed out. So in this path we know that this packet has never
      // existed.
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked("out of range packet_index from client in RecycleOutputPacket()");
      return;
    }
    packet = packets_at_ordinal[allocated_packet_index].get();
    if (packet->is_free()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "packet_index already free at protocol level - invalid client "
          "message");
      return;
    }

    ZX_ASSERT(packet);
    VLOGF("RecycleOutputPacket ptr: %p index: %u", packet, packet->packet_index());
    ZX_ASSERT(packet->buffer());
    --packet->buffer()->output_in_flight_count_;

    // Mark free at protocol level.
    packet->SetFree(true);

    // Before handing the packet to the core codec, clear some fields that the
    // core codec is expected to set (or optionally set in the case of
    // timestamp_ish).  In addition to these parameters, a core codec can emit
    // output config changes via onCoreCodecMidStreamOutputConstraintsChange().
    packet->ClearStartOffset();
    packet->ClearValidLengthBytes();
    packet->ClearTimestampIsh();
  }  // ~lock

  VLOGF("calling CoreCodecRecycleOutputPacket packet ptr: %p index: %u buffer: %p index: %u",
        packet, packet->packet_index(), packet->buffer(), packet->buffer()->index());

  // Recycle to core codec. This is the output ordering domain (fidl thread) so
  // CoreCodecCloseBufferLifetimeOrdinal can't happen between releasing the lock above and this
  // call.
  CoreCodecRecycleOutputPacket(packet);
}

void CodecImpl::QueueInputFormatDetails(uint64_t stream_lifetime_ordinal,
                                        fuchsia::media::FormatDetails format_details) {
  ZX_DEBUG_ASSERT(IsFidl());
  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }
    if (!EnsureFutureStreamSeenLocked(stream_lifetime_ordinal)) {
      return;
    }
  }  // ~lock

  if (!format_details.has_format_details_version_ordinal()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail(
        "client QueueInputFormatDetails(): Format details have no version "
        "ordinal.");
    return;
  }

  PostToStreamControl(
      [this, stream_lifetime_ordinal, format_details = std::move(format_details)]() mutable {
        QueueInputFormatDetails_StreamControl(stream_lifetime_ordinal, std::move(format_details));
      });
}

// TODO(b/527318241): Need test coverage for this method, to cover at least the same format
// including OOB bytes as were specified during codec creation, and codec creation with no OOB bytes
// then this method setting OOB bytes (not the ideal client usage pattern in the long run since the
// CreateDecoder() might decline to provide a optimized but partial Codec implementation, but should
// be allowed nonetheless).
void CodecImpl::QueueInputFormatDetails_StreamControl(
    uint64_t stream_lifetime_ordinal, fuchsia::media::FormatDetails format_details) {
  ZX_DEBUG_ASSERT(IsStreamControl());

  ScopedLock lock(lock_);
  if (IsStoppingLocked()) {
    return;
  }
  if (!CheckStreamLifetimeOrdinalLocked(stream_lifetime_ordinal)) {
    return;
  }

  if (!is_supports_dynamic_buffers()) {
    if (!CheckWaitEnsureInputConfigured(lock)) {
      stream_->AssertHeld(this);
      ZX_DEBUG_ASSERT(IsStoppingLocked() || !stream_ || stream_->future_discarded());
      return;
    }
  }

  ZX_DEBUG_ASSERT(stream_lifetime_ordinal >= stream_lifetime_ordinal_);
  if (stream_lifetime_ordinal > stream_lifetime_ordinal_) {
    if (!StartNewStream(lock, stream_lifetime_ordinal, /*is_for_packet=*/false)) {
      return;
    }
  }
  ZX_DEBUG_ASSERT(stream_lifetime_ordinal == stream_lifetime_ordinal_);
  if (stream_->failure_seen()) {
    // Already reported to client.
    return;
  }
  if (stream_->input_end_of_stream()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("QueueInputFormatDetails() after QueueInputEndOfStream() unexpected");
    return;
  }
  stream_->AssertHeld(this);
  if (stream_->future_discarded()) {
    // No reason to handle since the stream is future-discarded.
    return;
  }
  stream_->SetInputFormatDetails(
      std::make_unique<fuchsia::media::FormatDetails>(std::move(format_details)));
  // SetOobConfigPending(true) to ensure oob_config_pending() is true.
  //
  // This call is needed only to properly handle a call to
  // QueueInputFormatDetails() mid-stream.  For new streams that lack any calls
  // to QueueInputFormatDetails() before an input packet arrives, the
  // oob_config_pending() will already be true because it starts true for a new
  // stream.  For QueueInputFormatDetails() at the start of a stream before any
  // packets, oob_config_pending() will already be true.
  //
  // For decoders this is basically a pending oob_bytes.  For encoders
  // this pending config change can potentially include uncompressed format
  // details, if mid-stream format change is supported by the encoder.
  stream_->SetOobConfigPending(true);
}

void CodecImpl::QueueInputPacket(fuchsia::media::Packet packet) {
  ZX_DEBUG_ASSERT(IsFidl());
  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }
    if (!packet.has_stream_lifetime_ordinal()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "QueueInputPacket with packet that has no stream lifetime "
          "ordinal");
      return;
    }
    if (!EnsureFutureStreamSeenLocked(packet.stream_lifetime_ordinal())) {
      return;
    }
  }  // ~lock
  if (kLogTimestampDelay) {
    LOG(INFO, "input timestamp: has: %d value: 0x%" PRIx64, packet.has_timestamp_ish(),
        packet.has_timestamp_ish() ? packet.timestamp_ish() : 0);
  }
  PostToStreamControl([this, packet = std::move(packet)]() mutable {
    QueueInputPacket_StreamControl(std::move(packet));
  });
}

void CodecImpl::QueueInputPacket_StreamControl(fuchsia::media::Packet packet) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  ZX_DEBUG_ASSERT(packet.has_stream_lifetime_ordinal());

  // Until after creation of send_free_input_packet_locked further down, we can only return if
  // IsStoppingLocked() is true.

  if (!packet.has_header()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("QueueInputPacket with packet has no header");
    return;
  }
  fuchsia::media::PacketHeader temp_header_copy = fidl::Clone(packet.header());

  if (!packet.header().has_buffer_lifetime_ordinal()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail(
        "QueueInputPacket with header that has no buffer lifetime "
        "ordinal");
    return;
  }
  uint64_t buffer_lifetime_ordinal = packet.header().buffer_lifetime_ordinal();

  if (!packet.has_stream_lifetime_ordinal()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("QueueInputPacket without packet stream_lifetime_ordinal.");
    return;
  }

  if (!packet.header().has_packet_index()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("QueueInputPacket with packet has no packet index");
    return;
  }

  if (!packet.has_buffer_index()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("QueueInputPacket with packet has no buffer index");
    return;
  }

  if (!packet.has_start_offset()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("client QueueInputPacket() with packet has no start offset");
    return;
  }

  if (!packet.has_valid_length_bytes()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("client QueueInputPacket() with packet has no valid length bytes");
    return;
  }

  if (packet.valid_length_bytes() == 0) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("client QueueInputPacket() with valid_length_bytes 0 - not allowed");
    return;
  }
  if (packet.start_offset() + packet.valid_length_bytes() < packet.start_offset()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("client QueueInputPacket() start_offset + valid_length_bytes overflow");
    return;
  }

  CodecBuffer* buffer = nullptr;
  CodecPacket* codec_packet = nullptr;
  {  // scope lock
    ScopedLock lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }

    if (!CheckPlausibleBufferLifetimeOrdinalLocked(kInputPort, buffer_lifetime_ordinal)) {
      ZX_DEBUG_ASSERT(IsStoppingLocked());
      return;
    }

    if (!CheckStreamLifetimeOrdinalLocked(packet.stream_lifetime_ordinal())) {
      ZX_DEBUG_ASSERT(IsStoppingLocked());
      return;
    }

    // Unless we cancel this cleanup, we'll free the input packet back to the
    // client.
    CodecPacket* dynamic_packet = nullptr;
    auto send_free_input_packet_locked =
        fit::defer([this, &lock, header = std::move(temp_header_copy), &dynamic_packet]() mutable {
          lock.AssertHeld(lock_);

          // Mute sending this if FailLocked() was called previously, in case
          // the reason we're here is something horribly wrong with the packet
          // header. This way we avoid repeating gibberish back to the client.
          // While that gibberish might be a slight clue for debugging in some
          // cases, it's not valid protocol, so don't send it.  If
          // IsStoppingLocked(), the Codec channel will close soon, making this
          // response unnecessary.
          if (IsStoppingLocked()) {
            return;
          }

          if (dynamic_packet) {
            dynamic_packet->ClearProtocolPacketIndex();
            free_input_packets_.emplace_back(dynamic_packet);
            // SendFreeInputPacketLocked below takes care of
            // protocol_packets_by_protocol_packet_index_.
          }

          // Depending on how many packet fields have been checked by the time
          // this runs, we may not have fully validated every field yet. This
          // will make at least as much sense as the packet sent by the client.
          //
          // Moving more checks above creation of send_free_input_packet_locked
          // is likely possible, but not necessarily worthwhile.
          SendFreeInputPacketLocked(std::move(header));
        });

    if (!CheckWaitEnsureInputConfigured(lock)) {
      if (IsStoppingLocked()) {
        return;
      }
      stream_->AssertHeld(this);
      ZX_DEBUG_ASSERT(stream_ && stream_->future_discarded());
      return;
    }
    ZX_DEBUG_ASSERT(is_dynamic_buffers_[kInputPort].has_value());

    // For input, mid-stream config changes are not a thing and input buffers
    // are never unilaterally de-configured by the Codec server.
    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[kInputPort] ==
                    port_settings_[kInputPort]->buffer_lifetime_ordinal());

    // For this message we're strict re. buffer_lifetime_ordinal.
    if (buffer_lifetime_ordinal != port_settings_[kInputPort]->buffer_lifetime_ordinal()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      // At least for now, we don't support old buffer_lifetime_ordinal buffers on input. If a video
      // encoder needs to be fed new image dimensions that don't fit in the old buffers or aren't
      // consistent with the old buffers' constraints, new buffers can be allocated for the new
      // dimensions using a new buffer_lifetime_ordinal. If dimensions switch up and down fairly
      // frequently, the preferred approach is to allocate buffers large enough to hold all needed
      // image dimensions, and feed the encoder with images of various image dimensions using the
      // large-enough buffers. However, if a client is constrained to use fixed image dimensions per
      // encoder input buffer, the client can instead keep their own sets of buffers, each set
      // associated with a single fixed image dimensions, and use AddBuffer to re-add old buffers as
      // needed (taking care not to re-fill an input buffer too soon) using a new
      // buffer_lifetime_ordinal each time image dimensions change (with some degree of buffer setup
      // performance penalty per dimension switch, vs using a single set of buffers large enough to
      // hold any of the needed image dimensions).
      FailLocked("QueueInputPacket with invalid buffer_lifetime_ordinal.");
      return;
    }

    // other cases rejected above
    ZX_DEBUG_ASSERT(packet.stream_lifetime_ordinal() >= stream_lifetime_ordinal_);

    if (packet.stream_lifetime_ordinal() > stream_lifetime_ordinal_) {
      // This case implicitly starts a new stream.  If the client wanted to
      // ensure that the old stream would be fully processed, the client would
      // have sent FlushEndOfStreamAndCloseStream() previously, whose
      // processing (previous to reaching here) takes care of the flush.
      //
      // Start a new stream, synchronously.
      if (!StartNewStream(lock, packet.stream_lifetime_ordinal(), /*is_for_packet=*/true)) {
        return;
      }
    }
    ZX_DEBUG_ASSERT(packet.stream_lifetime_ordinal() == stream_lifetime_ordinal_);

    uint32_t protocol_packet_index = packet.header().packet_index();
    auto& input_packets = all_packets(kInputPort);

    // CheckWaitEnsureInputConfigured() returned true above, so is_dynamic_buffers_[kInputPort] is
    // known to be filled out.
    ZX_DEBUG_ASSERT(is_dynamic_buffers_[kInputPort].has_value());
    if (*is_dynamic_buffers_[kInputPort]) {
      if (free_input_packets_.empty()) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
        // See stream_processor.fidl docs on PacketHeader.packet_index.
        FailLocked("QueueInputPacket when already max packets");
        return;
      }
      auto& protocol_packets_by_protocol_packet_index =
          protocol_packets_by_protocol_packet_index_[kInputPort][buffer_lifetime_ordinal];
      auto protocol_packets_by_protocol_packet_index_iter =
          protocol_packets_by_protocol_packet_index.find(protocol_packet_index);
      if (protocol_packets_by_protocol_packet_index_iter !=
          protocol_packets_by_protocol_packet_index.end()) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
        FailLocked("QueueInputPacket duplicate packet_index already in flight");
        return;
      }
      codec_packet = free_input_packets_.back();
      dynamic_packet = codec_packet;
      free_input_packets_.pop_back();
      codec_packet->SetProtocolPacketIndex(protocol_packet_index);
      protocol_packets_by_protocol_packet_index.insert(
          std::make_pair(protocol_packet_index, codec_packet));
    } else {
      ZX_DEBUG_ASSERT(!*is_dynamic_buffers_[kInputPort]);
      if (protocol_packet_index >= input_packets.size()) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
        FailLocked(
            "QueueInputPacket with packet_index out of range - "
            "packet_index: %u size: %u",
            packet.header().packet_index(), input_packets.size());
        return;
      }
      codec_packet = input_packets[protocol_packet_index].get();
    }

    uint32_t buffer_index = packet.buffer_index();

    auto* buffers = all_buffers(kInputPort);
    std::optional<BuffersByIndex::iterator> buffers_iter;
    if (buffers) {
      buffers_iter = buffers->find(buffer_index);
    }
    if (*is_dynamic_buffers_[kInputPort]) {
      if (!buffers || *buffers_iter == buffers->end()) {
        // see if buffer is currently adding
        auto& adding_buffers_by_ordinal = adding_buffers_[kInputPort];
        auto adding_buffers_by_ordinal_iter =
            adding_buffers_by_ordinal.find(buffer_lifetime_ordinal);
        if (adding_buffers_by_ordinal_iter == adding_buffers_by_ordinal.end()) {
          LogEvent(media_metrics::
                       StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
          FailLocked("QueueInputPacket before AddBuffer");
          return;
        }
        auto& adding_buffers_by_index = adding_buffers_by_ordinal_iter->second;
        auto adding_buffers_by_index_iter = adding_buffers_by_index.find(buffer_index);
        if (adding_buffers_by_index_iter == adding_buffers_by_index.end()) {
          LogEvent(media_metrics::
                       StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
          // no active buffer, no adding buffer
          FailLocked("QueueInputPacket buffer_index not found");
          return;
        }
        auto& adding_buffer = *adding_buffers_by_index_iter->second;
        // check if adding buffer is already removing
        if (adding_buffer.continue_remove_) {
          LogEvent(media_metrics::
                       StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
          // The client sent RemoveBuffer for this buffer_index before QueueInputPacket specifying
          // this buffer_index, which is not permitted. This check intentionally won't catch cases
          // where the buffer_index was fully done removing before a reuse of the same buffer_index
          // value, as that is permitted.
          FailLocked(
              "QueueInputPacket buffer_index RemoveBuffer already in progress (server case 1)");
          return;
        }
        auto is_buffer_ready = [this, &lock, buffer_index] {
          lock.AssertHeld(lock_);
          auto* buffers = all_buffers(kInputPort);
          std::optional<BuffersByIndex::iterator> buffers_iter;
          if (buffers) {
            buffers_iter = buffers->find(buffer_index);
          }
          return buffers && (*buffers_iter != buffers->end());
        };
        lock.AssertHeld(stream_->parent_->lock_);
        while (!IsStoppingLocked() && !stream_->future_discarded() && !is_buffer_ready()) {
          // This is not processing anything other than input GetVmoInfo response, so any input
          // RemoveBuffer later than current QueueInputPacket (on stream control thread) can't get
          // processed until after the current method returns (good).
          RunAnySysmemCompletionsOrWait(lock);
        }
        if (IsStoppingLocked()) {
          return;
        }
        if (stream_->future_discarded()) {
          // A discarded stream isn't an error for the CodecImpl instance.
          return;
        }
        buffers = all_buffers(kInputPort);
        buffers_iter = buffers->find(buffer_index);
      }
      ZX_ASSERT(buffers && buffers_iter.has_value() && *buffers_iter != buffers->end());
    } else {
      if (!buffers || *buffers_iter == buffers->end()) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
        FailLocked("QueueInputPacket with unknown buffer_index (out of range and/or never added)");
        return;
      }
    }
    ZX_ASSERT(buffers && buffers_iter.has_value() && (*buffers_iter != buffers->end()));
    buffer = (*buffers_iter)->second.get();

    if (buffer->pending_remove_completion_) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
      // The client isn't allowed to queue an input packet specifying a buffer for which the client
      // has already sent RemoveBuffer.
      FailLocked("QueueInputPacket buffer_index RemoveBuffer already in progress (server case 2)");
      return;
    }
    // Input buffer re-config is always client-driven, not CodecAdapter-driven, so if the client
    // isn't removing the input buffer then the buffer isn't being removed. We can assert here
    // because this is input, and because client-driven re-configs other than RemoveBuffer will
    // change buffer_lifetime_ordinal[port] which was already checked above.
    ZX_DEBUG_ASSERT(!buffer->is_remove_pending());

    // Protocol check re. free/busy coherency.  This applies to packets only,
    // not buffers.
    if (!codec_packet->is_free()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked("client QueueInputPacket() with packet_index !free");
      return;
    }

    if (stream_->input_end_of_stream()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked("QueueInputPacket() after QueueInputEndOfStream() unexpeted");
      return;
    }

    if (stream_->failure_seen()) {
      // Already reported to client.
      return;
    }

    stream_->AssertHeld(this);
    if (stream_->future_discarded()) {
      // Don't queue to core codec.  The stream_ may have never fully started,
      // or may have been future-discarded since.  Either way, skip queueing to
      // the core codec.
      //
      // If the stream didn't fully start - as in, the client moved on to
      // another stream before fully configuring output, then the core codec is
      // not presently in a state compatible with queueing input, but the Codec
      // interface is.  So in that case, we must avoid queueing to the core
      // codec for correctness.
      //
      // If the stream was just future-discarded after fully starting, then this
      // is just an optimization to avoid giving the core codec more work to do
      // for a stream the client has already discarded.
      //
      // ~send_free_input_packet_locked
      // ~lock
      return;
    }

    codec_packet->SetFree(false);

    ZX_DEBUG_ASSERT(buffer != nullptr);
    codec_packet->SetBuffer(buffer);
    codec_packet->SetStartOffset(packet.start_offset());
    codec_packet->SetValidLengthBytes(packet.valid_length_bytes());
    if (packet.has_timestamp_ish()) {
      codec_packet->SetTimstampIsh(packet.timestamp_ish());
    } else {
      codec_packet->ClearTimestampIsh();
    }

    // Sending OnFreeInputPacket() will happen later instead, when the core
    // codec gives back the packet. The remaining failure paths below that call
    // Fail don't need to send OnFreeInputPacket.
    send_free_input_packet_locked.cancel();
  }  // ~lock

  if (stream_->oob_config_pending()) {
    HandlePendingInputFormatDetails();
    stream_->SetOobConfigPending(false);
  }

  if (codec_packet->start_offset() + codec_packet->valid_length_bytes() >
      codec_packet->buffer()->size()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("client QueueInputPacket() with packet end > buffer size");
    return;
  }

  ZX_ASSERT(port_settings_[kInputPort]);

  // Flush the data out to RAM if needed.
  if (IsCoreCodecHwBased(kInputPort) &&
      port_settings_[kInputPort]->coherency_domain() == fuchsia_sysmem2::CoherencyDomain::kCpu) {
    // This flushes only the portion of the buffer that the packet is
    // referencing.
    codec_packet->CacheFlush();
  }

  // We don't need to be under lock for this, because the fact that we're on the StreamControl
  // domain is enough to guarantee that any other input-related or stream-related control of the
  // core codec will occur after this.
  CoreCodecQueueInputPacket(codec_packet);
}

void CodecImpl::QueueInputEndOfStream(uint64_t stream_lifetime_ordinal) {
  ZX_DEBUG_ASSERT(IsFidl());
  LogEvent(
      media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_StreamEndOfStreamInput);
  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }
    if (!EnsureFutureStreamSeenLocked(stream_lifetime_ordinal)) {
      return;
    }
  }  // ~lock
  PostToStreamControl([this, stream_lifetime_ordinal] {
    QueueInputEndOfStream_StreamControl(stream_lifetime_ordinal);
  });
}

// A correctly-operating client will only send this message if
// DetailedCodecDescription.supports_dynamic_buffers was set to true.
void CodecImpl::ParticipateInBufferAllocation(
    fuchsia::media::StreamProcessorParticipateInBufferAllocationRequest request) {
  ZX_DEBUG_ASSERT(IsFidl());
  if (!is_supports_dynamic_buffers()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("client sent ParticipateInBufferAllocation when !supports_dynamic_buffers");
    return;
  }
  ZX_ASSERT(::codec_impl::internal::kEnableDynamicBuffers);
  is_force_output_buffers_fixed_image_size_message_permitted_ = false;
  ZX_DEBUG_ASSERT(!!codec_adapter_);
  if (!request.has_port()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("ParticipateInBufferAllocation port field must be set");
    return;
  }
  CodecPort port;
  switch (request.port()) {
    case fuchsia::media::Port::INPUT:
      port = kInputPort;
      break;
    case fuchsia::media::Port::OUTPUT:
      port = kOutputPort;
      break;
    default:
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      Fail("ParticipateInBufferAllocation unrecognized port value - port: %u", request.port());
      return;
  }
  if (!request.has_buffer_constraints_version_ordinal()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("ParticipateInBufferAllocation buffer_constraints_version_ordinal field must be set");
    return;
  }
  if (port == kInputPort &&
      request.buffer_constraints_version_ordinal() != kInputBufferConstraintsVersionOrdinal) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail(
        "ParticipateInBufferAllocation buffer_constraints_version_ordinal must be 1 for input port");
    return;
  }
  if (!request.has_sysmem2_token() || !request.sysmem2_token().is_valid()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("ParticipateInBufferAllocation sysmem2_token field must be set");
    return;
  }
  uint64_t buffer_constraints_version_ordinal = request.buffer_constraints_version_ordinal();
  auto token = fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(
      request.mutable_sysmem2_token()->TakeChannel());
  std::optional<uint64_t> maybe_buffer_lifetime_ordinal;
  if (request.has_buffer_lifetime_ordinal()) {
    maybe_buffer_lifetime_ordinal = request.buffer_lifetime_ordinal();
  }
  bool allow_single_buffer = request.has_allow_single_buffer() && request.allow_single_buffer();
  switch (request.port()) {
    case fuchsia::media::Port::INPUT:
      // Everything input-related synchronizes by running on (and/or queueing to) the StreamControl
      // thread. This is also true for participating in buffer allocation, since the sysmem
      // constraints are allowed to depend on whether there's already a stream with other buffers
      // allocated using a particular format (due to potential FW and/or HW limitations).
      PostToStreamControl([this, port, buffer_constraints_version_ordinal,
                           maybe_buffer_lifetime_ordinal, token = std::move(token),
                           allow_single_buffer]() mutable {
        ParticipateInBufferAllocationInternal(port, buffer_constraints_version_ordinal,
                                              std::move(token), maybe_buffer_lifetime_ordinal,
                                              allow_single_buffer);
      });
      return;
    case fuchsia::media::Port::OUTPUT:
      ParticipateInBufferAllocationInternal(port, buffer_constraints_version_ordinal,
                                            std::move(token), maybe_buffer_lifetime_ordinal,
                                            allow_single_buffer);
      return;
    default:
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      Fail("ParticipateInBufferAllocation unrecognised port value");
      return;
  }
}

std::optional<fuchsia_sysmem2::BufferCollectionConstraints>
CodecImpl::GetBufferConstraintsForDynamic(ScopedLock& lock, CodecPort port,
                                          uint64_t buffer_constraints_version_ordinal,
                                          bool allow_single_buffer,
                                          uint64_t* out_codec_adapter_constraints_version) {
  ZX_DEBUG_ASSERT(out_codec_adapter_constraints_version);
  // We're releasing the lock during the call to CoreCodecGetBufferCollectionConstraints3/2; we
  // don't want the ordinals to change during that interval. They won't change because the thread
  // we're running on is the only thread that changes them (per port).
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  lock.AssertHeld(lock_);

  // caller checks these
  ZX_DEBUG_ASSERT(buffer_constraints_version_ordinal >=
                  last_required_buffer_constraints_version_ordinal_[port]);
  ZX_DEBUG_ASSERT(buffer_constraints_version_ordinal <=
                  sent_buffer_constraints_version_ordinal_[port]);

  ZX_DEBUG_ASSERT(
      !snapped_buffer_constraints_version_ordinal_[port].has_value() ||
      snapped_buffer_constraints_version_ordinal_[port]->buffer_constraints_version_ordinal() <=
          buffer_constraints_version_ordinal);
  if (snapped_buffer_constraints_version_ordinal_[port].has_value() &&
      snapped_buffer_constraints_version_ordinal_[port]->buffer_constraints_version_ordinal() ==
          buffer_constraints_version_ordinal) {
    *out_codec_adapter_constraints_version =
        snapped_buffer_constraints_version_ordinal_[port]->constraints_version();
    // intentional copy/clone
    return snapped_buffer_constraints_version_ordinal_[port]->constraints();
  }

  std::optional<CoreCodecGetBufferCollectionConstraints3Result> maybe_result;
  {  // scope unlock
    // Relevant settings won't change while we have the lock released to call the core codec
    // here, because we're on the only thread that makes those changes (different thread per
    // input or output).
    ScopedUnlock unlock(*this);
    maybe_result = CoreCodecGetBufferCollectionConstraints3(port);
  }  // ~unlock
  // Get TA analysis back in sync with reality after ~unlock just above. See also comments on
  // ~ScopedUnlock.
  lock.AssertHeld(lock_);

  // CodecAdapter(s) with IsSupportsDynamicBuffers() true are required to override
  // CoreCodecGetBufferCollectionConstraints3 and fill out the constraints field.
  ZX_DEBUG_ASSERT(maybe_result.has_value());
  auto& result = *maybe_result;
  *out_codec_adapter_constraints_version = result.constraints_version;
  auto& constraints = maybe_result->constraints;

  // The core codec doesn't fill out usage directly.  Instead we fill it out here.
  if (!FixupBufferCollectionConstraintsLocked(port, &constraints)) {
    // FixupBufferCollectionConstraints() already called Fail().
    ZX_DEBUG_ASSERT(IsStoppingLocked());
    return std::nullopt;
  }

  // When allow_single_buffer, the constraints for dynamic buffers don't constrain how many buffers
  // get allocated per ParticipateInBufferAllocationInternal message. Instead, the client is
  // responsible for paying attention to buffer_count_for_server_current or
  // dynamic_buffers_video_decoder_output_safe and ensuring that sufficient buffers are allocated
  // fairly soon (or at least, before the client would be annoyed that the stream processing has
  // stalled out due to insufficient buffers added so far).
  //
  // If the client doesn't like dealing with that, the client can use SetInputBufferPartialSettings
  // / SetOutputBufferPartialSettings instead (along with passing token.Duplicate() to other parts
  // of the pipeline so the buffer counts can be aggregated by sysmem to make sure the pipeline will
  // be happy overall re. buffer count). Or the client can leave `allow_single_buffer` un-set or set
  // to false, and add any desired slack using the client's own related sysmem token.
  constraints.min_buffer_count() = 1;
  // In dynamic buffer mode, we never include slack - the client can include slack as desired via
  // the client's own related sysmem token.
  constraints.min_buffer_count_for_dedicated_slack().reset();
  constraints.min_buffer_count_for_shared_slack().reset();
  if (allow_single_buffer) {
    // In this case it's entirely up to the client to ensure that the server will have at least
    // buffer_count_for_server_current buffers for forward progress to be guaranteed.
    constraints.min_buffer_count_for_camping().reset();
  }

  // intentional copy/clone of constraints
  snapped_buffer_constraints_version_ordinal_[port].emplace(SnappedBufferConstraintsVersionOrdinal(
      buffer_constraints_version_ordinal, result.constraints_version, constraints));

  return std::move(constraints);
}

std::optional<zx::vmo> CodecImpl::TryGetMatchExistingVmo(CodecPort port,
                                                         uint64_t buffer_lifetime_ordinal) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  if (buffer_lifetime_ordinal != buffer_lifetime_ordinal_[port]) {
    // The caller already handled "<".
    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal > buffer_lifetime_ordinal_[port]);
    return std::nullopt;
  }
  const zx::vmo* vmo = nullptr;
  // Try to find a VMO handle to an existing buffer under the same port and buffer_lifetime_ordinal.
  // We look in adding_buffers_ and active_buffers_. The order doesn't matter.
  auto& adding_by_ordinal = adding_buffers_[port];
  auto adding_by_ordinal_iter = adding_by_ordinal.find(buffer_lifetime_ordinal);
  if (adding_by_ordinal_iter != adding_by_ordinal.end() &&
      !adding_by_ordinal_iter->second.empty()) {
    // we don't care which specific buffer so can just use begin()
    auto& adding_buffer = adding_by_ordinal_iter->second.begin()->second;
    vmo = &adding_buffer->unverified_vmo_;
  } else {
    auto& active_by_ordinal = active_buffers_[port];
    auto active_by_ordinal_iter = active_by_ordinal.find(buffer_lifetime_ordinal);
    if (active_by_ordinal_iter != active_by_ordinal.end() &&
        !active_by_ordinal_iter->second.empty()) {
      // we don't care which specific buffer so can just use begin()
      auto& active_buffer = *active_by_ordinal_iter->second.begin()->second;
      vmo = &active_buffer.original_vmo();
    }
  }
  if (!vmo) {
    // This can happen if the client has depleted the number of buffers down to zero by using
    // StreamProcessor.RemoveBuffer on every buffer and all of those have completed because the
    // CodecAdapter has also closed all it's handles to all the buffers. This situation is
    // permitted, but not recommended client behavior when setting
    // ParticipateInBufferAllocation.buffer_lifetime_ordinal. Still, it's not an error. So we VLOGF
    // not LOGF to avoid the spam by default.
    VLOGF("TryGetMatchExistingVmo found no existing buffer to match (not technically an error)");
    return std::nullopt;
  }
  // duplicate the vmo handle so we can set must_match_vmo field in the constraints
  zx::vmo dup_vmo;
  zx_status_t dup_status = vmo->duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_vmo);
  // We've already duplicated the vmo once for GetVmoInfo, so if it fails to duplicate here that's
  // unexpected/OOM.
  ZX_ASSERT(dup_status == ZX_OK);
  return dup_vmo;
}

void CodecImpl::ParticipateInBufferAllocationInternal(
    CodecPort port, uint64_t buffer_constraints_version_ordinal,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
    std::optional<uint64_t> maybe_buffer_lifetime_ordinal, bool allow_single_buffer) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  LOG(DEBUG, "CodecImpl::ParticipateInBufferAllocationInternal port: %u", port);
  ScopedLock lock(lock_);
  if (IsStoppingLocked()) {
    // This StreamProcessor is going away. We can just drop the token and return.
    return;
  }
  if (buffer_constraints_version_ordinal > sent_buffer_constraints_version_ordinal_[port]) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked(
        "ParticipateInBufferAllocation too-new buffer_constraints_version_ordinal - port: %d",
        port);
    return;
  }
  if (maybe_buffer_lifetime_ordinal.has_value() &&
      (*maybe_buffer_lifetime_ordinal < buffer_lifetime_ordinal_[port])) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("ParticipateInBufferAllocation with buffer_lifetime_ordinal set to old value");
    return;
  }
  if (buffer_constraints_version_ordinal <
      last_required_buffer_constraints_version_ordinal_[port]) {
    // We set generic constraints here to avoid making error handling more complicated for the
    // client, but the potential later AddBuffer will ignore the buffer since its
    // buffer_constraints_version_ordinal and/or buffer_lifetime_ordinal is already stale. Failing
    // the LogicalBufferCollection here would create more hassle for the client / would potentially
    // cause the client (c2 code running in this process calling gralloc IAllocator.allocate2) to
    // give up when it shouldn't.
    //
    // This won't happen except when we're near a mid-stream dimension change where we're either not
    // using bufferqueue or haven't yet started only reusing buffers, and can only happen for the
    // output port because last_required_buffer_constraints_version_ordinal_[kInputPort] stays zero.
    ZX_DEBUG_ASSERT(port == kOutputPort);
    LOG(DEBUG,
        "CodecImpl::ParticipateInBufferAllocationInternal setting generic constraints port: %u",
        port);
    PostToSharedFidl([this, token = std::move(token)]() mutable {
      ZX_DEBUG_ASSERT(IsFidl());
      auto buffer_collection_endpoints =
          fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
      fuchsia_sysmem2::AllocatorBindSharedCollectionRequest bind_request;
      bind_request.token() = std::move(token);
      bind_request.buffer_collection_request() = std::move(buffer_collection_endpoints.server);
      auto bind_result = sysmem_->BindSharedCollection(std::move(bind_request));
      // one-way message; sysmem Allocator disconnection is fatal
      ZX_ASSERT(bind_result.is_ok());
      auto buffer_collection = fidl::SyncClient(std::move(buffer_collection_endpoints.client));
      fuchsia_sysmem2::BufferCollectionSetConstraintsRequest request;
      auto& constraints = request.constraints().emplace();
      // We have to set usage else sysmem will reject; however the buffer will not get used so we
      // set explicit "none" usage to inform sysmem that we're doing this on purpose.
      constraints.usage().emplace().none() = fuchsia_sysmem2::kNoneUsage;
      // We still set min_buffer_count because stream_processor.fidl docs says the server will.
      constraints.min_buffer_count() = 1;
      // Failures to send these one-way messages can happen if some other participant has caused
      // BufferCollection server_end to close by this point, but CodecImpl doesn't need to care.
      std::ignore = buffer_collection->SetConstraints(std::move(request));
      std::ignore = buffer_collection->Release();
    });
    return;
  }

  // We've peeled off too new and too old above.
  ZX_DEBUG_ASSERT(buffer_constraints_version_ordinal >=
                      last_required_buffer_constraints_version_ordinal_[port] &&
                  buffer_constraints_version_ordinal <=
                      sent_buffer_constraints_version_ordinal_[port]);

  // In some cases this ensures or helps ensure that SingleBufferSettings will match for all buffers
  // under the same StreamProcessor, port, and buffer_lifetime_ordinal, but not in all cases and
  // usages. However, AddBuffer strictly enforces this, so the CodecAdapter can still rely on this
  // despite the gaps here in some cases/usages.
  uint64_t codec_adapter_constraints_version;
  auto maybe_constraints =
      GetBufferConstraintsForDynamic(lock, port, buffer_constraints_version_ordinal,
                                     allow_single_buffer, &codec_adapter_constraints_version);
  if (!maybe_constraints.has_value()) {
    // GetBufferConstraintsForDynamic() already called Fail().
    ZX_DEBUG_ASSERT(IsStoppingLocked());
    return;
  }
  fuchsia_sysmem2::BufferCollectionConstraints constraints = std::move(*maybe_constraints);

  // If we've seen at least one AddBuffer whose buffer is not yet removed under the same port and
  // buffer_lifetime_ordinal, we can tell sysmem to match that buffer's SingleBufferSettings, which
  // means there's no need to ask the CodecAdapter for constraints, and sysmem will ensure that the
  // SingleBufferSettings will match exactly.
  //
  // Separately CodecImpl also enforces that SingleBufferSettings is identical for all buffers under
  // a given port and buffer_lifetime_ordinal. The mechanism described in the previous paragraph, if
  // used correctly, can be used by a client to ensure that enforcement won't reject the new buffers
  // due to non-identical SingleBufferSettings.
  //
  // This mechanism doesn't need the result from AddBuffer's GetVmoInfo to be available yet. We can
  // duplicate a VMO handle of a buffer that has a GetVmoInfo still in-flight (which is using a
  // separate handle dup).
  std::optional<zx::vmo> maybe_match_existing_vmo;
  if (maybe_buffer_lifetime_ordinal.has_value()) {
    // This can only find an existing VMO to match if there's been a previous AddBuffer of a buffer
    // that hasn't been removed yet, for the same port and buffer_lifetime_ordinal. See doc comments
    // in stream_processor.fidl re. ParticipateInBufferAllocation.
    maybe_match_existing_vmo = TryGetMatchExistingVmo(port, *maybe_buffer_lifetime_ordinal);
  }

  // Unlock before posting to avoid potential thread ping-pong (for input; output is just re-posting
  // so can't ping-pong); this thread doesn't need the lock to post safely, thanks to ClosureQueue.
  lock.unlock();

  // For output, the only reason we re-post here is to avoid handling lock acquisition differently
  // for input vs. output.
  PostToSharedFidl([this, port, token = std::move(token), constraints = std::move(constraints),
                    maybe_match_existing_vmo = TakeOptional(maybe_match_existing_vmo)]() mutable {
    std::lock_guard<std::mutex> lock(lock_);
    if (!sysmem_.is_valid()) {
      return;
    }
    if (IsStoppingLocked()) {
      return;
    }
    // Re. communication with sysmem here, we just need to send SetConstraints then Release. We
    // don't need to send or wait for response from WaitForAllBuffersAllocated. The client will
    // (probably) do that (directly or indirectly) then send the buffer(s) to AddBuffer (one message
    // per buffer).
    //
    // Later when AddBuffer is sent by the client, we'll use
    // CoreCodecGetBufferCollectionConstraints2 again to get the then-current constraints, and
    // sysmem CheckVmoConstraints to ensure the added buffer is (a) a sysmem buffer and (b) is
    // compatible with the core codec's current constraints.
    //
    // When buffer_constraints_action_required is false, by definition the core codec isn't changing
    // anything that would make a buffer incompatible (assuming the buffer is allocated per the
    // latest buffer_constraints_version_ordinal with action required true).
    auto buffer_collection_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
    fuchsia_sysmem2::AllocatorBindSharedCollectionRequest bind_request;
    bind_request.token() = std::move(token);
    bind_request.buffer_collection_request() = std::move(buffer_collection_endpoints.server);
    auto bind_result = sysmem_->BindSharedCollection(std::move(bind_request));
    // one-way message; Allocator disconnection is fatal
    ZX_ASSERT(bind_result.is_ok());
    auto buffer_collection = fidl::SyncClient(std::move(buffer_collection_endpoints.client));

    fuchsia_sysmem2::NodeSetNameRequest set_name_request;
    set_name_request.name() = GetBufferName(port);
    set_name_request.priority() = 11;
    // Ignore result; client will find out about allocation failure from sysmem.
    std::ignore = buffer_collection->SetName(std::move(set_name_request));

    fuchsia_sysmem2::NodeSetDebugClientInfoRequest set_client_info_request;
    set_client_info_request.name() = codec_adapter_->CoreCodecGetName();
    set_client_info_request.id() = 0;
    // Ignore result; client will find out about allocation failure from sysmem.
    std::ignore = buffer_collection->SetDebugClientInfo(std::move(set_client_info_request));

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.constraints() = std::move(constraints);
    if (maybe_match_existing_vmo.has_value()) {
      set_constraints_request.must_match_vmo() = TakeOptional(maybe_match_existing_vmo);
    }

    // std::ignore = buffer_collection->SetVerboseLogging();

    // Ignore result; client will find out about allocation failure from sysmem.
    std::ignore = buffer_collection->SetConstraints(std::move(set_constraints_request));
    // Ignore result; client will find out about allocation failure from sysmem.
    std::ignore = buffer_collection->Release();

    // We don't create PortSettings or necessarily have a valid buffer_lifetime_ordinal until
    // AddBuffer. The AddBuffer will get the SingleBufferSettings from sysmem using GetVmoInfo.
    //
    // AddBuffer verifies that concurrently-existing buffers of the same buffer_lifetime_ordinal
    // have the same SingleBufferSettings. This applies regardless of whether there's one
    // ParticipateInAllocation call per buffer_lifetime_ordinal per port, or several.

    // ~buffer_collection
  });
}

std::optional<CodecImpl::BufferLifetimeOrdinalCleanupOutsideLock>
CodecImpl::MaybeDeleteBufferLifetimeOrdinal(CodecPort port, uint64_t buffer_lifetime_ordinal) {
  if (buffer_lifetime_ordinal == buffer_lifetime_ordinal_[port]) {
    // We only delete non-current buffer_lifetime_ordinal(s).
    return std::nullopt;
  }
  auto& active_buffers_by_ordinal = active_buffers_[port];
  auto active_buffers_by_ordinal_iter = active_buffers_by_ordinal.find(buffer_lifetime_ordinal);
  if (active_buffers_by_ordinal_iter != active_buffers_by_ordinal.end()) {
    auto& active_buffers_by_index = active_buffers_by_ordinal_iter->second;
    if (!active_buffers_by_index.empty()) {
      // We still have an active buffer; can't delete this buffer_lifetime_ordinal yet.
      return std::nullopt;
    }
  }
  auto& adding_buffers_by_ordinal = adding_buffers_[port];
  auto adding_buffers_by_ordinal_iter = adding_buffers_by_ordinal.find(buffer_lifetime_ordinal);
  if (adding_buffers_by_ordinal_iter != adding_buffers_by_ordinal.end()) {
    auto& adding_buffers_by_index = adding_buffers_by_ordinal_iter->second;
    if (!adding_buffers_by_index.empty()) {
      // We still have an adding buffer; can't delete this buffer_lifetime_ordinal yet.
      return std::nullopt;
    }
  }
  // We now know that the buffer_lifetime_ordinal is not current, has no active buffers, and has no
  // adding buffers. This means we can now clean up the buffer_lifetime_ordinal. Because the
  // buffer_lifetime_ordinal is not current, this buffer_lifetime_ordinal won't ever get re-created
  // (for this CodecImpl and port).

  BufferLifetimeOrdinalCleanupOutsideLock to_delete;

  // Beyond here, the server can no longer enforce that a RemoveBuffer from the client is not
  // redundant, which is fine.
  if (active_buffers_by_ordinal_iter != active_buffers_by_ordinal.end()) {
    to_delete.buffers_node_handle_to_delete.emplace(
        active_buffers_by_ordinal.extract(active_buffers_by_ordinal_iter));
  }

  auto& packets_by_ordinal = active_packets_[port];
  if (packets_by_ordinal.find(buffer_lifetime_ordinal) != packets_by_ordinal.end()) {
    // This also stops any further calls to CoreCodecRecycleOutputPacket re. this
    // buffer_lifetime_ordinal. The StreamProcessor.RecycleOutputPacket handler also runs on
    // the output/fidl thread, so this aspect doesn't rely on continuing to hold lock_.
    to_delete.packets_node_handle_to_delete.emplace(
        packets_by_ordinal.extract(buffer_lifetime_ordinal));
  }

  to_delete.fake_map_range_to_delete.emplace(
      fake_map_range_[port].extract(buffer_lifetime_ordinal));

  to_delete.adding_buffers_node_handle_to_delete.emplace(
      adding_buffers_by_ordinal.extract(buffer_lifetime_ordinal));

  to_delete.protocol_packets_node_handle_to_delete.emplace(
      protocol_packets_by_protocol_packet_index_[port].extract(buffer_lifetime_ordinal));

  if (active_buffers_by_ordinal.empty() && adding_buffers_by_ordinal.empty()) {
    if (on_zero_buffers_[port].has_value() && *on_zero_buffers_[port]) {
      ZX_DEBUG_ASSERT(IsZeroBuffers(port));
      to_delete.callback_to_run_on_delete = std::move(*on_zero_buffers_[port]);
      // we intentionally leave on_zero_buffers_[port].has_value() here, since
      // we don't need to set on_zero_buffers_[port] again elsewhere
      ZX_DEBUG_ASSERT(on_zero_buffers_[port].has_value());
    }
  }

  return to_delete;
}

void CodecImpl::DeleteBuffer(CodecBuffer* buffer) {
  // We know "this" is alive due to deletion of CodecBuffer.zero_children_wait_ on fidl thread
  // preventing this method from running after ~this on fidl thread, and due to
  // stream_control_queue_.StopAndClear() before ~this, preventing this method from running after
  // ~this on stream control thread.
  //
  // We know buffer is still alive here because of the above and the fact that no forced destruction
  // of buffers happens until ~this.
  CodecPort port = buffer->port();
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  uint64_t buffer_lifetime_ordinal = buffer->lifetime_ordinal();
  uint32_t buffer_index = buffer->index();

  fit::function<void(ScopedLock&)> pending_remove_completion;
  std::optional<CodecImpl::BufferLifetimeOrdinalCleanupOutsideLock> delete_outside_lock;
  BuffersByIndex::node_type buffer_node_handle;
  {  // scope lock
    ScopedLock lock(lock_);
    auto& port_buffers = active_buffers_[port];
    auto buffers_by_ordinal_iter = port_buffers.find(buffer_lifetime_ordinal);
    // Must not be removed until all handles to children are gone, which is only being handled now,
    // so must still be present.
    ZX_DEBUG_ASSERT(buffers_by_ordinal_iter != port_buffers.end());
    auto& buffers_at_ordinal = buffers_by_ordinal_iter->second;
    auto buffer_iter = buffers_at_ordinal.find(buffer_index);
    // Must not be removed until all handles to children are gone, which is only being handled now,
    // so must still be present.
    ZX_DEBUG_ASSERT(buffer_iter != buffers_at_ordinal.end());
    buffer_node_handle = buffers_at_ordinal.extract(buffer_iter);
    pending_remove_completion = std::move(buffer_node_handle.mapped()->pending_remove_completion_);
    // Clean up the whole buffer_lifetime_ordinal if it's both empty and not current.
    delete_outside_lock = MaybeDeleteBufferLifetimeOrdinal(port, buffer_lifetime_ordinal);
  }  // ~lock

  // Destroy the buffer outside the lock to avoid deadlock if destructor fails and calls FailFatal
  buffer_node_handle = {};

  // At this point, if packets_node_handle is not empty, it's impossible for CodecAdapter to see any
  // further CoreCodecRecycleOutputPacket calls with a packet under buffer_lifetime_ordinal.
  //
  // The DeleteBuffer calls all happen on this thread, so are ordered with respect to each other
  // despite not holding lock_ here.
  //
  // If we're deleting the packets under buffer_lifetime_ordinal shortly, inform CodecAdapter first.
  if (delete_outside_lock.has_value()) {
    CoreCodecCloseBufferLifetimeOrdinal(port, buffer_lifetime_ordinal);
    // At this point the CodecAdapter is no longer tracking the old buffer_lifetime_ordinal at all,
    // so it's now safe to delete all the packets under the old buffer_lifetime_ordinal, without the
    // CodecAdapter ever having any CodecPacket pointers that are invalid, even transiently.
    delete_outside_lock.reset();
  }

  ScopedLock lock(lock_);
  if (pending_remove_completion) {
    pending_remove_completion(lock);
  }
}

// A correctly-operating client will only send this message if
// DetailedCodecDescription.supports_dynamic_buffers was set to true.
void CodecImpl::RemoveBuffer(fuchsia::media::StreamProcessorRemoveBufferRequest request,
                             RemoveBufferCallback callback) {
  ZX_DEBUG_ASSERT(IsFidl());
  if (!is_supports_dynamic_buffers()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("client sent RemoveBuffer when !supports_dynamic_buffers");
    return;
  }
  ZX_ASSERT(::codec_impl::internal::kEnableDynamicBuffers);
  // We also re-check after acquiring the lock in RemoveBufferInternal.
  if (IsStopping()) {
    return;
  }
  ZX_DEBUG_ASSERT(!!codec_adapter_);
  if (!request.has_port()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("RemoveBuffer must have port set");
    return;
  }
  if (!request.has_buffer_lifetime_ordinal()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("RemoveBuffer must have buffer_lifetime_ordinal set");
    return;
  }
  if (!request.has_buffer_index()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("RemoveBuffer must have buffer_index set");
    return;
  }
  CodecPort port;
  switch (request.port()) {
    case fuchsia::media::Port::INPUT:
      port = kInputPort;
      break;
    case fuchsia::media::Port::OUTPUT:
      port = kOutputPort;
      break;
    default:
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      Fail("RemoveBuffer must have valid port set");
      return;
  }
  uint64_t buffer_lifetime_ordinal = request.buffer_lifetime_ordinal();
  uint32_t buffer_index = request.buffer_index();
  ZX_DEBUG_ASSERT(port == kInputPort || port == kOutputPort);
  if (port == kInputPort) {
    PostToStreamControl([this, port, buffer_lifetime_ordinal, buffer_index,
                         callback = std::move(callback)]() mutable {
      RemoveBufferInternal(port, buffer_lifetime_ordinal, buffer_index, std::move(callback));
    });
  } else {
    ZX_DEBUG_ASSERT(port == kOutputPort);
    RemoveBufferInternal(port, buffer_lifetime_ordinal, buffer_index, std::move(callback));
  }
}

void CodecImpl::RemoveBufferInternal(CodecPort port, uint64_t buffer_lifetime_ordinal,
                                     uint32_t buffer_index, RemoveBufferCallback callback) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  std::shared_ptr<zx::vmo> buffer_keep_alive;
  CodecBuffer* buffer_to_remove = nullptr;
  {  // scope lock
    ScopedLock lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }
    if (buffer_lifetime_ordinal > protocol_buffer_lifetime_ordinal_[port]) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked("RemoveBuffer has too-new buffer_lifetime_ordinal (vs protocol)");
      return;
    }
    // We intentionally allow buffer_lifetime_ordinal > buffer_lifetime_ordinal_[port] in this path
    // because AddBuffer can choose to not add a buffer (after AddBuffer updates
    // protocol_buffer_lifetime_ordinal_[port]) and not update buffer_lifetime_ordinal_[port], and
    // RemoveBuffer of that same buffer needs to just complete.
    if (is_dynamic_buffers_[port].has_value() && !is_dynamic_buffers_[port] &&
        (buffer_lifetime_ordinal == buffer_lifetime_ordinal_[port])) {
      // At least so far, when not using dynamic buffers, RemoveBuffer is only allowed after the
      // StreamProcessor is moved to a new buffer_lifetime_ordinal (whether an even value due to
      // server-driven action, or an odd value from the client). In this usage, the RemoveBuffer
      // didn't start the remove, but the client can use RemoveBuffer to tell when the removal is
      // done.
      //
      // Among other things this restriction allows us to avoid needing to handle RemoveBuffer in
      // the interval from SetInputBufferPartialSettings or SetOutputBufferPartialSettings to
      // OnBufferCollectionInfo (at least so far).
      //
      // If we want/need to handle this later, we could queue/pend the RemoveBuffer behind
      // OnBufferCollectionInfo (as needed), with dup prevention. It's simpler to reject this case
      // until/unless we need it later.
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "RemoveBuffer for non-dynamic case only supported for non-current buffer_lifetime_ordinal");
      return;
    }

    auto remove_done_async = [this, port,
                              callback = std::move(callback)](ScopedLock& lock) mutable {
      ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
      lock.AssertHeld(lock_);
      if (port == kOutputPort) {
        // This must order after any queued output packets that use the buffer, so queue this in
        // order with stream output.
        PostStreamOutputLocked([callback = std::move(callback)]() mutable {
          std::move(callback)(fuchsia::media::StreamProcessor_RemoveBuffer_Result::WithResponse(
              fuchsia::media::StreamProcessor_RemoveBuffer_Response{}));
        });
      } else {
        ZX_DEBUG_ASSERT(port == kInputPort);
        PostToSharedFidl([callback = std::move(callback)]() mutable {
          std::move(callback)(fuchsia::media::StreamProcessor_RemoveBuffer_Result::WithResponse(
              fuchsia::media::StreamProcessor_RemoveBuffer_Response{}));
        });
      }
    };

    auto& adding_buffers = adding_buffers_[port];
    auto adding_buffers_by_ordinal_iter = adding_buffers.find(buffer_lifetime_ordinal);
    if (adding_buffers_by_ordinal_iter != adding_buffers_[port].end()) {
      auto& adding_buffers_by_index = adding_buffers_by_ordinal_iter->second;
      auto adding_buffers_by_index_iter = adding_buffers_by_index.find(buffer_index);
      if (adding_buffers_by_index_iter != adding_buffers_by_index.end()) {
        auto& adding_buffer = adding_buffers_by_index_iter->second;
        if (adding_buffer->continue_remove_) {
          // This failure case only applies within the same buffer_index lifetime. If the old
          // buffer_index was fully removed previously, then this case won't apply.
          LogEvent(media_metrics::
                       StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
          FailLocked(
              "redundant RemoveBuffer not allowed (adding case) - port: %lu buffer_lifetime_ordinal: %" PRId64
              " buffer_index: %" PRId64,
              port, buffer_lifetime_ordinal, buffer_index);
          return;
        }
        adding_buffer->continue_remove_ = std::move(remove_done_async);
        // The continue_remove_ being set will be noticed during GetVmoInfo completion. Done here.
        return;
      }
    }

    auto& active_buffers = active_buffers_[port];
    auto active_buffers_by_ordinal_iter = active_buffers.find(buffer_lifetime_ordinal);
    if (active_buffers_by_ordinal_iter != active_buffers.end()) {
      auto& active_buffers_by_index = active_buffers_by_ordinal_iter->second;
      auto active_buffers_by_index_iter = active_buffers_by_index.find(buffer_index);
      if (active_buffers_by_index_iter != active_buffers_by_index.end()) {
        auto& active_buffer = active_buffers_by_index_iter->second;
        if (active_buffer->pending_remove_completion_) {
          // This failure case only applies within the same buffer_index lifetime. If the old
          // buffer_index was fully removed previously, then this case won't apply.
          LogEvent(media_metrics::
                       StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
          FailLocked(
              "redundant RemoveBuffer not allowed (active case) - port: %lu buffer_lifetime_ordinal: %" PRId64
              " buffer_index: %" PRId64,
              port, buffer_lifetime_ordinal, buffer_index);
          return;
        }
        buffer_to_remove = active_buffer.get();
        // We complete the RemoveBuffer just after the CodecBuffer is destructed, which occurs when
        // CodecBuffer::parent_vmo_ sees ZX_VMO_ZERO_CHILDREN.
        active_buffer->pending_remove_completion_ = std::move(remove_done_async);
      }
    }

    if (!buffer_to_remove) {
      // We complete immediately since the buffer isn't in active_buffers_[port], so the specified
      // buffer doesn't currently exist. The specified buffer may have already completed removal
      // prior to the RemoveBuffer arriving from the client, or the specified buffer may have never
      // existed (or never been fully added), but at this point we don't distinguish between these
      // possibilities, and we assume the specified buffer refers to an already-removed buffer. In
      // this path we don't enforce that there's never been any previous RemoveBuffer for the same
      // buffer.
      std::move(remove_done_async)(lock);
      return;
    }

    if (buffer_to_remove->is_remove_pending_) {
      // already pending removal at CodecAdapter layer; pending_remove_completion_ set (the first
      // and only time) above which will inform client when removal done
      ZX_DEBUG_ASSERT(buffer_to_remove->pending_remove_completion_);
      ZX_DEBUG_ASSERT(!buffer_to_remove->until_remove_started_child_vmo_);
      return;
    }

    ZX_DEBUG_ASSERT(!buffer_to_remove->is_remove_pending_);
    buffer_to_remove->is_remove_pending_ = true;
    ZX_DEBUG_ASSERT(buffer_to_remove->until_remove_started_child_vmo_);
    buffer_keep_alive = std::move(buffer_to_remove->until_remove_started_child_vmo_);
    ZX_DEBUG_ASSERT(!buffer_to_remove->until_remove_started_child_vmo_);
  }  // ~lock
  ZX_DEBUG_ASSERT(buffer_to_remove);
  if (buffer_to_remove->was_ever_added_to_core_codec()) {
    CoreCodecRemoveBuffer(port, buffer_to_remove);
  }
  // ~buffer_keep_alive, which allows ZX_VMO_ZERO_CHILDREN on buffer_to_remove.parent_vmo_, once the
  // CodecAdapter has closed all its handles to the buffer previously obtained using
  // buffer_to_remove.GetChildVmo, and previously queued output packets using the buffer have been
  // sent. After those things have occurred, we complete the RemoveBuffer.
}

void CodecImpl::EnableOldOutputBuffers() {
  ZX_DEBUG_ASSERT(IsFidl());
  if (!is_supports_dynamic_buffers()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("client sent EnableOldOutputBuffers when !supports_dynamic_buffers");
    return;
  }
  ZX_ASSERT(::codec_impl::internal::kEnableDynamicBuffers);
  ScopedLock lock(lock_);
  if (is_enable_old_output_buffers_) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("EnableOldOutputBuffers can only be sent up to once.");
    return;
  }
  if (protocol_buffer_lifetime_ordinal_[kOutputPort] != 0) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("EnableOldOutputBuffers sent after first output buffer_lifetime_ordinal");
    return;
  }
  is_enable_old_output_buffers_ = true;
}

void CodecImpl::EnableSameOutputBufferConcurrentlyInFlight() {
  ZX_DEBUG_ASSERT(IsFidl());
  if (!is_supports_dynamic_buffers()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    // We fail this instead of ignoring, since ignoring would incorrectly imply that we're promising
    // to be able to decode streams that output the same output buffer more than once concurrently
    // (for example, VP9 streams that use show_existing_frame on the same decoded frame more than
    // once).
    Fail("client sent EnableSameOutputBufferConcurrentlyInFlight when !supports_dynamic_buffers");
    return;
  }
  ZX_ASSERT(::codec_impl::internal::kEnableDynamicBuffers);
  ScopedLock lock(lock_);
  if (is_enable_same_output_buffer_concurrently_in_flight_) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("EnableSameOutputBufferConcurrentlyInFlight can only be sent up to once.");
    return;
  }
  if (protocol_buffer_lifetime_ordinal_[kOutputPort] != 0) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked(
        "EnableSameOutputBufferConcurrentlyInFlight sent after first output buffer_lifetime_ordinal");
    return;
  }
  is_enable_same_output_buffer_concurrently_in_flight_ = true;
}

void CodecImpl::EnableForceOutputBuffersFixedImageSize() {
  ZX_DEBUG_ASSERT(IsFidl());
  if (!is_supports_dynamic_buffers()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    Fail("client sent EnableForceOutputBuffersFixedImageSize when !supports_dynamic_buffers");
    return;
  }
  ZX_ASSERT(::codec_impl::internal::kEnableDynamicBuffers);
  ScopedLock lock(lock_);
  if (is_force_output_buffers_fixed_image_size_) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("EnableForceOutputBuffersFixedImageSize sent more than once");
    return;
  }
  if (!is_force_output_buffers_fixed_image_size_message_permitted_) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    FailLocked("EnableForceOutputBuffersFixedImageSize sent too late");
    return;
  }
  // Unfortunately the client says this is necessary for this StreamProcessor.
  is_force_output_buffers_fixed_image_size_ = true;
}

void CodecImpl::handle_unknown_method(uint64_t ordinal, bool method_has_response) {
  // StreamProcessor servers report which new sets of messages they support via
  // DetailedCodecDescription. Clients are not supposed to send messages that
  // the server didn't report support for via DetailedCodecDescription.
  //
  // For tooling reasons (abi_compat), we add new messages as "flexible", even
  // when we'd ideally like to add most messages as "strict" to get
  // non-recognizing servers to close the channel, since a client shouldn't use
  // messages that the server didn't report as supported. We may be able to add
  // messages as "strict" in future if abi_compat is changed to permit this, in
  // particular when servers and clients can both be in "platform" and
  // "external".
  //
  // For these current reasons, this unknown interaction handler currently logs
  // then closes the channel. This is achieving logically strict behavior for
  // all unknown messages, despite the new messages being added as "flexible"
  // for reasons mentioned above.
  //
  // In future we may be able to ignore new "flexible" messages added in future
  // (if any, in said hypothetical future), but at the moment we must assume all
  // unknown messages are logically strict despite not being "strict" in FIDL.
  Fail("unknown message received; closing channel - ordinal: 0x%" PRIx64
       " 'method_has_response': %u",
       ordinal, method_has_response);
}

void CodecImpl::QueueInputEndOfStream_StreamControl(uint64_t stream_lifetime_ordinal) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  {  // scope lock
    ScopedLock lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }
    if (!CheckStreamLifetimeOrdinalLocked(stream_lifetime_ordinal)) {
      return;
    }

    if (!is_supports_dynamic_buffers()) {
      if (!CheckWaitEnsureInputConfigured(lock)) {
        if (IsStoppingLocked()) {
          return;
        }
        if (stream_) {
          stream_->AssertHeld(this);
          ZX_ASSERT(stream_->future_discarded());
        }
        return;
      }
    }

    ZX_DEBUG_ASSERT(stream_lifetime_ordinal >= stream_lifetime_ordinal_);
    if (stream_lifetime_ordinal > stream_lifetime_ordinal_) {
      // We start a new stream given an end-of-stream for a stream we've not
      // seen before, since allowing empty streams to not be errors may be nicer
      // to use.
      if (!StartNewStream(lock, stream_lifetime_ordinal, /*is_for_packet=*/false)) {
        return;
      }
    }

    if (stream_->failure_seen()) {
      // Already reported to client.
      return;
    }

    if (stream_->input_end_of_stream()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked("client already sent QueueInputEndOfStream() for this stream");
      return;
    }
    stream_->SetInputEndOfStream();

    stream_->AssertHeld(this);
    if (stream_->future_discarded()) {
      // Don't queue to core codec.  The stream_ may have never fully started,
      // or may have been future-discarded since. Either way, skip queueing to
      // core codec. We only really must do this because the stream may not have
      // ever fully started, in the case where the client moves on to a new
      // stream before catching up to latest output config.
      return;
    }
  }  // ~lock

  CoreCodecQueueInputEndOfStream();
}

zx_status_t CodecImpl::Pin(uint32_t options, const zx::vmo& vmo, uint64_t offset, uint64_t size,
                           zx_paddr_t* addrs, size_t addrs_count, zx::pmt* pmt) {
  ZX_DEBUG_ASSERT(*core_codec_bti_);
  return core_codec_bti_->pin(options, vmo, offset, size, addrs, addrs_count, pmt);
}

bool CodecImpl::CheckWaitEnsureInputConfigured(ScopedLock& lock) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  lock.AssertHeld(lock_);
  // Ensure/finish input configuration.
  if (!IsPortAtLeastPartiallyConfiguredLocked(kInputPort)) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked(
        "client QueueInput*() with input buffers not at least partially "
        "configured");
    return false;
  }
  ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[kInputPort] % 2 == 1);
  ZX_DEBUG_ASSERT(is_dynamic_buffers_[kInputPort].has_value());
  if (*is_dynamic_buffers_[kInputPort]) {
    // In this case, if the caller needs a specific buffer to be done adding, the caller can wait
    // for that itself (in that case, no need to take any action on fidl thread that isn't already
    // started; it's just that an already-started GetVmoInfo may need to complete).
    return true;
  }
  // The client is required to know that sysmem is in fact done allocating the
  // BufferCollection successfully before the client sends
  // QueueInput...StreamControl.  We can't trust a client to necessarily get
  // that right however, so rather than just getting stuck indefinitely in that
  // case, we detect by asking sysmem to verify that it has allocated the
  // BufferCollection successfully.  This verification happens async, but will
  // shortly cause WaitEnsureSysmemReadyOnInput() to return and
  // IsStoppingLocked() to return true if verification fails.
  if (!IsInputConfiguredLocked()) {
    PostToSharedFidl([this, buffer_lifetime_ordinal = buffer_lifetime_ordinal_[kInputPort]] {
      ScopedLock lock(lock_);
      if (IsStoppingLocked()) {
        return;
      }
      if (buffer_lifetime_ordinal != buffer_lifetime_ordinal_[kInputPort]) {
        // stale; no problem; old buffers were allocated fine and client already
        // moved on after that.
        return;
      }
      // Else previous buffer_lifetime_ordinal check would have noticed.
      ZX_DEBUG_ASSERT(port_settings_[kInputPort]);
      // paranoid check - assert above believed to be valid
      if (!port_settings_[kInputPort]) {
        return;
      }
      auto& buffer_collection = port_settings_[kInputPort]->buffer_collection();
      // Else IsStoppingLocked() check above would have returned.
      ZX_DEBUG_ASSERT(!!buffer_collection && buffer_collection->is_valid());
      // paranoid check - assert above believed to be valid
      if (!buffer_collection || !buffer_collection->is_valid()) {
        return;
      }
      (*buffer_collection)
          ->CheckAllBuffersAllocated()
          .Then([this, buffer_lifetime_ordinal](
                    fidl::Result<::fuchsia_sysmem2::BufferCollection::CheckAllBuffersAllocated>&
                        result) {
            ScopedLock lock(lock_);
            if (IsStoppingLocked()) {
              return;
            }
            if (buffer_lifetime_ordinal != buffer_lifetime_ordinal_[kInputPort]) {
              // stale; no problem; old buffers were allocated fine and client
              // already moved on after that.
              return;
            }
            if (result.is_error()) {
              // This will cause any in-progress WaitEnsureSysmemReadyOnInput()
              // to return shortly and IsStoppingLocked() will be true.
              LogEvent(media_metrics::
                           StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
              FailLocked(
                  "Probably client did QueueInput* before the client "
                  "determined that sysmem was done successfully allocating "
                  "buffers after most recent SetInputBufferPartialSettings(): %d",
                  result.error_value());
              return;
            }
          });
    });
    if (!WaitEnsureSysmemReadyOnInput(lock)) {
      ZX_DEBUG_ASSERT(IsStoppingLocked());
      return false;
    }
  }
  if (!IsInputConfiguredLocked()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("client QueueInput*() with input buffers not configured");
    return false;
  }
  return true;
}

void CodecImpl::UnbindLocked() {
  VLOGF("UnbindLocked top");
  // We must have first gotten far enough through BindAsync() before calling UnbindLocked().
  ZX_DEBUG_ASSERT(was_logically_bound_);

  // The only stores are under lock_, so we can do a relaxed load here due to being under lock_.
  if (was_unbind_started_.load(std::memory_order_relaxed)) {
    VLOGF("was_unbind_started_.load(std::memory_order_relaxed)");
    // Ignore the second trigger if we have a near-simultaneous failure from
    // StreamControl thread (for example) and from fidl_thread() (for
    // example).  The first will start unbinding, and the second will be
    // ignored.  Since completion of the Unbind() call doesn't imply anything
    // about how done the unbind is, there's no need for the second caller to
    // be blocked waiting for the first caller's unbind to be done.
    return;
  }

  if (codec_admission_) {
    codec_admission_->SetCodecIsClosing();
  }

  // Tell StreamControl to not start any more work. This store is both under lock_ and seq_cst.
  // Being under lock_ for this store (the only store in the code) allows readers under lock_ to
  // do a relaxed load. Doing a seq_cst store here allows a reader that's not under lock_ to do a
  // seq_cst load and know that it'll see a/the prior store.
  VLOGF("was_unbind_started_.store(true)");
  was_unbind_started_.store(true, std::memory_order_seq_cst);
  // This allows any ongoing block/wait by StreamControl to complete early / not stay blocked.
  wake_stream_control_condition_.notify_all();

  // Unbind() / UnbindLocked() can be called from any thread.
  //
  // Regardless of what thread UnbindLocked() is called on, "this" will remain
  // allocated at least until the caller of UnbindLocked() releases lock_.
  //
  // In all cases, the posted lambda runs after BindAsync()'s work that's posted
  // to StreamControl, because any/all calls to UnbindLocked() happen after
  // BindAsync() has posted to StreamControl.
  //
  // We know the stream_control_queue_ isn't stopped yet, because the present
  // method is idempotent and the lambda being posted just below has the only
  // call to stream_control_queue_.StopAndClear().
  ZX_DEBUG_ASSERT(!stream_control_queue_.is_stopped());

  if (IsLegacyUnbind()) {
    VLOGF("IsLegacyUnbind()");
    // old way
    LegacyUnbindLockedInternal();
    return;
  }

  // new way
  VLOGF("calling UnbindLockedInternalAsync");
  UnbindLockedInternalAsync();
}

void CodecImpl::UnbindLockedInternalAsync() {
  ZX_DEBUG_ASSERT(!IsLegacyUnbind());

  // this is posting from unknown to shared fidl
  PostToSharedFidl([this] {
    VLOGF("UnbindLockedInternalAsync lambda on shared fidl");
    // Go ahead and ensure the binding_ is unbound first so we stop getting fidl dispatch calls,
    // without closing the channel yet; this isn't strictly relied on since every handler checks
    // IsStopping()/IsStoppingLocked(), but may as well end the dispatch calls first.
    if (binding_.is_bound()) {
      ZX_DEBUG_ASSERT(!codec_to_close_.is_valid());
      codec_to_close_ = binding_.Unbind().TakeChannel();
      ZX_DEBUG_ASSERT(codec_to_close_.is_valid());
    }
    VLOGF("calling PostToStreamControl");
    PostToStreamControl([this] {
      VLOGF("calling AsyncShutdownStepEndStreamAndRemoveInputBuffers on StreamControl");
      AsyncShutdownStepEndStreamAndRemoveInputBuffers();
    });
  });
}

void CodecImpl::AsyncShutdownStepEndStreamAndRemoveInputBuffers() {
  VLOGF("AsyncShutdownStepEndStreamAndRemoveInputBuffers top");
  // At this point we know that no more streams will be started by
  // StreamControl ordering domain (thanks to was_unbind_started_ /
  // IsStoppingLocked() checks), but lambdas posted to the StreamControl
  // ordering domain (by the fidl_thread() or by core codec) may still
  // be creating other activity such as posting lambdas to StreamControl or
  // fidl_thread().
  {  // scope lock
    ScopedLock lock(lock_);
    // Stop CodecAdapter associated with this CodecImpl, partly to make sure
    // it stops running code that could make calls into this CodecImpl, and
    // partly to ensure the CodecAdapter isn't in the middle of anything when
    // it gets deleted.
    //
    // We know the CodecAdapter won't start more activity because the
    // CodecAdapter isn't allowed to initiate actions while there's no active
    // stream, and because no new active stream will be created.  All
    // _StreamControl methods check IsStoppingLocked() at the start, and the
    // StreamControl ordering domain is the only domain that ever starts a
    // stream.
    //
    // We intentionally don't check for IsStoppingLocked() in protocol
    // dispatch methods running on fidl_thread(). For example the codec must
    // tolerate calls to configure buffers after EnsureStreamClosed() here.
    // The Unbind() later is what silences the protocol message dispatch
    // methods.  Checking for IsStoppingLocked() in protocol dispatch methods
    // would only decrease the probability of certain event orderings, not
    // eliminate those orderings, so it's actually better to let them happen
    // to get more coverage of those orderings.
    if (is_core_codec_init_called_) {
      VLOGF("calling EnsureStreamClosed");
      EnsureStreamClosed(lock);
      VLOGF("calling EnsureBuffersNotConfigured(input)");
      EnsureBuffersNotConfigured(lock, kInputPort, true);
      VLOGF("EnsureBuffersNotConfigured done");
    }

    // Because the current async path is the only path that sets this bool to true, and the
    // current path is run-once.
    ZX_DEBUG_ASSERT(!is_stream_control_done_);
    ZX_DEBUG_ASSERT(!shared_fidl_queue_.is_stopped());
  }
  VLOGF("calling PostToSharedFidl");
  PostToSharedFidl([this] {
    VLOGF(
        "calling AsyncShutdownStepWaitForZeroInputBuffersAndEnsureZeroOutputBuffers on shared fidl");
    AsyncShutdownStepWaitForZeroInputBuffersAndEnsureZeroOutputBuffers();
  });
}

void CodecImpl::AsyncShutdownStepWaitForZeroInputBuffersAndEnsureZeroOutputBuffers() {
  VLOGF("AsyncShutdownStepWaitForZeroInputBuffersAndEnsureZeroOutputBuffers top");
  ZX_DEBUG_ASSERT(IsFidl());

  // if this is still !zero_buffers_countdown after the lock is released, it means there were
  // already zero input buffers and zero output buffers
  //
  // else this counts how many ports still have buffers, decrementing to 0 when the last port with
  // buffers has removed its last buffer
  std::shared_ptr<std::atomic<uint32_t>> zero_buffers_countdown;
  {  // scope lock
    VLOGF("acquiring lock...");
    ScopedLock lock(lock_);
    VLOGF("lock acquired");

    // We ensure CoreCodecCloseBufferLifetimeOrdinal has run before continuing async toward
    // error_handler. This allows a CodecAdapter to assert that all ordinals are gone before
    // destruction. We know "this" remains allocated because error_handler hasn't been called yet;
    // this async path is run-once and the only path that runs error_handler other than super early
    // failure of BindAsync in which case this async path never runs.
    //
    // Next step may need to check a few times before it moves on to the step after that.
    //
    // All input buffer_lifetime_ordinal(s) should go away shortly. No past ordinals are current.
    VLOGF("calling EnsureBuffersNotConfigured(output)");
    EnsureBuffersNotConfigured(lock, kOutputPort, true);
    VLOGF("EnsureBuffersNotConfigured(output) done");

    for (auto port : kPorts) {
      VLOGF("checking port: %u", port);
      if (!IsZeroBuffers(port)) {
        VLOGF("!IsZeroBuffers");
        if (!zero_buffers_countdown) {
          zero_buffers_countdown = std::make_shared<std::atomic<uint32_t>>();
        }
        zero_buffers_countdown->fetch_add(1);
        // after ZX_VMO_ZERO_CHILDREN handler has a chance to run on the fidl thread enough times,
        // we'll move on to the next step; the lock_ is acquired when deciding to run
        // on_zero_buffers_, so we know we can install both on_zero_buffers_ (under lock_ here)
        // without racing with the first installed on_zero_buffers_ for other port
        ZX_DEBUG_ASSERT(!on_zero_buffers_[port].has_value());
        VLOGF("installing on_zero_buffers_");
        on_zero_buffers_[port] = [this, port, zero_buffers_countdown] {
          VLOGF("on_zero_buffers_ lambda callback for port: %u", port);
          ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() ||
                          port == kOutputPort && IsFidl());
          // The current lambda won't run for either port until after up to both ports have
          // on_zero_buffers_ installed including the initial increments of *zero_buffers_countdown.
          // So we know this won't spurriously hit zero, as up to both ports increment before either
          // port decrements.
          uint32_t after_val = zero_buffers_countdown->fetch_sub(1) - 1;
          VLOGF("after_val: %u", after_val);
          if (after_val == 0) {
            // this can be posting from fidl (if last port reaching zero buffers was output) or from
            // StreamControl (if last was input)
            VLOGF("calling PostToStreamControl");
            PostToStreamControl([this] {
              VLOGF("calling AsyncShutdownStepQuitStreamControl");
              AsyncShutdownStepQuitStreamControl();
            });
          }
        };
      }
    }
    ZX_DEBUG_ASSERT(!zero_buffers_countdown ||
                    zero_buffers_countdown->load(std::memory_order_relaxed) <= 2);
  }  // ~lock

  if (zero_buffers_countdown) {
    // AsyncShutdownStepQuitStreamControl will get called shortly on StreamControl when zero
    // buffers remain (both zero input buffers and zero output buffers).
    return;
  }

  // there were already zero input buffers and zero output buffers; move on to next step
  PostToStreamControl([this] { AsyncShutdownStepQuitStreamControl(); });
}

void CodecImpl::AsyncShutdownStepQuitStreamControl() {
  VLOGF("AsyncShutdownStepQuitStreamControl top");
  ZX_DEBUG_ASSERT(IsStreamControl());

  fit::closure owner_error_handler;
  {  // scope lock
    ScopedLock lock(lock_);

    // We do this from here so we know that this thread won't run any more
    // tasks after the currently-running task.
    //
    // The currently-running StreamControl task (this method) still gets to
    // run to completion.
    stream_control_dispatcher_->QuitAsync();

    // This deletes any further tasks already queued to StreamControl, and will immediately
    // delete any additional tasks that try to queue to StreamControl.  We also need to ensure the
    // first time stream_control_queue_.StopAndClear() runs is on StreamControl, per
    // ClosureQueue's usage rules.
    VLOGF("calling stream_control_queue_.StopAndClear()");
    stream_control_queue_.StopAndClear();

    // We're ready to let ~CodecImpl do the rest.
    //
    // The core codec has been stopped, so it has no current stream.  The core codec is required
    // to be delete-able when it has no current stream, and required not to asynchronously post
    // more work to the CodecImpl (because calling onCoreCodec... methods is not allowed when
    // there is no current stream).
    //
    // The binding_.Unbind() has already run previously on fidl dispatcher and won't run again
    // during EnsureUnbindCompleted().
    //
    // The stream_control_dispatcher.Join() will run during ~CodecImpl, so no more activity from
    // StreamControl after that.
    //
    // Anything posted using PostToSharedFidl() can be deleted instead of run since the whole
    // CodecImpl is going away, and shared_fidl_queue_ makes it safe for ~CodecImpl to complete
    // without needing to wait/fence past previously-posted labmdas to FIDL thread.
    is_stream_control_done_ = true;
    // Must notify_all() under lock_ in this case since the rest of ~CodecImpl can run as soon as
    // is_stream_control_done_ = true just above.
    stream_control_done_condition_.notify_all();

    // We need to run the owner_error_handler_ on the FIDL thread, which will in turn call
    // ~CodecImpl.
    owner_error_handler = std::move(owner_error_handler_);
  }  // ~lock

  VLOGF("calling PostToSharedFidl");
  PostToSharedFidl([this, client_error_handler = std::move(owner_error_handler)] {
    ZX_DEBUG_ASSERT(IsFidl());
    // We go ahead and finish up the un-binding aspects (because we can free up resources prior to
    // the client code potentially running ~CodecImpl async later).
    VLOGF("calling EnsureUnbindCompleted");
    EnsureUnbindCompleted();
    // This call is expected to run ~CodecImpl, either synchronously during this
    // call or shortly later async.
    is_client_error_handler_called_ = true;
    VLOGF("calling client_error_handler...");
    client_error_handler();
    VLOGF("client_error_handler done");
    // "this" can be gone here
  });
  // "this" will be deleted shortly as soon as when lambda posted just above runs, which may be
  // immediately wrt. this thread
  //
  // "this" can be gone here
}

// This is an older way to accomplish unbind, and is not sufficiently async to be compatible with
// is_sharing_fidl_domain_for_core_codec_ || is_supports_dynamic_buffers_.
void CodecImpl::LegacyUnbindLockedInternal() {
  // This method is not designed to work when is_sharing_fidl_domain_for_core_codec_ ||
  // is_supports_dynamic_buffers_. CodecImpl client code should call UnbindAsync() instead, and
  // don't ~CodecImpl until the error handler passed to BindAsync runs. This all relies on DFv2
  // async PrepareStop, so under DFv1 this may still not be an option.
  ZX_ASSERT(IsLegacyUnbind());

  PostToStreamControl([this] {
    // At this point we know that no more streams will be started by
    // StreamControl ordering domain (thanks to was_unbind_started_ /
    // IsStoppingLocked() checks), but lambdas posted to the StreamControl
    // ordering domain (by the fidl_thread() or by core codec) may still
    // be creating other activity such as posting lambdas to StreamControl or
    // fidl_thread().
    {  // scope lock
      ScopedLock lock(lock_);
      // Stop CodecAdapter associated with this CodecImpl, partly to make sure
      // it stops running code that could make calls into this CodecImpl, and
      // partly to ensure the CodecAdapter isn't in the middle of anything when
      // it gets deleted.
      //
      // We know the CodecAdapter won't start more activity because the
      // CodecAdapter isn't allowed to initiate actions while there's no active
      // stream, and because no new active stream will be created.  All
      // _StreamControl methods check IsStoppingLocked() at the start, and the
      // StreamControl ordering domain is the only domain that ever starts a
      // stream.
      //
      // We intentionally don't check for IsStoppingLocked() in protocol
      // dispatch methods running on fidl_thread(). For example the codec must
      // tolerate calls to configure buffers after EnsureStreamClosed() here.
      // The Unbind() later is what silences the protocol message dispatch
      // methods.  Checking for IsStoppingLocked() in protocol dispatch methods
      // would only decrease the probability of certain event orderings, not
      // eliminate those orderings, so it's actually better to let them happen
      // to get more coverage of those orderings.
      if (is_core_codec_init_called_) {
        EnsureStreamClosed(lock);
        EnsureBuffersNotConfigured(lock, kInputPort, true);
      }

      // Because the current path is the only path that sets this bool to true,
      // and the current path is run-once.
      ZX_DEBUG_ASSERT(!is_stream_control_done_);
      // Because stream_control_done_ is false, and ~CodecImpl waits for
      // is_stream_control_done_ true before shared_fidl_queue_.StopAndClear().
      ZX_DEBUG_ASSERT(!shared_fidl_queue_.is_stopped());

      // We do this from here so we know that this thread won't run any more
      // tasks after the currently-running task.
      //
      // The currently-running StreamControl task (this method) still gets to
      // run to completion.
      stream_control_dispatcher_->QuitAsync();

      // This deletes any further tasks already queued to StreamControl, and will immediately
      // delete any additional tasks that try to queue to StreamControl.  We also need to ensure the
      // first time stream_control_queue_.StopAndClear() runs is on StreamControl, per
      // ClosureQueue's usage rules.
      stream_control_queue_.StopAndClear();

      // We're ready to let EnsureUnbindCompleted() and ~CodecImpl do the rest.
      //
      // The core codec has been stopped, so it has no current stream.  The core codec is required
      // to be delete-able when it has no current stream, and required not to asynchronously post
      // more work to the CodecImpl (because calling onCoreCodec... methods is not allowed when
      // there is no current stream).
      //
      // The binding_.Unbind() will run during EnsureUnbindCompleted() on the FIDL thread, so no
      // more FIDL dispatching to this CodecImpl after that.
      //
      // The stream_control_dispatcher.Join() will run during ~CodecImpl, so no more activity from
      // StreamControl after that.
      //
      // Anything posted using PostToSharedFidl() can be deleted instead of run since the whole
      // CodecImpl is going away, and shared_fidl_queue_ makes it safe for ~CodecImpl to complete
      // without needing to wait/fence past previously-posted labmdas to FIDL thread.
      is_stream_control_done_ = true;
      // Must notify_all() under lock_ in this case since the rest of ~CodecImpl can run as soon as
      // is_stream_control_done_ = true just above.
      stream_control_done_condition_.notify_all();

      // If we're not running from ~CodecImpl, we need to run the owner_error_handler_ on the FIDL
      // thread, which will in turn call ~CodecImpl.  If we are running from ~CodecImpl, then we're
      // already on the FIDL thread, and this posted work won't run thanks to shared_fidl_queue_
      // just deleting the posted task instead, in which case the owner_error_handler_ just gets
      // deleted instead of running (the usual semantics in response to unsolicited destruction).
      //
      // Must post under lock_ in this case else ~CodecImpl can have already finished as soon as
      // stream_control_done_ = true above.
      PostToSharedFidl([this, client_error_handler = std::move(owner_error_handler_)] {
        ZX_DEBUG_ASSERT(IsFidl());
        // We go ahead and finish up the un-binding aspects (because we can free up
        // resources prior to the client code potentially running ~CodecImpl async
        // later).
        //
        // However, this doesn't finish up aspects related to ordering release of
        // resources before acquisition of new resources.  In particular, this call
        // unbinds the channel, but intentionally doesn't close the channel itself until
        // after ~CodecImpl and after ~CodecAdmission.  The intent is to prevent the
        // possibility that overly-agressive client retries on channel closure by the
        // server could build up many CodecImpl instances, even if different instances
        // happen to use different FIDL threads (also potentially different than FIDL
        // thread on which a new CodecAdmission is created). By only closing the channel
        // itself as the last thing after all other cleanup is fully done, we don't
        // trigger the client to create a new CodecImpl while the old one still exists.
        EnsureUnbindCompleted();
        // This call is expected to run ~CodecImpl, either synchronously during this
        // call or shortly later async.
        is_client_error_handler_called_ = true;
        client_error_handler();
      });
    }  // ~lock

    // "this" will be deleted shortly async when lambda posted just above runs, or we're returning
    // back to rest of ~CodecImpl, or ~CodecImpl is racing/running separately and completing
    // immediately after ~lock just above. Regardless, done here.
    return;
  });
  // "this" remains allocated until caller releases lock_.
}

void CodecImpl::CaptureCoreCodecOrderingDomain(async_dispatcher_t* calling_core_codec_dispatcher) {
  std::lock_guard<std::mutex> lock(checker_core_codec_lock_);
  // CaptureCoreCodecOrderingDomain or SetSharingFidlDomainForCoreCodec, not both.
  ZX_DEBUG_ASSERT(!is_sharing_fidl_domain_for_core_codec_);
  is_capture_core_codec_ordering_domain_called_ = true;
  checker_core_codec_.emplace(async::synchronization_checker(calling_core_codec_dispatcher));
  // by definition true now; if this assert fails it may indicate that the caller isn't calling on a
  // thread managed by calling_core_codec_dispatcher (required to call this method), though this
  // check isn't necessarliy guaranteed to detect that first or at all
  ZX_DEBUG_ASSERT(IsCoreCodec());
}

void CodecImpl::UnbindAsync() {
  ZX_DEBUG_ASSERT(IsFidl());
  std::lock_guard<std::mutex> lock(lock_);
  ZX_DEBUG_ASSERT(was_bind_async_called_);

  // Unlike here, UnbindLocked can assume this is true, because internal calls to UnbindLocked are
  // only triggered if was_logically_bound_. But here, we can't assume this is true, as the client
  // code doesn't know whether BindAsync failed early or not.
  if (!was_logically_bound_) {
    VLOGF("!was_logically_bound_");
    // BindAsync failed before really binding, the BindAsync error_handler will run soon async on
    // fidl dispatcher.
    return;
  }

  // We don't need BindAsync to be fully done yet. If it isn't fully done, UnbindLocked() can
  // basically chase just behind BindAsync through StreamControl then back to fidl thread, which is
  // fine.
  ZX_DEBUG_ASSERT(was_logically_bound_);

  // Caller shouldn't be calling UnbindAsync after owner_error_handler_ is called. Both are
  // restricted to only run on fidl dispatcher (always serialized). If this turns out to be onerous
  // for client code we could relax this restriction, but the client would still need to avoid
  // calling UnbindAsync on an already-destructed CodecImpl so this check seems worth having for
  // now.
  ZX_DEBUG_ASSERT(owner_error_handler_);

  // This can be a nop if was_unbind_started_ is already true; either way the error_handler passed
  // to BindAsync gets run on the fidl thread fairly soon after UnbindAsync returns.
  VLOGF("calling UnbindLocked");
  UnbindLocked();
}

void CodecImpl::Unbind() {
  std::lock_guard<std::mutex> lock(lock_);
  UnbindLocked();
  // ~lock
  //
  // "this" may be deleted very shortly after ~lock, depending on what thread
  // Unbind() is called from.
}

void CodecImpl::EnsureUnbindCompleted() {
  ZX_DEBUG_ASSERT(IsFidl());
  ZX_DEBUG_ASSERT(was_logically_bound_);
  if (was_unbind_completed_) {
    return;
  }
  // Or will be, before this method returns.
  was_unbind_completed_ = true;

  // Unbind from the channel so we won't see any more incoming FIDL messages. This binding doesn't
  // own "this".
  //
  // The Unbind() stops any additional FIDL dispatching re. this CodecImpl.
  if (binding_.is_bound()) {
    // This call unbinds the channel, but intentionally doesn't close the channel itself until
    // after ~CodecImpl and after ~CodecAdmission. The intent is to prevent the possibility that
    // overly-agressive client retries on channel closure by the server could build up many
    // CodecImpl instances. By only closing the channel itself as the last thing after all other
    // cleanup is fully done, we don't trigger the client to create a new CodecImpl while the old
    // one still exists.
    codec_to_close_ = binding_.Unbind().TakeChannel();
  }

  // Join() now after prior QuitAsync() called from StreamControl. This will be quick.
  stream_control_dispatcher_->Join();

  // Any previously-posted tasks via shared_fidl_queue_ are deleted here without running.
  //
  // If we're shutting down because UnbindLocked() was run first upon discovery of an
  // internally-noticed error, then previously-queued sending of FIDL messages on the FIDL thread
  // already ran before the EnsureUnbindCompleted(), which was posted after the sends.
  //
  // If we're running ~CodecImpl because the client code is just deleting CodecImpl for whatever
  // client-initiated reason, then previously queueud sending of FIDL messages can be just deleted
  // here without the sends actually occurring, which is fine since in that case the client code
  // has no particular expectation that any particular messages were sent before deletion vs. not
  // getting sent due to deletion.
  //
  // We do this before causing !sysmem_ to avoid queued tasks needing to check for !sysmem_.
  shared_fidl_queue_.StopAndClear();

  {  // scope lock
    ScopedLock lock(lock_);

    // When !IsLegacyUnbind() this won't have anything to do.
    EnsureBuffersNotConfigured(lock, kOutputPort, true);

    // By this point both PortSettings should have already been deleted.
    ZX_DEBUG_ASSERT(!port_settings_[kInputPort]);
    ZX_DEBUG_ASSERT(!port_settings_[kOutputPort]);

    // Unbind the sysmem_ fuchsia::sysmem2::Allocator connection - this also ensures that any
    // in-flight requests' completions will not run.
    //
    // We check sysmem_.is_valid() here because the current fidl_thread work item can be be running
    // ~CodecImpl (or fidl_thread part of UnbindLocked) before the remainder of BindAsync that would
    // set sysmem_. In that case, the current work item will finish running, but remaining work
    // items on shared_fidl_queue_ won't run because of the shared_fidl_queue_.StopAndClear() above.
    // So in that case, sysmem_ will remain !is_valid() until destruction.
    if (sysmem_.is_valid()) {
      // None of the UnbindMaybeGetEndpoint error cases prevent the client end from being closed,
      // which is the only thing we care about here.
      (void)sysmem_.UnbindMaybeGetEndpoint();
    }
  }  // ~lock
}

fuchsia::mediacodec::SecureMemoryMode CodecImpl::OutputSecureMemoryMode() {
  if (!IsDecoder() && !IsDecryptor()) {
    return fuchsia::mediacodec::SecureMemoryMode::OFF;
  }
  if (IsDecoder()) {
    if (!decoder_params().has_secure_output_mode()) {
      return fuchsia::mediacodec::SecureMemoryMode::OFF;
    }
    return decoder_params().secure_output_mode();
  } else {
    ZX_DEBUG_ASSERT(IsDecryptor());
    if (!decryptor_params().has_require_secure_mode()) {
      return fuchsia::mediacodec::SecureMemoryMode::OFF;
    }
    return decryptor_params().require_secure_mode() ? fuchsia::mediacodec::SecureMemoryMode::ON
                                                    : fuchsia::mediacodec::SecureMemoryMode::OFF;
  }
}

fuchsia::mediacodec::SecureMemoryMode CodecImpl::InputSecureMemoryMode() {
  if (!IsDecoder()) {
    return fuchsia::mediacodec::SecureMemoryMode::OFF;
  }
  if (!decoder_params().has_secure_input_mode()) {
    return fuchsia::mediacodec::SecureMemoryMode::OFF;
  }
  return decoder_params().secure_input_mode();
}

fuchsia::mediacodec::SecureMemoryMode CodecImpl::PortSecureMemoryMode(CodecPort port) {
  if (port == kOutputPort) {
    return OutputSecureMemoryMode();
  } else {
    ZX_DEBUG_ASSERT(port == kInputPort);
    return InputSecureMemoryMode();
  }
}

bool CodecImpl::IsPortSecureRequired(CodecPort port) {
  // Return false for DYNAMIC, if/when we add that.
  return PortSecureMemoryMode(port) == fuchsia::mediacodec::SecureMemoryMode::ON;
}

bool CodecImpl::IsPortSecurePermitted(CodecPort port) {
  // Return true for DYNAMIC, if/when we add that.
  return PortSecureMemoryMode(port) != fuchsia::mediacodec::SecureMemoryMode::OFF;
}

bool CodecImpl::IsStreamActiveLocked() {
  ZX_DEBUG_ASSERT(!!stream_ == (stream_lifetime_ordinal_ % 2 == 1));
  return !!stream_;
}

void CodecImpl::SetBufferSettingsCommon(
    ScopedLock& lock, CodecPort port, fuchsia::media::StreamBufferPartialSettings* partial_settings,
    const fuchsia::media::StreamBufferConstraints& stream_constraints) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  ZX_DEBUG_ASSERT(!IsStoppingLocked());
  lock.AssertHeld(lock_);

  if (!partial_settings->has_buffer_lifetime_ordinal()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("partial_settings do not have buffer lifetime ordinal");
    return;
  }
  if (!partial_settings->has_buffer_constraints_version_ordinal()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("partial_settings do not have buffer constraints version ordinal");
    return;
  }
  if ((!partial_settings->has_sysmem_token() || !partial_settings->sysmem_token().is_valid()) &&
      (!partial_settings->has_sysmem2_token() || !partial_settings->sysmem2_token().is_valid())) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("partial_settings missing valid sysmem2_token (sysmem_token also missing)");
    return;
  }
  if (partial_settings->has_sysmem_token() && partial_settings->has_sysmem2_token()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked(
        "partial_settings must have only sysmem2_token or sysmem_token, not both (prefer sysmem2_token)");
    return;
  }
  if (partial_settings->has_sysmem_token() && !partial_settings->has_sysmem2_token()) {
    LOG(WARN,
        "client is using deprecated sysmem_token field; client should switch to sysmem2_token field");
    // Token channels served by sysmem serve both sysmem(1) and sysmem2 BufferCollectionToken on the
    // same channel.
    partial_settings->set_sysmem2_token(
        fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken>(
            partial_settings->mutable_sysmem_token()->TakeChannel()));
    partial_settings->clear_sysmem_token();
  }

  if (is_dynamic_buffers_[port].has_value() && *is_dynamic_buffers_[port]) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    // Once dynamic buffers are used on a port, that port can't use non-dynamic buffers for rest of
    // CodecImpl instance lifetime. The client can use a new StreamProcessor instance instead.
    FailLocked(
        "client used SetInputBufferPartialSettings / SetOutputBufferPartialSettings after dynamic");
    return;
  }
  ZX_DEBUG_ASSERT(!is_dynamic_buffers_[port].has_value() || !*is_dynamic_buffers_[port]);
  is_dynamic_buffers_[port] = false;

  ZX_DEBUG_ASSERT(
      !port_settings_[port] ||
      (buffer_lifetime_ordinal_[port] >= port_settings_[port]->buffer_lifetime_ordinal() &&
       buffer_lifetime_ordinal_[port] <= port_settings_[port]->buffer_lifetime_ordinal() + 1));

  // Extract buffer_lifetime_ordinal and buffer_constraints_version_ordinal from
  // StreamBufferPartialSettings
  // is providing.
  uint64_t buffer_lifetime_ordinal = partial_settings->buffer_lifetime_ordinal();

  uint64_t buffer_constraints_version_ordinal =
      partial_settings->buffer_constraints_version_ordinal();

  if (buffer_lifetime_ordinal <= protocol_buffer_lifetime_ordinal_[port]) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked(
        "buffer_lifetime_ordinal <= "
        "protocol_buffer_lifetime_ordinal_[port] - port: %d",
        port);
    return;
  }
  if (buffer_lifetime_ordinal % 2 == 0) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked(
        "Only odd values for buffer_lifetime_ordinal are permitted - port: %d "
        "value %lu",
        port, buffer_lifetime_ordinal);
    return;
  }
  protocol_buffer_lifetime_ordinal_[port] = buffer_lifetime_ordinal;

  if (buffer_constraints_version_ordinal > sent_buffer_constraints_version_ordinal_[port]) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("Client sent too-new buffer_constraints_version_ordinal - port: %d", port);
    return;
  }

  if (buffer_constraints_version_ordinal <
      last_required_buffer_constraints_version_ordinal_[port]) {
    // ignore - client may catch up later
    return;
  }

  // We've peeled off too new and too old above.
  ZX_DEBUG_ASSERT(buffer_constraints_version_ordinal >=
                      last_required_buffer_constraints_version_ordinal_[port] &&
                  buffer_constraints_version_ordinal <=
                      sent_buffer_constraints_version_ordinal_[port]);

  // We've already checked above that the buffer_lifetime_ordinal is in
  // sequence.
  ZX_DEBUG_ASSERT(!port_settings_[port] ||
                  buffer_lifetime_ordinal > buffer_lifetime_ordinal_[port]);

  if (!ValidatePartialBufferSettingsVsConstraintsLocked(port, *partial_settings,
                                                        stream_constraints)) {
    // This assert is safe only because this thread still holds lock_.  This
    // is asserting that ValidateBufferSettingsVsConstraintsLocked() already
    // called FailLocked().
    ZX_DEBUG_ASSERT(IsStoppingLocked());
    return;
  }

  // Little if any reason to do this outside the lock.
  EnsureBuffersNotConfigured(lock, port, false);

  ZX_DEBUG_ASSERT(active_packets_[port].size() == active_buffers_[port].size());
  if (active_buffers_[port].size() >= kMaxActiveBufferLifetimeOrdinals) {
    // We fail the whole StreamProcessor since this shouldn't happen if the CodecAdapter instance is
    // functioning correctly (at least, for any bitstream format encountered so far). So we want to
    // delete the current CodecAdapter instance since it's likely broken and/or being fed an
    // actively hostile stream.
    FailLocked("Too many active buffer_lifetime_ordinals (SetBufferSettingsCommon)");
    return;
  }

  // This also starts the new buffer_lifetime_ordinal.
  {  // scope port_settings, to enforce not using it after we've moved it out
    std::unique_ptr<PortSettings> port_settings;
    port_settings = std::make_unique<PortSettings>(this, port, std::move(*partial_settings));
    port_settings_[port] = std::move(port_settings);
  }  // ~port_settings, which has been moved out, so we can't use it anyway
  buffer_lifetime_ordinal_[port] = port_settings_[port]->buffer_lifetime_ordinal();
  ZX_DEBUG_ASSERT(active_buffers_[port].find(buffer_lifetime_ordinal) ==
                  active_buffers_[port].end());
  active_buffers_[port].insert(std::make_pair(buffer_lifetime_ordinal, BuffersByIndex{}));

  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token = port_settings_[port]->TakeToken();
  // We intentionally don't want to hand the sysmem token directly to the core
  // codec, at least for now (maybe later it'll be necessary).
  ZX_DEBUG_ASSERT(!port_settings_[port]->partial_settings().has_sysmem_token());
  ZX_DEBUG_ASSERT(!port_settings_[port]->partial_settings().has_sysmem2_token());

  fuchsia_sysmem2::BufferCollectionConstraints buffer_collection_constraints =
      [this, port, &lock, &stream_constraints]() {
        // port_settings_[port] can only change on this thread so are safe to
        // read outside the lock.
        lock.AssertHeld(lock_);
        ScopedUnlock unlock(*this);
        // The constraints returned here won't depend on buffer_constraints_version_ordinal or
        // buffer_lifetime_ordinal, but for output port, can depend on the output requirements of
        // the current position in an active stream.
        auto result = CoreCodecGetBufferCollectionConstraints3(port);
        if (result.has_value()) {
          return std::move(result->constraints);
        }
        // CodecAdapters with IsSupportsDynamicBuffers() true must override
        // CoreCodecGetBufferCollectionConstraints3 and fill out the result's constraints field.
        ZX_DEBUG_ASSERT(!is_supports_dynamic_buffers_);
        // fall back to CoreCodecGetBufferCollectionConstraints2
        return CoreCodecGetBufferCollectionConstraints2(port, stream_constraints,
                                                        port_settings_[port]->partial_settings());
      }();
  // TA analysis doesn't pick up on the fact that we're running a lambda above, so doesn't
  // erroneously complain about lock_ not being held here (as it would without the lambda), but
  // regardless, we want to AssertHeld here to verify that lock_ is held here, for consistency with
  // other usages of ScopedUnlock.
  lock.AssertHeld(lock_);
  // The core codec doesn't fill out usage directly.  Instead we fill it out
  // here.
  if (!FixupBufferCollectionConstraintsLocked(port, &buffer_collection_constraints)) {
    // FixupBufferCollectionConstraints() already called Fail().
    ZX_DEBUG_ASSERT(IsStoppingLocked());
    return;
  }
  // For output, the only reason we re-post here is to share the lock
  // acquisition code with input.
  PostToSharedFidl([this, port, buffer_lifetime_ordinal = buffer_lifetime_ordinal_[port],
                    token = std::move(token),
                    buffer_collection_constraints =
                        std::move(buffer_collection_constraints)]() mutable {
    std::lock_guard<std::mutex> lock(lock_);
    if (buffer_lifetime_ordinal != buffer_lifetime_ordinal_[port]) {
      return;
    }
    if (!sysmem_.is_valid()) {
      return;
    }
    if (IsStoppingLocked()) {
      return;
    }

    auto buffer_collection_request = port_settings_[port]->NewBufferCollectionRequest(
        shared_fidl_dispatcher_,
        [this, port, buffer_lifetime_ordinal](fidl::UnbindInfo unbind_info) {
          std::lock_guard<std::mutex> lock(lock_);
          if (buffer_lifetime_ordinal != buffer_lifetime_ordinal_[port]) {
            // It's fine if a BufferCollection fails after we're already
            // done using it.
            return;
          }
          // We're intentionally picky about the BufferCollection failing
          // too soon, as all clean closes should use Close(), which will
          // avoid causing this.  If we find a case where a client
          // legitimately needs to try one way then if that fails try
          // another way, we should see if we can avoid the need to do
          // that by expressing in sysmem constraints, or more likely just
          // accept that such a client will need to start with a new codec
          // instance for the 2nd try.
          UnbindLocked();
        });
    fuchsia_sysmem2::AllocatorBindSharedCollectionRequest bind_request;
    bind_request.token() = std::move(token);
    bind_request.buffer_collection_request() = std::move(buffer_collection_request);
    auto bind_result = sysmem_->BindSharedCollection(std::move(bind_request));
    // one-way message; Allocator disconnection is fatal
    ZX_ASSERT(bind_result.is_ok());

    std::string buffer_name = GetBufferName(port);

    auto& buffer_collection = port_settings_[port]->buffer_collection();

    fuchsia_sysmem2::NodeSetNameRequest set_name_request;
    set_name_request.name() = std::move(buffer_name);
    set_name_request.priority() = 11;
    // We can ignore one-way send failure because the error handler for ZX_CHANNEL_PEER_CLOSED.
    (void)(*buffer_collection)->SetName(std::move(set_name_request));

    fuchsia_sysmem2::NodeSetDebugClientInfoRequest set_client_info_request;
    set_client_info_request.name() = codec_adapter_->CoreCodecGetName();
    set_client_info_request.id() = 0;
    // We can ignore one-way send failure because the error handler for ZX_CHANNEL_PEER_CLOSED.
    (void)(*buffer_collection)->SetDebugClientInfo(std::move(set_client_info_request));

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.constraints() = std::move(buffer_collection_constraints);
    VLOGF(
        "set_constraints_request.constraints().min_buffer_count: %u min_buffer_count_for_camping: %u",
        *set_constraints_request.constraints()->min_buffer_count(),
        *set_constraints_request.constraints()->min_buffer_count_for_camping());
    // We can ignore one-way send failure because the error handler for ZX_CHANNEL_PEER_CLOSED.
    (void)(*buffer_collection)->SetConstraints(std::move(set_constraints_request));

    (*buffer_collection)
        ->WaitForAllBuffersAllocated()
        .Then([this, port, buffer_lifetime_ordinal](
                  fidl::Result<fuchsia_sysmem2::BufferCollection::WaitForAllBuffersAllocated>&
                      result) mutable {
          zx_status_t status;
          fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info;
          if (result.is_error()) {
            if (result.error_value().is_framework_error()) {
              status = result.error_value().framework_error().status();
            } else {
              ZX_DEBUG_ASSERT(result.error_value().is_domain_error());
              status = sysmem::V1CopyFromV2Error(result.error_value().domain_error());
            }
          } else {
            ZX_DEBUG_ASSERT(result.is_ok());
            status = ZX_OK;
            buffer_collection_info = std::move(result->buffer_collection_info().value());
          }
          OnBufferCollectionInfo(port, buffer_lifetime_ordinal, status,
                                 std::move(buffer_collection_info));
        });
  });
}

void CodecImpl::OnBufferCollectionInfo(
    CodecPort port, uint64_t buffer_lifetime_ordinal, zx_status_t status,
    fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info) {
  ZX_DEBUG_ASSERT(IsFidl());

  if (port == kInputPort) {
    PostSysmemCompletion([this, port, buffer_lifetime_ordinal, status,
                          buffer_collection_info = std::move(buffer_collection_info)]() mutable {
      OnBufferCollectionInfoInternal(port, buffer_lifetime_ordinal, status,
                                     std::move(buffer_collection_info));
    });
  } else {
    ZX_DEBUG_ASSERT(port == kOutputPort);
    OnBufferCollectionInfoInternal(port, buffer_lifetime_ordinal, status,
                                   std::move(buffer_collection_info));
  }
}

void CodecImpl::OnBufferCollectionInfoInternal(
    CodecPort port, uint64_t buffer_lifetime_ordinal, zx_status_t allocate_status,
    fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());

  if (port == kOutputPort) {
    LogEvent(
        media_metrics::
            StreamProcessorEvents2MigratedMetricDimensionEvent_OutputBufferAllocationCompleted);
  } else {
    LogEvent(media_metrics::
                 StreamProcessorEvents2MigratedMetricDimensionEvent_InputBufferAllocationCompleted);
  }

  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }

    // The buffer_lifetime_ordinal_[port] can only change on the current thread.
    if (buffer_lifetime_ordinal != buffer_lifetime_ordinal_[port]) {
      // stale response
      return;
    }
    if (allocate_status != ZX_OK) {
      if (port == kOutputPort) {
        LogEvent(
            media_metrics::
                StreamProcessorEvents2MigratedMetricDimensionEvent_OutputBufferAllocationFailure);
      } else {
        LogEvent(
            media_metrics::
                StreamProcessorEvents2MigratedMetricDimensionEvent_InputBufferAllocationFailure);
      }
      FailLocked(
          "OnBufferCollectionInfoLocked() sees failure - port: %d "
          "allocate_status: %d",
          port, allocate_status);
      return;
    }
  }  // ~lock

  if (buffer_collection_info.buffers()->size() > std::numeric_limits<uint32_t>::max()) {
    // buffer_index is uint32_t
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_AllocationError);
    Fail("buffer_collection_info.buffers()->size() > std::numeric_limits<uint32_t>::max()");
    return;
  }
  uint32_t buffer_count = static_cast<uint32_t>(buffer_collection_info.buffers()->size());

  // This code trusts sysmem to really be sysmem and to behave correctly, but
  // doesn't hurt to double-check some things in debug build.
  ZX_DEBUG_ASSERT(buffer_count >= 1);
  ZX_DEBUG_ASSERT(buffer_collection_info.buffers()->at(buffer_count - 1).vmo().has_value());
  ZX_DEBUG_ASSERT(buffer_collection_info.buffers()->at(buffer_count - 1).vmo()->is_valid());

  // Let's move the VMO handles out first, so that the BufferCollectionInfo we send to the core
  // codec doesn't have the VMO handles.  We want the core codec to get its VMO handles via the
  // CodecBuffer*(s) we'll provide shortly below.
  std::vector<zx::vmo> vmos;
  vmos.reserve(buffer_count);
  for (uint32_t i = 0; i < buffer_count; ++i) {
    vmos.emplace_back(TakeOptionalValue(buffer_collection_info.buffers()->at(i).vmo()));
    ZX_DEBUG_ASSERT(!buffer_collection_info.buffers()->at(i).vmo().has_value());
  }
  ZX_DEBUG_ASSERT(vmos.size() == buffer_count);

  // When is_supports_dynamic_buffers_, we temporarily remove VmoBuffers for call to
  // CoreCodecSetBufferCollectionInfo, for consistency with dynamic buffers case. We don't want
  // is_supports_dynamic_buffers_ true CodecAdapter(s) to vary their behavior depending on whether
  // dynamic buffers are currently being used. Not making it obvious to the CodecAdapter whether
  // *is_dynamic_buffers_[port] helps with that.
  std::vector<fuchsia_sysmem2::VmoBuffer> vmo_buffers;
  if (is_supports_dynamic_buffers()) {
    vmo_buffers = std::move(*buffer_collection_info.buffers());
    buffer_collection_info.buffers().reset();
  }
  // Now we can tell the core codec about the collection info.  The core codec
  // can clone the FIDL struct if it wants, or can just copy out any info it
  // wants from specific fields.
  CoreCodecSetBufferCollectionInfo(port, buffer_collection_info);
  if (is_supports_dynamic_buffers()) {
    buffer_collection_info.buffers().emplace(std::move(vmo_buffers));
  }

  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);

    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal == buffer_lifetime_ordinal_[port]);

    // The only way port_settings_[port] gets cleared is if
    // buffer_lifetime_ordinal changes.
    ZX_DEBUG_ASSERT(port_settings_[port]);

    // This completes the settings, analogous to having completed
    // SetInputBufferSettings()/SetOutputBufferSettings().
    port_settings_[port]->SetBufferCollectionInfo(std::move(buffer_collection_info));
  }

  if (IsPortSecureRequired(port) && !port_settings_[port]->is_secure()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("IsPortSecureRequired(port) && !port_settings_[port]->is_secure() - port: %d", port);
    return;
  }
  if (!IsPortSecurePermitted(port) && port_settings_[port]->is_secure()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("!IsPortSecurePermitted(port) && port_settings_[port]->is_secure() - port: %d", port);
    return;
  }

  auto& fake_map_ranges_by_ordinal = fake_map_range_[port];
  auto fake_map_ranges_by_ordinal_iter = fake_map_ranges_by_ordinal.find(buffer_lifetime_ordinal);
  ZX_DEBUG_ASSERT(fake_map_ranges_by_ordinal_iter == fake_map_ranges_by_ordinal.end());
  if (port_settings_[port]->is_secure()) {
    if (IsCoreCodecMappedBufferUseful(port)) {
      std::optional<FakeMapRange> new_fake_map_range;
      zx_status_t status =
          FakeMapRange::Create(port_settings_[port]->vmo_usable_size(), &new_fake_map_range);
      if (status != ZX_OK) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_InitializationError);
        Fail("FakeMapRange::Init() failed");
        return;
      }
      fake_map_range_[port].emplace(
          buffer_lifetime_ordinal,
          std::unique_ptr<FakeMapRange>(new FakeMapRange(std::move(*new_fake_map_range))));
    }
  }

  // We convert the buffer_collection_info into AddInputBuffer_StreamControl()
  // and AddOutputBufferInternal() calls, almost as if the client were adding
  // the buffers itself (but without the check that the client isn't adding
  // buffers itself while using sysmem).
  for (uint32_t i = 0; i < buffer_count; i++) {
    // While under the lock we'll move out the stuff we need into locals
    uint64_t vmo_usable_start = 0;
    uint64_t vmo_usable_size = 0;
    bool is_secure = false;
    {  // scope lock
      std::lock_guard<std::mutex> lock(lock_);

      ZX_DEBUG_ASSERT(buffer_lifetime_ordinal == buffer_lifetime_ordinal_[port]);
      ZX_DEBUG_ASSERT(port_settings_[port]);

      vmo_usable_start = port_settings_[port]->vmo_usable_start(i);
      vmo_usable_size = port_settings_[port]->vmo_usable_size();
      is_secure = port_settings_[port]->is_secure();
    }  // ~lock

    CodecBuffer::Info buffer_info{.port = port,
                                  .lifetime_ordinal = buffer_lifetime_ordinal,
                                  .index = i,
                                  .is_secure = is_secure};
    CodecVmoRange vmo_range(std::move(vmos[i]), vmo_usable_start, vmo_usable_size);
    if (port == kInputPort) {
      AddInputBuffer_StreamControl(std::move(buffer_info), std::move(vmo_range));
    } else {
      ZX_DEBUG_ASSERT(port == kOutputPort);
      AddOutputBufferInternal(std::move(buffer_info), std::move(vmo_range));
    }
  }

  if (port == kOutputPort) {
    LogEvent(media_metrics::
                 StreamProcessorEvents2MigratedMetricDimensionEvent_OutputBufferAllocationSuccess);
  } else {
    LogEvent(media_metrics::
                 StreamProcessorEvents2MigratedMetricDimensionEvent_InputBufferAllocationSuccess);
  }
}

CodecImpl::BuffersByIndex* CodecImpl::all_buffers(CodecPort port) {
  if (buffer_lifetime_ordinal_[port] % 2 == 0) {
    return nullptr;
  }
  auto iter = active_buffers_[port].find(buffer_lifetime_ordinal_[port]);
  if (iter == active_buffers_[port].end()) {
    return nullptr;
  }
  return &iter->second;
}

CodecImpl::PacketsByIndex& CodecImpl::all_packets(CodecPort port) {
  // Caller must know/ensure these.
  ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[port] % 2 == 1);
  ZX_DEBUG_ASSERT(active_packets_[port].find(buffer_lifetime_ordinal_[port]) !=
                  active_packets_[port].end());
  return active_packets_[port][buffer_lifetime_ordinal_[port]];
}

uint64_t CodecImpl::current_buffer_count(CodecPort port) {
  uint64_t buffer_lifetime_ordinal = buffer_lifetime_ordinal_[port];
  uint64_t count = 0;
  auto& adding_buffers_by_ordinal = adding_buffers_[port];
  auto adding_buffers_by_ordinal_iter = adding_buffers_by_ordinal.find(buffer_lifetime_ordinal);
  if (adding_buffers_by_ordinal_iter != adding_buffers_by_ordinal.end()) {
    auto& adding_buffers_by_index = adding_buffers_by_ordinal_iter->second;
    count += adding_buffers_by_index.size();
  }
  auto& active_buffers_by_ordinal = active_buffers_[port];
  auto active_buffers_by_ordinal_iter = active_buffers_by_ordinal.find(buffer_lifetime_ordinal);
  if (active_buffers_by_ordinal_iter != active_buffers_by_ordinal.end()) {
    auto& active_buffers_by_index = active_buffers_by_ordinal_iter->second;
    count += active_buffers_by_index.size();
  }
  return count;
}

void CodecImpl::EnsureBuffersNotConfigured(ScopedLock& lock, CodecPort port, bool is_client_gone) {
  VLOGF("EnsureBuffersNotConfigured top");
  // This method can be called on input only if there's no current stream.
  //
  // On output, this method can be called if there's no current stream or if we're in the middle of
  // an output config change.
  //
  // If there's no current stream, the CodecAdapter will quickly drop its duplicated buffer handles,
  // if any, but this may occur async and completion is handled async.
  //
  // If there's a current stream, the CodecAdapter may retain duplicated handles to buffers that are
  // needed to retain the stream processing state, and if specified by the bitstream, may emit those
  // buffers as output later (however, these emitted frames may be dropped by CodecAdapter unless
  // the client has opted in to receiving old output buffers). If IsSupportsDynamicBuffers is true,
  // the CodecAdapter _will_ do this (if/as specified by the bitstream format and specific
  // bitstream).
  //
  // When using dynamic buffers, if a stream is active, "NotConfigured" still allows the
  // CodecAdapter to later output a frame in a buffer with older buffer_lifetime_ordinal which has
  // not yet seen buffer.parent_vmo_ ZX_VMO_ZERO_CHILDREN. For example, when decoding VP9, this can
  // happen if the stream uses "show_existing_frame" to show an old-dimensions frame that's still in
  // VP9's set of 8 reference frames. What "NotConfigured" does is prevent the CodecAdapter from
  // choosing to reuse a buffer with old buffer_lifetime_ordinal, and the CodecImpl stops
  // preventing CodecBuffer.parent_vmo_ from seeing ZX_VMO_ZERO_CHILDREN.
  //
  // After this call, any remaining buffers are pending removal. Some may have already been pending
  // removal prior to this call, in which case buffer.pending_remove_completion_ is already set. A
  // buffer pending removal due to this call can have pending_remove_completion_ set later if the
  // client later uses RemoveBuffer.
  //
  // On input, this can only be called on StreamControl.
  //
  // On output, this can only be called on fidl_thread(), though StreamControl can be waiting on
  // this call to complete before proceeding with a mid-stream buffer reallocation. The output
  // ordering domain (fidl thread) waiting on the StreamControl ordering domain is not permitted.
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  lock.AssertHeld(lock_);

  is_port_configured_[port] = false;
  if (buffer_lifetime_ordinal_[port] % 2 == 1) {
    buffer_lifetime_ordinal_[port]++;
  }
  if (port_settings_[port]) {
    // This will close the BufferCollection (async as-needed) cleanly, without causing the
    // LogicalBufferCollection to fail.  Mainly we care so we can more easily tell during debugging
    // whether a LogicalBufferCollection was cleanly closed by all participants, vs. potentially
    // getting failed by a participant exiting or non-cleanly closing.  A Sync() by the client is
    // sufficient to ensure this async close is done.
    port_settings_[port] = nullptr;
  }
  if (port == kInputPort) {
    free_input_packets_.clear();
  }

  // Inform core codec that the most-recent buffers are now old buffers.
  //
  // If there's an active stream, the CodecAdapter can retain any handles to buffers needed to hold
  // the state of ongoing processing. For example a video decoder can keep old-dimensions reference
  // frames (and potentially later emit such a frame as output if the bitstream format supports that
  // and is_supports_dynamic_buffers_).
  //
  // After this block, the CodecAdapter can still be writing into one or more of these buffers
  // (among those which are not currently outstanding), but only if it retains its own handle to the
  // buffer(s). This is only relevant for call sites where the CodecAdapter's processing isn't
  // paused.
  //
  // The CodecAdapter will no longer consider any of these buffers available for reuse after this
  // block (whether currently outstanding or not). Any buffers currently filling can be output, but
  // will not be filled again. In other words, zero of these buffers are considered "free" for
  // reuse after this block.
  //
  // The rules above allow CodecImpl to decouple the recycling of old packets from clients that
  // haven't sent EnableOldOutputBuffers. In that case, the client won't send any
  // RecycleOutputPacket for old buffer_lifetime_ordinal(s), so CodecImpl recycles the packets back
  // to the CodecAdapter early, before the client is necessarily done reading from the old buffers.
  // By not treating a recycled buffer (referenced by a recycled packet) as "free", the client can
  // continue to read from the buffer despite the packet having been recycled.
  //
  // If !codec_adapter_->IsSupportsDynamicBuffers(), this may cause the CodecAdapter to immediately
  // drop old buffers (or it may not have had any duplicates in the first place) despite any such
  // buffers still being in the reference frame set and potentially needed for decoding later frames
  // or to be a later output frame. Such an immediate dropping of bitstream-needed buffers is
  // neither required nor encouraged when !codec_adapter_->IsSupportsDynamicBuffers(), and this
  // immediate dropping of necessary buffers is only permitted when
  // !codec_adapter_->IsSupportsDynamicBuffers.
  //
  // With only stream-driven buffer reallocation at points where the stream switches dimensions,
  // it's rare that a bitstream actually outputs an old-dimensions buffer from before a dimensions
  // switch. This is the only type of mid-stream buffer reallocation when
  // !is_supports_dynamic_buffers_. However, when is_supports_dynamic_buffers_ true, buffer
  // reallocations can be client-driven and mid-stream. This is the primary reason for the strict
  // requirement that is_supports_dynamic_buffers_ true CodecAdapter(s) retain buffers necessary
  // for current stream state; those buffers can be same-dimensions and at any arbitrary point in
  // the stream, so not retaining buffers necessary for stream state would impact all streams, not
  // just rare streams.
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  auto& port_buffers = active_buffers_[port];
  // The kMaxActiveBufferLifetimeOrdinals limit is among the reasons why sweeping all buffers here
  // is fine, even for buffer_lifetime_ordinal(s) with all buffers already having is_remove_pending_
  // true.
  //
  // An even buffer_lifetime_ordinal can't have buffers.
  ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[port] % 2 == 0);
  for (auto& buffers_entry : port_buffers) {
    auto buffer_lifetime_ordinal = buffers_entry.first;
    auto& buffers_by_index = buffers_entry.second;
    for (auto& buffers_by_index_entry : buffers_by_index) {
      auto buffer_index = buffers_by_index_entry.first;
      auto& buffer = *buffers_by_index_entry.second;
      ZX_DEBUG_ASSERT(buffer_lifetime_ordinal == buffer.lifetime_ordinal());
      ZX_DEBUG_ASSERT(buffer_index == buffer.index());
      bool was_remove_pending = buffer.is_remove_pending_;
      buffer.is_remove_pending_ = true;
      if (is_supports_dynamic_buffers() && buffer.was_ever_added_to_core_codec() &&
          !was_remove_pending) {
        // Because we're currently running on the only thread that modifies active_buffers_[port],
        // releasing the lock doesn't invalidate iterators.
        ScopedUnlock unlock(*this);
        VLOGF("calling CoreCodecRemoveBuffer - port: %u buffer_lifetime_ordinal: %" PRIu64
              " buffer_index: %u buffer: %p",
              buffer.port(), buffer.lifetime_ordinal(), buffer.index(), &buffer);
        CoreCodecRemoveBuffer(port, &buffer);
      }
    }
  }
  if (!is_supports_dynamic_buffers()) {
    ScopedUnlock unlock(*this);
    CoreCodecEnsureBuffersNotConfigured(port);
  }
  lock.AssertHeld(lock_);

  for (auto& buffers_entry : port_buffers) {
    auto& buffers_by_index = buffers_entry.second;
    for (auto& buffers_by_index_entry : buffers_by_index) {
      auto& buffer = *buffers_by_index_entry.second;
      ZX_DEBUG_ASSERT(buffer.is_remove_pending_);
      // Drop the keep-alive child handle under parent_vmo_ - this allows ZX_VMO_ZERO_CHILDREN to
      // trigger as soon as there are no more handles to child VMOs held by the CodecAdapter.
      if (buffer.until_remove_started_child_vmo_) {
        zx_info_handle_count_t handle_count;
        zx_status_t status = buffer.until_remove_started_child_vmo_->get_info(
            ZX_INFO_HANDLE_COUNT, &handle_count, sizeof(handle_count), nullptr, nullptr);
        ZX_ASSERT(status == ZX_OK);

        VLOGF("until_remove_started_child_vmo_.reset() - port: %u buffer: %p", buffer.port(),
              &buffer);
        buffer.until_remove_started_child_vmo_.reset();
      }
    }
  }

  // When is_client_gone || !is_enable_old_output_buffers_, the client will never be recycling
  // packets of any old buffer_lifetime_ordinal. However, when !is_client_gone &&
  // is_enable_old_output_buffers_, the client is still the one responsible for recycling packets,
  // even packets of old buffer_lifetime_ordinal(s). It works this way to prevent a client that
  // can handle packets of old buffer_lifetime_ordinal from seeing colliding packet_index(s) emitted
  // on output, while still being able to recycle all packets when the client is gone.
  if (port == kOutputPort && is_supports_dynamic_buffers() &&
      (is_client_gone || !is_enable_old_output_buffers_)) {
    ZX_DEBUG_ASSERT(port == kOutputPort && IsFidl());
    std::vector<CodecPacket*> recycle_outside_lock;
    auto& port_packets = active_packets_[port];
    for (auto& packets_entry : port_packets) {
      auto buffer_lifetime_ordinal = packets_entry.first;
      auto& packets_by_index = packets_entry.second;
      for (auto& packet_ptr : packets_by_index) {
        auto* packet = packet_ptr.get();
        ZX_DEBUG_ASSERT(buffer_lifetime_ordinal == packet->buffer_lifetime_ordinal());
        if (!packet->is_free()) {
          --packet->buffer()->output_in_flight_count_;
          packet->SetFree(true);
          recycle_outside_lock.push_back(packet);
        }
      }
    }
    // Before dropping lock_, we've updated buffer_lifetime_ordinal_[port] above such that
    // StreamProcessor.RecycleOutputPacket handler will avoid recycling any old packet if
    // !is_enable_old_output_buffers_[port].
    //
    // We avoid calling SetFree(true) outside the lock since it's also called elsewhere from a
    // different thread.
    //
    // The current thread is the only thread that modifiles active_packets_[port].
    if (!recycle_outside_lock.empty()) {
      ScopedUnlock unlock(*this);
      for (auto* packet : recycle_outside_lock) {
        // We can safely assert that is_free() is still true here outside lock_. The overall
        // complete list of reasons why this is safe to assert here isn't short, but to summarize,
        // we know that (a) this thread set is_free to true above, (b) we know no other thread
        // will set is_free to false until after CoreCodecRecycleOutputPacket given correct
        // CodecAdapter behavior, and (c) we know we're on the only thread that ever modifies
        // active_packets_[port]. This is difficult to express with __TA_GUARDED annotations and
        // adding more of those (hopefully soon) should take priority over keeping this assert.
        ZX_DEBUG_ASSERT(packet->is_free());
        // This particular CoreCodecRecycleOutputPacket is only done when
        // is_supports_dynamic_buffers_.
        ZX_DEBUG_ASSERT(is_supports_dynamic_buffers());
        VLOGF(
            "EnsureBuffersNotConfigured calling CoreCodecRecycleOutputPacket port: %u packet ptr: %p index: %u buffer ptr: %p index: %u",
            port, packet, packet->packet_index(), packet->buffer(), packet->buffer()->index());
        CoreCodecRecycleOutputPacket(packet);
      }
    }
    lock.AssertHeld(lock_);
  }

  auto next_iter = port_buffers.begin();
  for (auto iter = port_buffers.begin(); iter != port_buffers.end(); iter = next_iter) {
    next_iter = iter;
    ++next_iter;
    auto old_buffer_lifetime_ordinal = iter->first;
    auto delete_outside_lock = MaybeDeleteBufferLifetimeOrdinal(port, old_buffer_lifetime_ordinal);
    if (delete_outside_lock.has_value()) {
      ScopedUnlock lock(*this);
      CoreCodecCloseBufferLifetimeOrdinal(port, old_buffer_lifetime_ordinal);
      // At this point the CodecAdapter is no longer tracking the old buffer_lifetime_ordinal at
      // all, so it's now safe to delete all the packets under the old buffer_lifetime_ordinal,
      // without the CodecAdapter ever having any CodecPacket pointers that are invalid, even
      // transiently.
      delete_outside_lock.reset();
    }
  }
}

bool CodecImpl::ValidatePartialBufferSettingsVsConstraintsLocked(
    CodecPort port, const fuchsia::media::StreamBufferPartialSettings& partial_settings,
    const fuchsia::media::StreamBufferConstraints& constraints) {
  // Most of the constraints will be handled by telling sysmem about them, not
  // via the client, so there's not a ton to validate here.
  if (partial_settings.has_single_buffer_mode()) {
    if (partial_settings.single_buffer_mode()) {
      LogEvent(media_metrics::
                   StreamProcessorEvents2MigratedMetricDimensionEvent_ClientConstraintsFailure);
      FailLocked("single_buffer_mode (deprecated; obsolete)");
      return false;
    } else {
      LOG(WARN,
          "has_single_buffer_mode() (set to false) seen - client should stop setting this, even to "
          "false (deprecated; obsolete)");
    }
  }
  ZX_DEBUG_ASSERT(partial_settings.sysmem2_token().is_valid());
  return true;
}

// A correctly-operating client will only send this message if
// DetailedCodecDescription.supports_dynamic_buffers was set to true.
void CodecImpl::AddBuffer(fuchsia::media::StreamProcessorAddBufferRequest request) {
  ZX_DEBUG_ASSERT(IsFidl());
  if (!is_supports_dynamic_buffers()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("client sent AddBuffer when !supports_dynamic_buffers");
    return;
  }
  ZX_ASSERT(::codec_impl::internal::kEnableDynamicBuffers);
  // We also re-check in AddBufferInternal after acquiring lock_.
  if (IsStopping()) {
    return;
  }
  is_force_output_buffers_fixed_image_size_message_permitted_ = false;
  if (!request.has_port()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("AddBuffer port must be set");
    return;
  }
  auto maybe_codec_port = CodecPortFromFidlPort(request.port());
  if (!maybe_codec_port.has_value()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("AddBuffer port unrecognized - port: %u", request.port());
    return;
  }
  CodecPort port = *maybe_codec_port;
  if (!request.has_buffer_constraints_version_ordinal()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("AddBuffer buffer_constraints_version_ordinal must be set");
    return;
  }
  uint64_t buffer_constraints_version_ordinal = request.buffer_constraints_version_ordinal();
  if (!request.has_buffer_lifetime_ordinal()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("AddBuffer must have buffer_lifetime_ordinal set");
    return;
  }
  uint64_t buffer_lifetime_ordinal = request.buffer_lifetime_ordinal();
  if (buffer_lifetime_ordinal % 2 == 0) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("AddBuffer buffer_lifetime_ordinal values must be odd");
    return;
  }
  if (!request.has_buffer_index()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("AddBuffer buffer_index must be set");
    return;
  }
  uint32_t buffer_index = request.buffer_index();
  if (!request.has_buffer()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("AddBuffer buffer must be set");
    return;
  }
  if (!request.buffer().is_valid()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    Fail("AddBuffer buffer must be valid handle");
    return;
  }
  zx::vmo buffer = std::move(*request.mutable_buffer());
  ZX_DEBUG_ASSERT(port == kInputPort || port == kOutputPort);
  if (port == kInputPort) {
    PostToStreamControl([this, port, buffer_constraints_version_ordinal, buffer_lifetime_ordinal,
                         buffer_index, buffer = std::move(buffer)]() mutable {
      AddBufferInternal(port, buffer_constraints_version_ordinal, buffer_lifetime_ordinal,
                        buffer_index, std::move(buffer));
    });
  } else {
    ZX_DEBUG_ASSERT(port == kOutputPort);
    AddBufferInternal(port, buffer_constraints_version_ordinal, buffer_lifetime_ordinal,
                      buffer_index, std::move(buffer));
  }
}

void CodecImpl::AddBufferInternal(CodecPort port, uint64_t buffer_constraints_version_ordinal,
                                  uint64_t buffer_lifetime_ordinal, uint32_t buffer_index,
                                  zx::vmo unverified_buffer_vmo) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());

  // We duplicate before acquiring the lock so that the bottom of the lock hold interval knows we'll
  // be sending GetVmoInfo.
  zx::vmo dup_vmo;
  zx_status_t dup_status = unverified_buffer_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_vmo);
  if (dup_status != ZX_OK) {
    // This is more likely than not to be somehow caused by the client, but let's not assume
    // it's a client protocol error. Still, we can't continue if we can't dup the handle.
    Fail("AddBuffer buffer handle failed to duplicate");
    return;
  }

  std::optional<zx::vmo> match_existing_vmo;
  std::shared_ptr<AddingBuffer> adding_buffer;
  CodecPacket* output_packet_to_recycle = nullptr;
  std::optional<fuchsia_sysmem2::BufferCollectionConstraints> constraints_to_check;
  bool is_wake_stream_control_condition_needed = false;
  {  // scope lock
    ScopedLock lock(lock_);
    if (IsStoppingLocked()) {
      return;
    }

    // We track the most recent buffer_lifetime_ordinal received from the client (for protocol
    // enforcement) regardless of whether the associated attempt to start that new
    // buffer_lifetime_ordinal was applied, or ignored due to the client having stale info.
    ZX_DEBUG_ASSERT(
        (buffer_lifetime_ordinal_[port] % 2 == 1 &&
         protocol_buffer_lifetime_ordinal_[port] >= buffer_lifetime_ordinal_[port]) ||
        (buffer_lifetime_ordinal_[port] % 2 == 0 &&
         protocol_buffer_lifetime_ordinal_[port] + 1 >= buffer_lifetime_ordinal_[port]));
    if (buffer_lifetime_ordinal < protocol_buffer_lifetime_ordinal_[port]) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
      // Adding buffers to an old buffer_lifetime_ordinal isn't supported. So far we don't know of
      // any reason to support this.
      FailLocked("AddBuffer buffer_lifetime_ordinal can't go backward");
      return;
    }
    protocol_buffer_lifetime_ordinal_[port] = buffer_lifetime_ordinal;

    // We bump the buffer_lifetime_ordinal_ up by 1 (to an even value) when the core codec indicates
    // it needs new output buffers, but the old port_settings_ remain in place until the client
    // takes action. Only odd buffer_lifetime_ordinal values are sent by clients (enforced by the
    // caller of this method). An even value in buffer_lifetime_ordinal_[port] means that the prior
    // buffer_lifetime_ordinal value from the client is no longer current (despite the client not
    // necessarily knowing that yet) and will need to be replaced by the client, but there isn't yet
    // a new buffer_lifetime_ordinal from the client that was successfully configured (that had
    // sufficiently-fresh buffer_constraints_version_ordinal, etc).
    ZX_DEBUG_ASSERT(
        !port_settings_[port] ||
        (buffer_lifetime_ordinal_[port] >= port_settings_[port]->buffer_lifetime_ordinal() &&
         buffer_lifetime_ordinal_[port] <= port_settings_[port]->buffer_lifetime_ordinal() + 1));

    if (is_dynamic_buffers_[port].has_value() && !*is_dynamic_buffers_[port]) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
      FailLocked(
          "AddBuffer: cannot mix non-dynamic buffers and dynamic buffers (per port and stream); consider CloseCurrentStream with param(s) true first - port: %u",
          port);
      return;
    }
    ZX_DEBUG_ASSERT(!is_dynamic_buffers_[port].has_value() || *is_dynamic_buffers_[port]);
    is_dynamic_buffers_[port] = true;

    if (buffer_constraints_version_ordinal > sent_buffer_constraints_version_ordinal_[port]) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
      FailLocked("client sent AddBuffer with too-new buffer_constraints_version_ordinal");
      return;
    }
    if (buffer_constraints_version_ordinal <
        last_required_buffer_constraints_version_ordinal_[port]) {
      // Ignore the buffer under stale buffer_constraints_version_ordinal; the core codec doesn't
      // want this buffer; the client should catch up to at least the last required value, using a
      // new buffer_lifetime_ordinal started with the new
      // last_required_buffer_constraints_version_ordinal_[port] specified by the client.
      //
      // This is particularly important if a video decoder is trying to allocate new output buffers
      // for a new stream that has different bit depth or similar.
      LOG(DEBUG, "buffer_constraints_version_ordinal stale; dropping");
      return;
    }
    // We've peeled off too new and too old above.
    ZX_DEBUG_ASSERT(buffer_constraints_version_ordinal >=
                        last_required_buffer_constraints_version_ordinal_[port] &&
                    buffer_constraints_version_ordinal <=
                        sent_buffer_constraints_version_ordinal_[port]);

    ZX_DEBUG_ASSERT(codec_adapter_);
    if (!is_supports_dynamic_buffers()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
      FailLocked("client sent AddBuffer when !supports_dynamic_buffers");
      return;
    }
    ZX_ASSERT(::codec_impl::internal::kEnableDynamicBuffers);

    if (buffer_lifetime_ordinal < buffer_lifetime_ordinal_[port]) {
      // At least currently, we can treat this as a protocol error, given the previous checks
      // already performed before we get here.
      //
      // For the similar check in SetBufferSettingsCommon, we just return (we ignore the client's
      // questionable message and let the client "catch up"). If it weren't for the risk of breaking
      // some unknown client, we'd make the similar check there a client protocol error also.
      //
      // TODO(b/527322674): Consider updating SetBufferSettingsCommon to also reject this, in a
      // separate CL. And remove/fixup previous paragraph.
      //
      // There is a prior check against protocol_buffer_lifetime_ordinal_[port] ensuring that the
      // client isn't actually trying to (incorrectly) go backward. The only way
      // buffer_lifetime_ordinal_[port] is greater than a non-backward buffer_lifetime_ordinal from
      // the client is when we've bumped buffer_lifetime_ordinal_[port] to the next even value (all
      // client values must be odd) to indicate that the most recent buffer_lifetime_ordinal from
      // the client is stale. We do this bump in a few places; these fall into the following two
      // categories:
      //   * The client caused the old buffers to be released. The client knows that the client's
      //     most recent buffer_lifetime_ordinal is no longer valid. In this case the client
      //     shouldn't be trying to add more buffers to a buffer_lifetime_ordinal that the client
      //     already ended.
      //   * The server caused the old buffer_lifetime_ordinal to become stale, and the server also
      //     caused the old buffer_constraints_version_ordinal to become stale, both under the same
      //     lock hold interval. The client finds out about both of these becoming stale in a single
      //     OnOutputConstraints message with buffer_constraints_action_required true. The client
      //     might not know about that message yet, in which case a correctly-operating client would
      //     have triggered the check above re. too-old buffer_constraints_version_ordinal and this
      //     method would have returned already above (so the current check wouldn't run). If the
      //     client does know, the client knows about both being stale and the client shouldn't be
      //     specifying a new-enough buffer_constraints_version_ordinal but too-old
      //     buffer_lifetime_ordinal.
      //
      // In contrast, we don't treat new buffer_lifetime_ordinal but old
      // buffer_constraints_version_ordinal as a protocol error because it's permitted for a client
      // to unilaterally bump the buffer_lifetime_ordinal to the next odd value and move on to a new
      // stream without the client knowing yet about an old still-in-flight OnOutputConstraints with
      // old stream_lifetime_ordinal and buffer_constraints_action_required true. In that case the
      // client is allowed to ignore the OnOutputConstraints with stale stream_lifetime_ordinal when
      // it finally arrives, and if the server can't (or doesn't want to) use the current buffers
      // for the new stream, the server will send another OnOutputConstraints with
      // buffer_constraints_action_required true and an even newer
      // buffer_constraints_version_ordinal.
      //
      // Currently this can only happen on the output port; the input port currently always has
      // buffer_lifetime_ordinal_[port] equal to 0.
      ZX_DEBUG_ASSERT(kOutputPort);
      ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[port] % 2 == 0);
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
      // This can happen if the client made the old buffer_lifetime_ordinal stale but then used the
      // old buffer_lifetime_ordinal again, or if the server made both buffer_lifetime_ordinal and
      // buffer_constraints_version_ordinal stale in a single OnOutputConstraints message with
      // buffer_constraints_action_required true, but the client kept using the old
      // buffer_lifetime_ordinal despite using the new buffer_constraints_version_ordinal. In either
      // case, this is a protocol error. In contrast, if the client tries to use the old
      // buffer_constraints_version_ordinal and old buffer_lifetime_ordinal, that's fine; in that
      // case the client will catch up shortly.
      FailLocked(
          "AddBuffer with new-enough buffer_constraints_version_ordinal but too-old buffer_lifetime_ordinal");
      return;
    }

    // We've already checked above that the buffer_lifetime_ordinal is in sequence.
    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal >= buffer_lifetime_ordinal_[port]);

    if (buffer_lifetime_ordinal > buffer_lifetime_ordinal_[port]) {
      // We intentionally allow a client to create a new buffer_lifetime_ordinal at any time
      // including mid-stream with active stream processing ongoing. For example a client may need
      // to allocate new buffers to accommodate a new sysmem participant known to the client that
      // has dynamically arrived (or perhaps stop accommodating a sysmem participant that has
      // departed), without stopping the current stream (for example, without forcing a
      // re-seek/re-cue of a logical video that's currently playing).
      //
      // It's the CodecAdapter's responsibility to ensure that the latest stream position
      // trigger(ed/s) an onCoreCodecMidStreamOutputConstraintsChange(true) if the added buffer is
      // not suitable for continued decode at the latest stream position.
      //
      // CoreCodecGetBufferCollectionConstraints2 does take into account any active stream at the
      // time of that call, but that call occurred a while back, and an ongoing stream may have
      // changed it's implied constraints since then by the time the CoreCodecAddBuffer is attempted
      // below.
      //
      // For this reason, we don't call CoreCodecGetBufferCollectionConstraints2 again here or
      // verify that the buffer being added conforms to the current
      // CoreCodecGetBufferCollectionConstraints2. That would only create confusion since the adding
      // buffer isn't required to conform to the current CoreCodecGetBufferCollectionConstraints2
      // (not an error, no reason to compute that it doesn't if it doesn't).
      //
      // However, we do verify and enforce here that all concurrently-existing buffers of the
      // buffer_lifetime_ordinal have the same SingleBufferSettings. The client is free to achieve
      // that a number of different ways. If SingleBufferSettings doesn't match (among
      // concurrently-present buffers of a given buffer_lifetime_ordinal), that intentionally
      // becomes a codec failure not just a stream failure, because added buffers are not
      // stream-specific.
      //
      // start a new buffer_lifetime_ordinal

      // This ensures all pre-existing buffers of the port are at least pending removal (or
      // removed). The CodecAdapter will retain handles/mappings/pins of buffers derived from
      // CodecBuffer.GetChildVmo for any buffers needed to retain any ongoing stream processing
      // state. Output of packets referencing said buffers remains possible until the CodecAdapter
      // has closed/ended all such handles/mappings/pins.
      EnsureBuffersNotConfigured(lock, port, false);

      // This starts the new buffer_lifetime_ordinal.
      port_settings_[port] = std::make_unique<PortSettings>(
          this, port, buffer_constraints_version_ordinal, buffer_lifetime_ordinal);
      ZX_DEBUG_ASSERT(buffer_lifetime_ordinal == port_settings_[port]->buffer_lifetime_ordinal());
      buffer_lifetime_ordinal_[port] = buffer_lifetime_ordinal;
      ZX_DEBUG_ASSERT(adding_buffers_[port].find(buffer_lifetime_ordinal) ==
                      adding_buffers_[port].end());
      ZX_DEBUG_ASSERT(active_buffers_[port].find(buffer_lifetime_ordinal) ==
                      active_buffers_[port].end());
      // For dynamic buffer mode, this just means we've seen the new buffer_lifetime_ordinal from
      // the client. It doesn't mean we've called CoreCodecSetBufferCollectionInfo yet, and it
      // doesn't mean we've called CoreCodecAddBuffer yet. Those have to wait until we have the
      // GetVmoInfo response, and we want to allow all the GetVmoInfo calls for the new set of
      // buffers to be in flight concurrently, to avoid extra switching back and forth between this
      // process and sysmem). In the case of output, the CodecAdapter can still be processing into
      // an old buffer previously selected from the CodecAdapter's free buffers list, before
      // EnsureBuffersNotConfigured was called above to empty the free buffers list.
      is_port_configured_[port] = true;
      is_wake_stream_control_condition_needed = true;
    }

    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal == buffer_lifetime_ordinal_[port]);

    uint64_t buffer_count = current_buffer_count(port);
    if (buffer_count >= dynamic_buffers_max_[port]) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked("AddBuffer when already at max - port: %u current_buffer_count: %" PRId64
                 " max: %" PRId64,
                 port, buffer_count, dynamic_buffers_max_[port]);
      return;
    }

    auto& adding_buffers_by_ordinal = adding_buffers_[port];
    auto adding_buffers_by_ordinal_iter = adding_buffers_by_ordinal.find(buffer_lifetime_ordinal);
    if (adding_buffers_by_ordinal_iter != adding_buffers_by_ordinal.end()) {
      auto& adding_buffers_by_index = adding_buffers_by_ordinal_iter->second;
      auto adding_buffers_by_index_iter = adding_buffers_by_index.find(buffer_index);
      if (adding_buffers_by_index_iter != adding_buffers_by_index.end()) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
        FailLocked(
            "AddBuffer found already-adding buffer - port: %lu buffer_lifetime_ordinal: %" PRId64
            " buffer_index: %u",
            port, buffer_lifetime_ordinal, buffer_index);
        return;
      }
    }

    auto& active_buffers_by_ordinal = active_buffers_[port];
    auto active_buffers_by_ordinal_iter = active_buffers_by_ordinal.find(buffer_lifetime_ordinal);
    if (active_buffers_by_ordinal_iter != active_buffers_by_ordinal.end()) {
      auto& active_buffers_by_index = active_buffers_by_ordinal_iter->second;
      auto active_buffers_by_index_iter = active_buffers_by_index.find(buffer_index);
      if (active_buffers_by_index_iter != active_buffers_by_index.end()) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
        FailLocked(
            "AddBuffer found already-active buffer - port: %lu buffer_lifetime_ordinal: %" PRId64
            " buffer_index: %u",
            port, buffer_lifetime_ordinal, buffer_index);
        return;
      }
    }

    // We intentionally allow the SingleBufferSettings to change if RemoveBuffer completes removal
    // of all old buffers before AddBuffer adds a new buffer to the same buffer_lifetime_ordinal.
    //
    // Because the CodecAdapter can be actively processing a stream at this point, this can pick up
    // newer constraints than the buffer_constraints_version_ordinal constraints, which can lead to
    // sysmem indicating that the unverified_buffer_vmo doesn't conform to the newer constraints
    // retrieved here despite the buffer_constraints_version_ordinal check above. See error handling
    // below in completion of GetVmoInfo for how we handle this.
    uint64_t constraints_version = 0;
    auto constraints_to_check_or_fail = GetBufferConstraintsForDynamic(
        lock, port, buffer_constraints_version_ordinal, true, &constraints_version);
    if (!constraints_to_check_or_fail.has_value()) {
      // GetBufferConstraintsForDynamic already logged and failed
      ZX_DEBUG_ASSERT(IsStoppingLocked());
      return;
    }
    constraints_to_check = TakeOptionalValue(constraints_to_check_or_fail);

    match_existing_vmo = TryGetMatchExistingVmo(port, buffer_lifetime_ordinal);

    // We know there isn't already an entry per check above.
    adding_buffer = std::make_shared<AddingBuffer>(std::move(unverified_buffer_vmo));
    // intentional clone of the shared_ptr; used again later
    adding_buffers_[port][buffer_lifetime_ordinal][buffer_index] = adding_buffer;

    // For input, this also ensures we have an entry for buffer_lifetime_ordinal in active_packets_,
    // so we can check for that entry when an input packet arrives even if GetVmoInfo (started
    // below) hasn't completed yet. If GetVmoInfo succeeds, this gets cleaned up when the
    // buffer_lifetime_ordinal is closed out. If GetVmoInfo fails, this gets cleaned up with
    // CodecImpl gets deleted shortly after that.
    //
    // For both input and output, adding the packet before the buffer allows CodecAdapter to know
    // (per port and buffer_lifetime_ordinal) that the count of packets will always be greater than
    // or equal to the number of buffers.
    auto& packets_by_index = active_packets_[port][buffer_lifetime_ordinal];
    if (current_buffer_count(port) > packets_by_index.size()) {
      ZX_DEBUG_ASSERT(packets_by_index.size() + 1 == current_buffer_count(port));
      uint32_t packet_index = static_cast<uint32_t>(packets_by_index.size());
      auto* new_packet = packets_by_index
                             .emplace_back(std::unique_ptr<CodecPacket>(
                                 new CodecPacket(buffer_lifetime_ordinal, packet_index)))
                             .get();
      // For input we just need to have sufficient free packets to allow the client to have as many
      // packets in flight as there are buffers (per port and buffer_lifetime_ordinal). The new
      // input packet starts with the client implicitly.
      //
      // For output packets, we need to "recycle" the new packet to the CodecAdapter. We can go
      // ahead and do that now; no need to track "adding" packets.
      if (port == kOutputPort) {
        // We "recycle" the new packet before adding the new buffer. This is consistent with the
        // ordering when !*is_dynamic_buffers_ (but only because we check
        // is_supports_dynamic_buffers_ there).
        //
        // We don't remove any packets until the entire buffer_lifetime_ordinal is cleaned up, after
        // all buffers under the buffer_lifetime_ordinal are gone, or until CodecImpl is deleted.
        //
        // This ordering means that CodecAdapter(s) (with is_supports_dynamic_buffers_ true) can
        // rely on their being at least as many packets as buffers at all times.
        //
        // A CodecAdapter that only ever puts an output buffer in flight once at a time can safely
        // assume that an available output buffer implies an available output packet.
        //
        // A decoder CodecAdapter for a bitstream format that's capable of emitting the same output
        // data (held in same buffer) more than once (whether within the bitstream spec or not) will
        // still need to tolerate having a buffer ready to emit without any currently free packets.
        //
        // At this point the new packet has is_new() true, and buffer() nullptr, so is easily
        // identifiable as a new packet rather than a previously-emitted output packet with a buffer
        // still associated. This packet is not associated with the buffer added above, it just
        // needs to exist so there will be enough packets to put all buffers in-flight downstream
        // concurrently if the bitstream happens to allow that.
        //
        // recycle outside lock
        output_packet_to_recycle = new_packet;
      } else {
        ZX_DEBUG_ASSERT(port == kInputPort);
        // For input packets, we assign a CodecPacket to the incoming protocol packet on reception;
        // in contrast for output packets we choose (somewhat arbitrarily) to leave a protocol
        // packet index set on every output packet except during a short transient interval where we
        // intentionally re-assign output protocol packet indexes to avoid client's taking
        // dependencies we don't want to commit to.
        new_packet->ClearProtocolPacketIndex();
        free_input_packets_.emplace_back(new_packet);
      }
    }
    ZX_DEBUG_ASSERT(current_buffer_count(port) <= packets_by_index.size());
  }  // ~lock

  if (is_wake_stream_control_condition_needed) {
    wake_stream_control_condition_.notify_all();
  }

  if (output_packet_to_recycle) {
    ZX_DEBUG_ASSERT(!output_packet_to_recycle->buffer());
    CoreCodecRecycleOutputPacket(output_packet_to_recycle);
  }

  // Check with sysmem to make sure the added buffer is consistent with constraints. Rely on this
  // thread being the only thread that modifies dynamic_buffer_collection_constraints_[port] etc to
  // start the GetVmoInfo call outside the lock. We intentionally do want more than one of these
  // GetVmoInfo calls to be able to proceed concurrently. On completion of GetVmoInfo, re-check the
  // buffer_constraints_version_ordinal against the
  // last_required_buffer_constraints_version_ordinal_[port] (etc). This defers checking for
  // duplicate buffer_index until GetVmoInfo completion, which is fine - we can't actually add the
  // buffer until after GetVmoInfo completion anyway, and it'd be more complicated overall to create
  // a CodecBuffer that's not yet fully-add-able just to enable detecting a duplicate buffer_index
  // sooner.
  fuchsia_sysmem2::AllocatorGetVmoInfoRequest request;
  request.vmo() = std::move(dup_vmo);
  // Regardless of buffer handle being sysmem strong or sysmem weak, we later will want a weak
  // parent VMO to enable ZX_VMO_ZERO_CHILDREN detection for the buffer handles given to the core
  // codec, so we know when it's safe to complete RemoveBuffer. So go ahead and get that sysmem
  // weak VMO handle here.
  request.need_single_buffer_settings() = true;
  bool is_match_existing;
  if (match_existing_vmo.has_value()) {
    request.vmo_settings_to_check() = TakeOptional(match_existing_vmo);
    if (port == kInputPort && IsDecoder()) {
      request.vmo_settings_to_check_ignore_size() = true;
    }
    is_match_existing = true;
  } else {
    is_match_existing = false;
  }
  request.constraints_to_check() = std::move(constraints_to_check);
  constraints_to_check.reset();
  // For output, we post to the same thread just to avoid adding more code here, but it'd also be
  // fine to not post when port is kOutputPort.
  PostToSharedFidl([this, request = std::move(request), port,
                    adding_buffer = std::move(adding_buffer), buffer_constraints_version_ordinal,
                    buffer_lifetime_ordinal, buffer_index, is_match_existing]() mutable {
    ZX_DEBUG_ASSERT(IsFidl());
    // We do shared_fidl_queue_.StopAndClear() before !sysmem_.is_valid(), so we can assert this
    // here.
    ZX_DEBUG_ASSERT(sysmem_.is_valid());
    // We postpone the buffer_lifetime_ordinal != buffer_lifetime_ordinal_[port] check until
    // GetVmoInfo response handling, to keep adding_buffers_ cleanup in one place.
    sysmem_->GetVmoInfo(std::move(request))
        .Then([this, port, adding_buffer = std::move(adding_buffer),
               buffer_constraints_version_ordinal, buffer_lifetime_ordinal, buffer_index,
               is_match_existing](
                  fidl::Result<fuchsia_sysmem2::Allocator::GetVmoInfo>& result) mutable {
          ZX_DEBUG_ASSERT(IsFidl());
          if (port == kInputPort) {
            PostSysmemCompletion([this, port, adding_buffer = std::move(adding_buffer),
                                  buffer_constraints_version_ordinal, buffer_lifetime_ordinal,
                                  buffer_index, is_match_existing,
                                  result = std::move(result)]() mutable {
              OnGetVmoInfoCompletion(port, std::move(adding_buffer),
                                     buffer_constraints_version_ordinal, buffer_lifetime_ordinal,
                                     buffer_index, is_match_existing, std::move(result));
            });
          } else {
            ZX_DEBUG_ASSERT(port == kOutputPort);
            OnGetVmoInfoCompletion(port, std::move(adding_buffer),
                                   buffer_constraints_version_ordinal, buffer_lifetime_ordinal,
                                   buffer_index, is_match_existing, std::move(result));
          }
        });
  });
}

void CodecImpl::OnGetVmoInfoCompletion(
    CodecPort port, std::shared_ptr<AddingBuffer> adding_buffer_param,
    uint64_t buffer_constraints_version_ordinal, uint64_t buffer_lifetime_ordinal,
    uint32_t buffer_index, bool is_match_existing,
    fidl::Result<fuchsia_sysmem2::Allocator::GetVmoInfo> result) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());

  auto constraints_version = CoreCodecGetConstraintsVersion(port);

  const fuchsia_sysmem2::BufferCollectionInfo* collection_info = nullptr;
  // A std::optional can't hold a reference anyway, so may as well use pointers rather than
  // std::optional<ptr> which would be redundant.
  CodecBuffer* buffer_ptr = nullptr;
  // this will destruct after the scoped "lock" below
  std::optional<BufferLifetimeOrdinalCleanupOutsideLock> delete_outside_lock;
  auto handle_delete_outside_lock =
      fit::defer([this, port, buffer_lifetime_ordinal, &delete_outside_lock] {
        if (delete_outside_lock.has_value()) {
          CoreCodecCloseBufferLifetimeOrdinal(port, buffer_lifetime_ordinal);
          delete_outside_lock.reset();
        }
      });
  {
    ScopedLock lock(lock_);

    // At this point, if the CodecAdapter has moved on to a new constraints_version, it can reject
    // the buffer being added and call onCoreCodecMidStreamOutputConstraintsChange2 with a
    // constraints version that's newer than constraints_version. This can happen since adding
    // buffers mid-stream during ongoing processing is permitted with is_supports_dynamic_buffers_
    // true.
    //
    // At this point we know the buffer is at least consistent with constraints previously returned
    // by the CodecAdapter. So the CodecAdapter will be able to correctly evaluate the added buffer
    // re. whether it's usable.

    auto& adding_by_ordinal = adding_buffers_[port];
    auto adding_by_ordinal_iter = adding_by_ordinal.find(buffer_lifetime_ordinal);
    // we only remove empty entries; this entry isn't empty
    ZX_DEBUG_ASSERT(adding_by_ordinal_iter != adding_by_ordinal.end());
    auto& adding_by_index = adding_by_ordinal_iter->second;
    auto adding_by_index_iter = adding_by_index.find(buffer_index);
    // we only remove adding_buffers_ entries when GetVmoInfo completes, so we know the entry is
    // still in adding_buffers_
    ZX_DEBUG_ASSERT(adding_by_index_iter != adding_by_index.end());
    auto adding_buffer = std::move(adding_by_index_iter->second);
    adding_by_index.erase(adding_by_index_iter);
    // This assert is the only reason we plumb adding_buffer_param to here. Treating
    // adding_buffer_param as authoritative and asserting that the found adding_buffer matches would
    // be better than removing the plumbing.
    ZX_DEBUG_ASSERT(adding_buffer.get() == adding_buffer_param.get());
    // Error paths here, and DeleteBuffer, both remove adding_buffer_[port][buffer_lifetime_ordinal]
    // if it's empty. This is a defer instead of just calling MaybeDeleteBufferLifetimeOrdinal here
    // because the success path doesn't need to run MaybeDeleteBufferLifetimeOrdinal because success
    // case moves the buffer from adding_buffers_ to active_buffers_.
    auto maybe_delete_buffer_lifetime_ordinal =
        fit::defer([this, port, buffer_lifetime_ordinal, &delete_outside_lock, &lock] {
          lock.AssertHeld(lock_);
          delete_outside_lock = MaybeDeleteBufferLifetimeOrdinal(port, buffer_lifetime_ordinal);
        });

    // The buffer_lifetime_ordinal != buffer_lifetime_ordinal_[port] check is below after we've
    // cleaned up adding_buffers_ entry.
    if (result.is_error()) {
      if (result.error_value().is_framework_error()) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_SysmemChannelClosed);
        FailLocked("AddBuffer GetVmoInfo failed - framework error: %s",
                   result.error_value().FormatDescription().c_str());
      } else {
        // GetVmoInfo doesn't fail due to constraints_to_check or vmo_settings_to_check - only if
        // there's a problem with the VMO or similar.
        //
        // If NOT_FOUND, probably the handle isn't referencing a VMO provided by sysmem (sysmem weak
        // VMOs count, but child (etc) VMOs don't count).
        ZX_DEBUG_ASSERT(result.error_value().is_domain_error());
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
        FailLocked("AddBuffer GetVmoInfo failed - error: %s",
                   result.error_value().FormatDescription().c_str());
      }
      // ~maybe_delete_buffer_lifetime_ordinal; ~lock; ~handle_delete_outside_lock
      return;
    }
    auto& response = *result;
    // GetVmoInfo succeeds even if both constraints_ok and vmo_settings_match are false, so we need
    // to check those here. We know we set constraints_to_check but we may not have set
    // vmo_settings_to_check if this is the first buffer of this buffer_lifetime_ordinal. Sysmem
    // sets these output fields exactly when we set the corresponding input fields.
    ZX_DEBUG_ASSERT(response.constraints_ok().has_value());
    ZX_DEBUG_ASSERT(is_match_existing == response.vmo_settings_match().has_value());
    if (response.vmo_settings_match().has_value() && !*response.vmo_settings_match()) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "AddBuffer non-matching SingleBufferSettings per buffer_lifetime_ordinal - port: %u",
          port);
      // ~maybe_delete_buffer_lifetime_ordinal; ~lock; ~handle_delete_outside_lock
      return;
    }
    ZX_ASSERT(response.single_buffer_settings().has_value());
    ZX_ASSERT(response.single_buffer_settings()->buffer_settings().has_value());
    ZX_ASSERT(response.single_buffer_settings()->buffer_settings()->coherency_domain().has_value());

    if (IsStoppingLocked()) {
      // ~maybe_delete_buffer_lifetime_ordinal; ~lock; ~handle_delete_outside_lock
      return;
    }

    // We intentionally don't clean up adding_by_index.empty() here, in case more buffers get added,
    // and to avoid making QueueInputPacket more complicated in the case where the specified buffer
    // hasn't completed GetVmoInfo yet.

    if (adding_buffer->continue_remove_) {
      auto continue_remove = std::move(adding_buffer->continue_remove_);
      // drop the buffer
      adding_buffer.reset();
      // complete the remove that came in before the add was done
      std::move(continue_remove)(lock);
      // ~maybe_delete_buffer_lifetime_ordinal; ~lock; ~handle_delete_outside_lock
      return;
    }

    // Was checked previously in AddBuffer, and sent_buffer_constraints_version_ordinal_[port] only
    // increases, so still true now.
    ZX_DEBUG_ASSERT(buffer_constraints_version_ordinal <=
                    sent_buffer_constraints_version_ordinal_[port]);
    if (buffer_constraints_version_ordinal <
        last_required_buffer_constraints_version_ordinal_[port]) {
      // No reason to keep going here since the client needs to catch up to the latest required
      // first. This path can happen if the client was doing an AddBuffer while decode was ongoing
      // but the decoder his a dimensions change in the stream before the AddBuffer is fully
      // complete.
      //
      // ~adding_buffer etc
      return;
    }

    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal <= buffer_lifetime_ordinal_[port]);
    if (buffer_lifetime_ordinal != buffer_lifetime_ordinal_[port]) {
      // This buffer would be pending removal if it had completed being added in time, so we can
      // just not add it here. Any subsequent RemoveBuffer specifying this buffer will complete
      // quickly.
      //
      // ~adding_buffer etc
      return;
    }

    // Even if the SingleBufferSettings of buffers being added by the client all match each other,
    // it doesn't imply that the CodecAdapter has ever accepted these settings or ever even had any
    // input to these settings whatseover (these could be completely arbitrary sysmem VMOs at this
    // point). We don't want to add any buffer that isn't consistent with constraints the
    // CodecAdapter specified.
    if (!*response.constraints_ok()) {
      // If the CodecAdapter's constraints have changed since we created the
      // buffer_constraints_version_ordinal, it's possible that ParticipateInBufferAllocation may
      // have been using older constraints than the constraints_to_check set by AddBuffer. We
      // intentionally don't track which ParticipateInBufferAllocation corresponds to the AddBuffer
      // and the protocol doesn't strictly require the ParticipateInBufferAllocation to be on the
      // same StreamProcessor instance, so this check gives the client the benefit of the doubt when
      // the CodecAdapter has a newer constraints_version.
      ZX_DEBUG_ASSERT(constraints_version >= last_sent_codec_adapter_output_constraints_version_);
      if (constraints_version == last_sent_codec_adapter_output_constraints_version_) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
        // The client is caught up with the latest constraints_version, but the constraints aren't
        // compatible with the VMO sent in AddBuffer.
        FailLocked("AddBuffer GetVmoInfo !constraints_ok");
        return;
      }
      // Let the client catch up. In this case, since we're not handing this buffer to the
      // CodecAdapter, we have to ensure a mid-stream output constraints change gets triggered for
      // the CodecAdapter's newer constraints_version.
      //
      // If the client catches up and still hits !constraints_ok, we can unambiguously blame the
      // client at that point.
      maybe_delete_buffer_lifetime_ordinal.call();
      lock.unlock();
      handle_delete_outside_lock.call();
      EnsureMidStreamOutputConstraintsChange(constraints_version, buffer_lifetime_ordinal);
      return;
    }

    CodecBuffer::Info buffer_info;
    buffer_info.port = port;
    buffer_info.lifetime_ordinal = buffer_lifetime_ordinal;
    buffer_info.index = buffer_index;
    ZX_ASSERT(response.single_buffer_settings().has_value());
    ZX_ASSERT(response.single_buffer_settings()->buffer_settings().has_value());
    ZX_ASSERT(response.single_buffer_settings()->buffer_settings()->is_secure().has_value());
    buffer_info.is_secure =
        response.single_buffer_settings()->buffer_settings()->is_secure().value();
    uint64_t vmo_size;
    zx_status_t get_size_status = adding_buffer->unverified_vmo_.get_size(&vmo_size);
    ZX_ASSERT(get_size_status == ZX_OK);
    // buffer VMO is now verified
    auto vmo_range = CodecVmoRange(std::move(adding_buffer->unverified_vmo_), 0, vmo_size);

    auto buffer = std::unique_ptr<CodecBuffer>(
        new CodecBuffer(this, std::move(buffer_info), std::move(vmo_range)));
    buffer->SetDoDelete([this](CodecBuffer* buffer) { DoCodecBufferDelete(buffer); });
    buffer_ptr = buffer.get();

    auto& active_by_ordinal = active_buffers_[port];
    auto active_by_ordinal_iter = active_by_ordinal.find(buffer_lifetime_ordinal);
    if (active_by_ordinal_iter == active_by_ordinal.end()) {
      ZX_DEBUG_ASSERT(active_packets_[port].size() >= active_buffers_[port].size());
      if (active_buffers_[port].size() >= kMaxActiveBufferLifetimeOrdinals) {
        // A client reconfiguring buffers ~kMaxActiveBufferLifetimeOrdinals times in a row without
        // waiting for a Sync completion could hit this. Clients that might reconfigure buffers that
        // many times in a row without the buffers being used for any output in between should use
        // Sync to fence ordinals older than this many values ago to avoid this (and consider ways
        // to avoid spamming buffer reconfiguration in the first place, if at all feasible).
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
        // A client can fence with Sync to avoid this. See above.
        FailLocked("Too many active buffer_lifetime_ordinals (AddBufferInternal)");
        return;
      }

      fuchsia_sysmem2::BufferCollectionInfo info;
      info.settings() = std::move(*response.single_buffer_settings());
      ZX_DEBUG_ASSERT(!info.buffer_collection_id().has_value());
      // When using dynamic buffers, the buffers field is un-set, and the CodecAdapter must rely on
      // CoreCodecAddBuffer and CoreCodecRemoveBuffer (and possibly-delayed closing of its own
      // buffer handles) to know how many buffers there are.
      ZX_DEBUG_ASSERT(!info.buffers().has_value());

      auto& port_settings = *port_settings_[port];
      // If the active buffers dropped to zero and is now going back up from zero, for the same
      // buffer_lifetime_ordinal, this can be clearing a BufferCollectionInfo.
      port_settings.ClearBufferCollectionInfo();
      port_settings.SetBufferCollectionInfo(std::move(info));
      // This will stay allocated until the CoreCodecSetBufferCollectionInfo call below because this
      // thread is the only thread that modifies port_settings_[port] (short of this thread having
      // exited first).
      collection_info = &port_settings.buffer_collection_info();

      auto& fake_map_ranges_by_ordinal = fake_map_range_[port];
      // If the active buffers dropped to zero and now is going back up from zero, for the same
      // buffer_lifetime_ordinal, this may be deleting an entry.
      fake_map_ranges_by_ordinal.erase(buffer_lifetime_ordinal);
      if (port_settings_[port]->is_secure() && IsCoreCodecMappedBufferUseful(port)) {
        std::optional<FakeMapRange> new_fake_map_range;
        zx_status_t status =
            FakeMapRange::Create(port_settings_[port]->vmo_usable_size(), &new_fake_map_range);
        if (status != ZX_OK) {
          LogEvent(media_metrics::
                       StreamProcessorEvents2MigratedMetricDimensionEvent_InitializationError);
          FailLocked("FakeMapRange::Create() failed");
          return;
        }
        fake_map_range_[port].emplace(std::make_pair(
            buffer_lifetime_ordinal,
            std::unique_ptr<FakeMapRange>(new FakeMapRange(std::move(*new_fake_map_range)))));
      }

      active_by_ordinal_iter =
          active_by_ordinal.insert(std::make_pair(buffer_lifetime_ordinal, BuffersByIndex{})).first;
    }

    if (IsCoreCodecMappedBufferUseful(port)) {
      if (port_settings_[port]->is_secure()) {
        auto& fake_map_ranges_by_ordinal = fake_map_range_[port];
        auto fake_map_ranges_by_ordinal_iter =
            fake_map_ranges_by_ordinal.find(buffer_lifetime_ordinal);
        ZX_DEBUG_ASSERT(fake_map_ranges_by_ordinal_iter != fake_map_ranges_by_ordinal.end());
        auto& fake_map_range = fake_map_ranges_by_ordinal_iter->second;
        buffer->FakeMap(fake_map_range->base());
      } else {
        if (!buffer->Map()) {
          LogEvent(
              media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_AllocationError);
          FailLocked("buffer->Map() failed");
          return;
        }
      }
    }

    auto& active_by_index = active_by_ordinal_iter->second;
    auto insert_result = active_by_index.insert(std::make_pair(buffer_index, std::move(buffer)));
    ZX_ASSERT(insert_result.second);
    // Now we know we're not just deleting an entry in adding_buffers_, but moving the buffer from
    // adding_buffers_ to active_buffers_, so no need to call MaybeDeleteBufferLifetimeOrdinal since
    // it wouldn't do anything due to a buffer in active_buffers_[port][buffer_lifetime_ordinal].
    // Later (possibly much later), DeleteBuffer will run MaybeDeleteBufferLifetimeOrdinal.
    maybe_delete_buffer_lifetime_ordinal.cancel();
  }  // ~lock

  ZX_ASSERT(!delete_outside_lock.has_value());
  // may as well cancel since it won't do anything
  handle_delete_outside_lock.cancel();

  PostToSharedFidl([this, buffer_ptr] {
    // This is the only way that buffer_ptr becomes invalid other than !shared_fidl_queue_ happening
    // first, so we know buffer_ptr is still alive here.
    buffer_ptr->BeginWaitForZeroChildren(shared_fidl_dispatcher_);
  });

  if (IsCoreCodecHwBased(port) && *core_codec_bti_) {
    zx_status_t status = buffer_ptr->Pin();
    if (status != ZX_OK) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_InitializationError);
      Fail("buffer->Pin() failed - status: %d port: %d", status, port);
      return;
    }
  }

  // The current thread is the only thread that modifies active_buffers_[port] or
  // active_packets_[port] (without first stopping this thread), so we know buffer_ptr and
  // packet_to_recycle remain allocated.

  if (collection_info) {
    // The CodecAdapter can trigger onCoreCodecMidStreamOutputConstraintsChange if these settings
    // are already stale (potentially due to stream switching) or become stale later.
    CoreCodecSetBufferCollectionInfo(port, *collection_info);
  }
  CoreCodecAddBuffer(port, buffer_ptr);
  buffer_ptr->SetWasEverAddedToCoreCodec();
}

bool CodecImpl::AddNonDynamicBufferCommon(CodecBuffer::Info buffer_info, CodecVmoRange vmo_range) {
  const CodecPort port = buffer_info.port;
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  bool buffers_done_configuring = false;
  uint64_t buffer_lifetime_ordinal = buffer_info.lifetime_ordinal;
  uint32_t buffer_index = buffer_info.index;

  ScopedLock lock(lock_);

  if (buffer_lifetime_ordinal % 2 == 0) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    FailLocked("Client sent even buffer_lifetime_ordinal, but must be odd - exiting - port: %u\n",
               port);
    return false;
  }

  if (buffer_lifetime_ordinal != protocol_buffer_lifetime_ordinal_[port]) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    FailLocked(
        "Incoherent SetOutputBufferSettings()/SetInputBufferSettings() + "
        "AddOutputBuffer()/AddInputBuffer()s - exiting - port: %d\n",
        port);
    return false;
  }

  // If the server has already moved on from the client's
  // buffer_lifetime_ordinal, the client's buffer_lifetime_ordinal won't match
  // the server's buffer_lifetime_ordinal_. The client will probably later catch
  // up.
  if (buffer_lifetime_ordinal != buffer_lifetime_ordinal_[port]) {
    // The case that ends up here is when a client's output configuration
    // (whole or last part) is being ignored because it's not yet caught up
    // with last_required_buffer_constraints_version_ordinal_.

    // This case won't happen for input, at least for now.  This is an assert
    // rather than a client behavior check, because previous client protocol
    // checks have already peeled off any invalid client behavior that might
    // otherwise cause this assert to trigger.
    ZX_DEBUG_ASSERT(port == kOutputPort);

    // Ignore the client's message.  The client will probably catch up later.
    return false;
  }

  auto* current_buffers_ptr = all_buffers(port);
  ZX_ASSERT(current_buffers_ptr);
  auto& current_buffers = *current_buffers_ptr;
  ZX_DEBUG_ASSERT(buffer_index == current_buffers.size());
  // when not using dynamic buffers, we require exactly the min_buffer_count
  // which is the buffer_collection_info_ buffer count - so in this case it's
  // also the max
  uint32_t required_buffer_count = port_settings_[port]->min_buffer_count();
  ZX_DEBUG_ASSERT(buffer_index < required_buffer_count);

  std::unique_ptr<CodecBuffer> local_buffer = std::unique_ptr<CodecBuffer>(
      new CodecBuffer(this, std::move(buffer_info), std::move(vmo_range)));
  local_buffer->SetDoDelete([this](CodecBuffer* buffer) { DoCodecBufferDelete(buffer); });

  if (IsCoreCodecMappedBufferUseful(port)) {
    auto& fake_map_ranges_by_ordinal = fake_map_range_[port];
    auto fake_map_ranges_by_ordinal_iter = fake_map_ranges_by_ordinal.find(buffer_lifetime_ordinal);
    if (fake_map_ranges_by_ordinal_iter != fake_map_ranges_by_ordinal.end()) {
      auto& fake_map_range = fake_map_ranges_by_ordinal_iter->second;
      // The fake_map_range_[port]->base() is % PAGE_SIZE == 0, which is the same as a mapping
      // would be.  There are sufficient virtual pages starting at FakeMapRange::base() to permit
      // CodecBuffer to include the low-order vmo_usable_start % PAGE_SIZE bits in
      // CodecBuffer::base(), for any vmo_usable_start() value (even the worst case of
      // PAGE_SIZE - 1, and buffer size % PAGE_SIZE == 2).  By including those low-order
      // intra-page-offset bits, we can treat non-secure and secure cases similarly.
      local_buffer->FakeMap(fake_map_range->base());
    } else {
      // So far, there's little reason to avoid doing the Map() part under the
      // lock, even if it can be a bit more time consuming, since there's no data
      // processing happening at this point anyway, and there wouldn't be any
      // happening in any other code location where we could potentially move the
      // Map() either.
      if (!local_buffer->Map()) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_InitializationError);
        FailLocked("AddOutputBuffer()/AddInputBuffer() couldn't Map() new buffer - port: %d", port);
        return false;
      }
    }
  }

  // We keep the buffers pinned for DMA continuously, since there's not much benefit to un-pinning
  // and re-pinning them (so far).  By pinning, we prevent sysmem from recycling the
  // BufferCollection VMOs until the driver has re-started and un-quarantined pinned pages (via
  // its BTI), after ensuring the HW is no longer doing DMA from/to the pages.
  //
  // TODO(https://fxbug.dev/42114424): All CodecAdapter(s) that start memory access that can
  // continue beyond VMO handle closure during process death/termination should have a BTI.
  // Resolving this
  // TODO will require updating at least the amlogic-video VP9 decoder to provide a BTI.
  //
  // TODO(https://fxbug.dev/42114425): Currently OEMCrypto's indirect (via FIDL) SMC calls that take
  // physical addresses are not guaranteed to be fully over/done before VMO handles are auto-closed
  // by OEMCrypto assuming OEMCryto's process dies/terminates.
  if (IsCoreCodecHwBased(port) && *core_codec_bti_) {
    zx_status_t status = local_buffer->Pin();
    if (status != ZX_OK) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_InitializationError);
      FailLocked("buffer->Pin() failed - status: %d port: %d", status, port);
      return false;
    }
  }

  CodecBuffer* buffer_ptr = local_buffer.get();
  current_buffers.emplace(buffer_index, std::move(local_buffer));

  // When is_supports_dynamic_buffers_, the number of packets is always >= the number of buffers.
  // When !is_supports_dynamic_buffers_, packets get added after all the buffers have been added.

  CodecPacket* packet_to_add_before_buffer = nullptr;
  if (is_supports_dynamic_buffers()) {
    auto& current_packets = active_packets_[port][buffer_lifetime_ordinal];
    if (current_buffers.size() > current_packets.size()) {
      ZX_DEBUG_ASSERT(current_packets.size() + 1 == current_buffers.size());
      auto& packets_by_index = active_packets_[port][buffer_lifetime_ordinal];
      ZX_DEBUG_ASSERT(packets_by_index.size() <= std::numeric_limits<uint32_t>().max());
      auto new_packet = std::unique_ptr<CodecPacket>(
          new CodecPacket(buffer_lifetime_ordinal, static_cast<uint32_t>(packets_by_index.size())));
      new_packet->SetParent(this);
      if (port == kOutputPort) {
        packet_to_add_before_buffer = new_packet.get();
      }
      packets_by_index.emplace_back(std::move(new_packet));
    }
  }

  {
    ScopedUnlock unlock(*this);
    if (packet_to_add_before_buffer) {
      ZX_DEBUG_ASSERT(!packet_to_add_before_buffer->buffer());
      CoreCodecRecycleOutputPacket(packet_to_add_before_buffer);
    }
    CoreCodecAddBuffer(port, buffer_ptr);
    buffer_ptr->SetWasEverAddedToCoreCodec();
  }
  lock.AssertHeld(lock_);

  if (current_buffers.size() == required_buffer_count) {
    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[port] ==
                    port_settings_[port]->buffer_lifetime_ordinal());
    // Stash this while we can, before the client de-configures.
    last_provided_buffer_constraints_version_ordinal_[port] =
        port_settings_[port]->buffer_constraints_version_ordinal();
    if (!is_supports_dynamic_buffers()) {
      // Now we allocate active_packets_[port][buffer_lifetime_ordinal].
      auto& packets_by_ordinal = active_packets_[port];
      ZX_DEBUG_ASSERT(packets_by_ordinal.find(buffer_lifetime_ordinal) == packets_by_ordinal.end());
      auto insert_result = packets_by_ordinal.emplace(buffer_lifetime_ordinal, PacketsByIndex{});
      ZX_ASSERT(insert_result.second);
      auto& packets_by_index = insert_result.first->second;
      ZX_DEBUG_ASSERT(packets_by_index.empty());
      ZX_DEBUG_ASSERT(all_packets(port).empty());
      uint32_t packet_count = port_settings_[port]->packet_count();
      for (uint32_t i = 0; i < packet_count; i++) {
        // Private constructor to prevent core codec maybe creating its own
        // Packet instances (which isn't the intent) seems worth the hassle of
        // not using make_unique<>() here.
        auto new_packet = std::unique_ptr<CodecPacket>(new CodecPacket(buffer_lifetime_ordinal, i));
        new_packet->SetParent(this);
        packets_by_index.emplace_back(std::move(new_packet));
      }

      {  // scope unlock
        ScopedUnlock unlock(*this);

        // A core codec can take action here to finish configuring buffers if
        // it's able, or can delay configuring buffers until
        // CoreCodecStartStream() or
        // CoreCodecMidStreamOutputBufferReConfigFinish() if that works better
        // for the core codec.
        //
        // In any case, during a mid-stream output constraints change, the core
        // codec must not call any onCoreCodecOutput* methods until the core
        // codec sees CoreCodecStopStream() (after stopping the stream, in
        // preparation for the next stream), or
        // CoreCodecMidStreamOutputBufferReConfigFinish().
        //
        // In other words, this call does /not/ imply un-pausing output.
        CoreCodecConfigureBuffers(port, packets_by_index);

        // All output packets need to start with the core codec.  This is
        // implicit for the StreamProcessor interface (implied by adding the last output
        // buffer) but explicit in the CodecAdapter interface.
        if (port == kOutputPort) {
          for (uint32_t i = 0; i < packet_count; i++) {
            CodecPacket* packet = packets_by_index[i].get();
            ZX_DEBUG_ASSERT(!packet->buffer());
            CoreCodecRecycleOutputPacket(packet);
          }
        }
      }  // ~unlock
    }

    is_port_configured_[port] = true;
    buffers_done_configuring = true;

    // For client-called AddOutputBuffer(), the last buffer being added is
    // analogous to CompleteOutputBufferPartialSettings(); we handle that
    // analogous-ness in IsOutputConfiguredLocked() (not by pretending we got
    // a CompleteOutputBufferPartialSettings() here), so
    // is_port_configured_[port] = true above is enough to make
    // IsOutputConfiguredLocked() return true if this is a client-driven
    // AddOutputBuffer().
  }

  PostToSharedFidl([this, buffer_ptr] {
    // This is the only way that buffer_ptr becomes invalid other than !shared_fidl_queue_ happening
    // first, so we know buffer_ptr is still alive here.
    buffer_ptr->BeginWaitForZeroChildren(shared_fidl_dispatcher_);
  });

  return buffers_done_configuring;
}

void CodecImpl::DoCodecBufferDelete(CodecBuffer* buffer) {
  // We know buffer is still allocated at this point because all buffers get destructed before
  // "this", and buffer destruction cancels the wait without running the buffer's do_delete_ (which
  // calls this method directly).
  ZX_DEBUG_ASSERT(IsFidl());
  if (buffer->port() == kInputPort) {
    PostToStreamControl([this, buffer] {
      // We know buffer is still alive here because the only ways for a buffer to be deallocated are
      // via this path or via forced deletion of the buffer which can only happen after
      // stream_control_queue_.StopAndClear(), which would prevent running this lambda posted via
      // stream_control_queue_.
      ZX_DEBUG_ASSERT(IsStreamControl());
      DeleteBuffer(buffer);
    });
    return;
  }
  ZX_DEBUG_ASSERT(buffer->port() == kOutputPort);
  DeleteBuffer(buffer);
}

bool CodecImpl::CheckPlausibleBufferLifetimeOrdinalLocked(CodecPort port,
                                                          uint64_t buffer_lifetime_ordinal) {
  // The client must only send odd values.  0 is even so we don't need a
  // separate check for that.
  if (buffer_lifetime_ordinal % 2 == 0) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    FailLocked(
        "CheckPlausibleBufferLifetimeOrdinalLocked() - buffer_lifetime_ordinal must "
        "be odd");
    return false;
  }
  if (buffer_lifetime_ordinal > protocol_buffer_lifetime_ordinal_[port]) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    FailLocked(
        "client sent new buffer_lifetime_ordinal in message type that doesn't "
        "allow new buffer_lifetime_ordinals");
    return false;
  }
  return true;
}

bool CodecImpl::CheckStreamLifetimeOrdinalLocked(uint64_t stream_lifetime_ordinal) {
  if (stream_lifetime_ordinal % 2 != 1) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    FailLocked("stream_lifetime_ordinal must be odd.\n");
    return false;
  }
  if (stream_lifetime_ordinal < stream_lifetime_ordinal_) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    FailLocked("client sent stream_lifetime_ordinal that went backwards");
    return false;
  }
  return true;
}

bool CodecImpl::StartNewStream(ScopedLock& lock, uint64_t stream_lifetime_ordinal,
                               bool is_for_packet) {
  VLOGF("StartNewStream()");
  ZX_DEBUG_ASSERT(IsStreamControl());
  ZX_DEBUG_ASSERT((stream_lifetime_ordinal % 2 == 1) && "new stream_lifetime_ordinal must be odd");

  if (IsStoppingLocked()) {
    // Don't start a new stream if the whole CodecImpl is already stopping.
    //
    // A completely different path will take care of calling
    // EnsureStreamClosed() during CodecImpl stop.
    //
    // Callers will already be checking IsStoppingLocked() at the top of each
    // relevant .*_StreamControl method. Assuming those checks remain in place,
    // we don't need to be checking again here for correctness, but we go ahead
    // and check again here just before starting a new stream to help avoid any
    // long waits for StreamControl to exit when stopping this CodecImpl. This
    // is a fairly minor optimization, just to avoid waiting for the rest of
    // StartNewStream and the extra cleanup involved when we've recently begun
    // trying to clean up.
    return false;
  }

  EnsureStreamClosed(lock);
  ZX_DEBUG_ASSERT(!IsStreamActiveLocked());

  // Now it's time to start the new stream.  We start the new stream at
  // Codec layer first then core codec layer.

  if ((is_for_packet || !is_supports_dynamic_buffers()) && !IsInputConfiguredLocked()) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    FailLocked("input not configured before start of stream (QueueInputPacket())");
    return false;
  }

  // The lock_ is held.  We don't need lock_ to keep stream_ alive.  That's not necessary because
  // only StreamControl domain will remove items from stream_queue_.  This hold interval is to
  // protect stream_queue_ against concurrent modification by output domain (FIDL thread) only.
  ZX_DEBUG_ASSERT(stream_queue_.size() >= 1);
  ZX_DEBUG_ASSERT(stream_lifetime_ordinal == stream_queue_.front()->stream_lifetime_ordinal());
  stream_ = stream_queue_.front().get();

  // Update the stream_lifetime_ordinal_ to the new stream.  We need to do
  // this before we send new output config, since the output config will be
  // generated using the current stream ordinal.
  ZX_DEBUG_ASSERT(stream_lifetime_ordinal > stream_lifetime_ordinal_);
  stream_lifetime_ordinal_ = stream_lifetime_ordinal;
  ZX_DEBUG_ASSERT(stream_->stream_lifetime_ordinal() == stream_lifetime_ordinal_);

  // The client is not permitted to unilaterally re-configure output while a
  // stream is active, but the client may still be responding to a previous
  // server-initiated mid-stream format change.
  //
  // ###########################################################################
  // We don't attempt to optimize every case as much as might be possible here.
  // The main overall optimization is that it's possible to switch streams
  // without reallocating buffers.  We also need to make sure it's possible to
  // detect output format at the start of a stream regardless of what happened
  // before, and possible to perform a mid-stream format change.
  // ###########################################################################
  //
  // Given the above, our *main concern* here is that we get to a state where we
  // *know* the client isn't trying to re-configure output during format
  // detection, which at best would be confusing to allow, so we avoid that
  // possibility here by forcing a client to catch up with the server, if
  // there's *any possibility* that the client might still be working on
  // catching up with the server.
  //
  // If the client's most recently fully-completed output config is less than
  // the most recently sent output constraints with action_required true, then
  // we force an even fresher output constraints here tagged as being relevant
  // to the current stream, and wait for the client to catch up to that before
  // continuing.  By marking as being for this stream, we ensure that the client
  // will bother to finish configuring output, which gets us to a state where we
  // know it's safe to do another mid-stream format change as needed (vs. the
  // client maybe finishing the old config or maybe not).
  //
  // We also force the client to catch up if the core codec previously indicated
  // that the current config is "meh".  This may not be strictly necessary since
  // the "meh" was with respect to the old stream, but just in case a core codec
  // cares, we move on from the old config before delivering new stream data.
  //
  // Some core codecs may require the output to be configured to _something_ as
  // they don't support giving us the real output config unless the output is
  // configured to at least something at first.
  //
  // Other core codecs (such as some HW-based codecs) can deal with no output
  // configured while detecting the output format, but even for those codecs, we
  // only do this if the above cases don't apply.  These codecs have to deal
  // with an output config that's already set across a stream switch anyway, to
  // permit buffers to stay configured across a stream switch when possible, so
  // the cases above potentially setting an output config that's not super
  // relevant to the new stream doesn't really complicate the core codec since
  // an old stream's config might not be super relevant to a new stream either.
  //
  // Format detection is separate and handled like a mid-stream format change.
  // This stuff here is just getting output config into a non-changing state
  // before we start format detection.
  bool is_new_config_needed;
  // The statement below could obviously be re-written as a giant boolean
  // expression, but this way seems easier to comment.
  if (last_provided_buffer_constraints_version_ordinal_[kOutputPort] <
      last_required_buffer_constraints_version_ordinal_[kOutputPort]) {
    // The client _might_ still be trying to catch up, so to disambiguate,
    // require an even fresher config with respect to this new stream to
    // unambiguously force the client to catch up to the even newer config.
    is_new_config_needed = true;
  } else if (IsCoreCodecRequiringOutputConfigForFormatDetection() && !IsOutputConfiguredLocked()) {
    // The core codec requires output to be configured before format detection,
    // so we force the client to provide an output config before format
    // detection.
    is_new_config_needed = true;
  } else if (IsOutputConfiguredLocked() &&
             port_settings_[kOutputPort]->buffer_constraints_version_ordinal() <=
                 codec_adapter_meh_output_buffer_constraints_version_ordinal_) {
    // The core codec previously expressed "meh" regarding the current config's
    // buffer_constraints_version_ordinal, so to avoid mixing that with core
    // codec stream switch, force the client to configure output buffers before
    // format detection for the new stream.
    is_new_config_needed = true;
  } else {
    // The core codec is ok to perform format detection in the current state,
    // and we know that a well-behaved client is not currently trying to change
    // the output config.
    is_new_config_needed = false;
  }

  if (is_new_config_needed) {
    auto paused_output = std::make_shared<PausedOutput>(*this);
    if (!RunSyncOnSharedFidlForStream(
            lock, [this, paused_output = std::move(paused_output)]() mutable {
              ScopedLock lock(lock_);
              if (IsStoppingLocked()) {
                return;
              }
              StartIgnoringClientOldOutputConfig(lock);
              EnsureBuffersNotConfigured(lock, kOutputPort, false);
              GenerateAndSendNewOutputConstraints(lock, std::move(paused_output));
            })) {
      ZX_DEBUG_ASSERT(IsStoppingLocked());
      return false;
    }

    // Now we can wait for the client to catch up to the current output config or for the client to
    // tell the server to discard the current stream.
    stream_->AssertHeld(this);
    while (!IsStoppingLocked() && !stream_->future_discarded() && !IsOutputConfiguredLocked()) {
      RunAnySysmemCompletionsOrWait(lock);
    }

    if (IsStoppingLocked()) {
      return false;
    }

    if (stream_->future_discarded()) {
      // A discarded stream isn't an error for the CodecImpl instance.
      return true;
    }

    // ~paused_output
  }

  // Now we have input configured, and output configured if needed by the core
  // codec, so we can move the core codec to running state.
  {  // scope unlock
    ScopedUnlock unlock(*this);
    CoreCodecStartStream();
  }  // ~unlock
  lock.AssertHeld(lock_);

  // Track this so the core codec doesn't have to bother with "ensure"
  // semantics, just start/stop, where stop isn't called unless the core codec
  // has a started stream.
  is_core_codec_stream_started_ = true;

  return true;
}

void CodecImpl::EnsureStreamClosed(ScopedLock& lock) {
  VLOGF("EnsureStreamClosed()");
  ZX_DEBUG_ASSERT(IsStreamControl());

  // Ensure the old stream is closed at CodecAdapter layer.  The stream may already be closed at
  // CodecAdapter layer, such as if the CodecAdapter previously used onCoreCodecFailStream() and the
  // async work posted by that call has already completed.
  EnsureCoreCodecStreamStopped(lock);

  // Now close the old stream at the StreamProcessor layer.
  EnsureCodecStreamClosedLockedInternal();

  ZX_DEBUG_ASSERT(!IsStreamActiveLocked());
}

void CodecImpl::EnsureCoreCodecStreamStopped(ScopedLock& lock) {
  lock.AssertHeld(lock_);
  // Stop the core codec, by using this thread to directly drive the core codec
  // from running to stopped (if not already stopped).  We do this first so the
  // core codec won't try to send us output while we have no stream at the Codec
  // layer.
  if (is_core_codec_stream_started_) {
    {  // scope unlock
      ScopedUnlock unlock(*this);
      VLOGF("CoreCodecStopStream()...");
      CoreCodecStopStream();
      VLOGF("CoreCodecStopStream() done.");
    }
    is_core_codec_stream_started_ = false;
  }
}

// The only valid caller of this is EnsureStreamClosed().  We have this in a
// separate method only to make it easier to assert a couple things in the
// caller.
void CodecImpl::EnsureCodecStreamClosedLockedInternal() {
  ZX_DEBUG_ASSERT(IsStreamControl());
  if (stream_lifetime_ordinal_ % 2 == 0) {
    // Already closed.
    return;
  }

  // The lock_ is held which protects against concurrent modification by output domain (FIDL
  // thread), and is also held while deleting first item in stream_queue_ so that output domain can
  // hold lock_ to ensure a stream obtained from stream_queue_ stays alive until output domain is
  // done marking the stream as future_flushed() or similar.
  ZX_DEBUG_ASSERT(stream_queue_.front()->stream_lifetime_ordinal() == stream_lifetime_ordinal_);
  stream_ = nullptr;
  stream_queue_.pop_front();

  stream_lifetime_ordinal_++;
  // Even values mean no current stream.
  ZX_DEBUG_ASSERT(stream_lifetime_ordinal_ % 2 == 0);
}

bool CodecImpl::RunAnySysmemCompletions(ScopedLock& lock) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  // Typically this loop will run once, but on return we want the queue to be
  // empty even if more showed up while in this method, for condition_variable
  // signalling reasons.
  bool any_ran = false;
  while (!sysmem_completion_queue_.empty()) {
    // We'll run them all, so extract all the items and run them all.
    std::queue<fit::closure> local_batch_to_run;
    local_batch_to_run.swap(sysmem_completion_queue_);
    {  // scope unlock
      // The unlock doesn't cause queue re-ordering, though so far none of these
      // items care anyway.
      ScopedUnlock unlock(*this);
      while (!local_batch_to_run.empty()) {
        any_ran = true;
        fit::closure to_run = std::move(local_batch_to_run.front());
        local_batch_to_run.pop();
        to_run();
      }
    }  // ~unlock
    lock.AssertHeld(lock_);
  }
  return any_ran;
}

void CodecImpl::PostSysmemCompletion(fit::closure to_run) {
  ZX_DEBUG_ASSERT(IsFidl());

  {  // scope lock
    ScopedLock lock(lock_);
    sysmem_completion_queue_.emplace(std::move(to_run));
    // In case there is no WaitEnsureSysmemReadyOnInput(), we post to
    // StreamControl to ensure that RunAnySysmemCompletions() runs soon.
    // Don't let them accumulate though.
    if (!is_sysmem_runner_pending_) {
      is_sysmem_runner_pending_ = true;
      PostToStreamControl([this] {
        ScopedLock lock(lock_);
        std::ignore = RunAnySysmemCompletions(lock);
        ZX_DEBUG_ASSERT(sysmem_completion_queue_.empty());
        is_sysmem_runner_pending_ = false;
      });
    }
  }  // ~lock

  // In case to_run needs to get run by a QueueInput...StreamControl() method
  // via WaitEnsureSysmemReadyOnInput(), we wake the StreamControl thread.  We
  // must do this even if is_sysmem_runner_pending_, in case that runner won't
  // run for a while due to WaitEnsureSysmemReadyOnInput() blocking
  // StreamControl.
  wake_stream_control_condition_.notify_all();
}

bool CodecImpl::WaitEnsureSysmemReadyOnInput(ScopedLock& lock) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  // Input buffer re-config is not permitted unless there's no current stream.
  ZX_DEBUG_ASSERT(!IsStreamActiveLocked());
  while (!IsInputConfiguredLocked()) {
    RunAnySysmemCompletionsOrWait(lock);
    // No need to check for stream switch since it's not permitted for a client
    // to be sending any message that can cause a new stream until after the
    // client is done configuring input buffers (enforced elsewhere).
    if (IsStoppingLocked()) {
      return false;
    }
  }
  return true;
}

// Using non-dynamic buffers, the sysmem allocation completions are related to
// SetInputBufferPartialSettings / SetOutputBufferPartialSettings. Completing these on the
// StreamControl thread is important wrt the ordering constraints of CodecAdapter re. when buffers
// can be added (in particular if the core codec doesn't support dynamic buffers).
//
// Using dynamic buffers, sysmem allocation completions are related to
// ParticipateInBufferAllocation, but those sysmem completions are dealt with outside of any code in
// this process. Instead, AddBuffer is sent by the client when the buffer has already been
// allocated. In this case, adding a buffer to the CodecAdapter is on StreamControl for input and
// fidl thread (output ordering domain) for output. In the case of input AddBuffer, we still need to
// run the AddBuffer via this queue, for the case where a QueueInput...StreamControl() method is
// calling WaitEnsureSysmemReadyOnInput() with zero input buffers added so far. The StreamProcessor
// protocol allows sending QueueInputFormatDetails and QueueInputEndOfStream before any AddBuffer
// for INPUT has been sent. This mechanism basically lets AddBuffer skip ahead of
// QueueInputFormatDetails and QueueInputEndOfStream, without allowing AddBuffer to slip any later
// wrt StreamControl.
void CodecImpl::RunAnySysmemCompletionsOrWait(ScopedLock& lock) {
  // If any sysmem completions ran, we immediately return, so that conditions
  // can be checked again in the caller immediately.
  ZX_DEBUG_ASSERT(IsStreamControl());
  bool any_completions_ran = RunAnySysmemCompletions(lock);
  ZX_DEBUG_ASSERT(sysmem_completion_queue_.empty());
  if (!any_completions_ran) {
    // We know sysmem_completion_queue_.empty() and the lock is held just before
    // this wait().
    lock.AssertHeld(lock_);
    wake_stream_control_condition_.wait(lock.unique_lock());
  }
}

// This is called on Output ordering domain (FIDL thread) any time a message is
// received which would be able to start a new stream.
//
// More complete protocol validation happens on StreamControl ordering domain.
// The validation here is just to validate to degree needed to not break our
// stream_queue_ and future_stream_lifetime_ordinal_.
bool CodecImpl::EnsureFutureStreamSeenLocked(uint64_t stream_lifetime_ordinal) {
  if (future_stream_lifetime_ordinal_ == stream_lifetime_ordinal) {
    return true;
  }
  if (stream_lifetime_ordinal < future_stream_lifetime_ordinal_) {
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolFailure);
    FailLocked("stream_lifetime_ordinal went backward - exiting\n");
    return false;
  }
  ZX_DEBUG_ASSERT(stream_lifetime_ordinal > future_stream_lifetime_ordinal_);
  if (future_stream_lifetime_ordinal_ % 2 == 1) {
    if (!EnsureFutureStreamCloseSeenLocked(future_stream_lifetime_ordinal_)) {
      return false;
    }
  }
  future_stream_lifetime_ordinal_ = stream_lifetime_ordinal;

  // The lock_ is held, protecting stream_queue_ against concurrent modification by StreamControl
  // domain.
  stream_queue_.push_back(std::make_unique<Stream>(this, stream_lifetime_ordinal));
  if (stream_queue_.size() > kMaxInFlightStreams) {
    LogEvent(
        media_metrics::
            StreamProcessorEvents2MigratedMetricDimensionEvent_MaxInFlightStreamsExceededError);
    FailLocked(
        "kMaxInFlightStreams reached - clients capable of causing this are "
        "instead supposed to wait/postpone to prevent this from occurring - "
        "exiting\n");
    return false;
  }
  return true;
}

// This is called on Output ordering domain (FIDL thread) any time a message is
// received which would close a stream.
//
// More complete protocol validation happens on StreamControl ordering domain.
// The validation here is just to validate to degree needed to not break our
// stream_queue_ and future_stream_lifetime_ordinal_.
bool CodecImpl::EnsureFutureStreamCloseSeenLocked(uint64_t stream_lifetime_ordinal) {
  if (future_stream_lifetime_ordinal_ % 2 == 0) {
    // Already closed.
    if (stream_lifetime_ordinal != future_stream_lifetime_ordinal_ - 1) {
      LogEvent(
          media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
      FailLocked(
          "CloseCurrentStream() seen with stream_lifetime_ordinal != "
          "most-recent seen stream");
      return false;
    }
    return true;
  }
  if (stream_lifetime_ordinal != future_stream_lifetime_ordinal_) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("attempt to close a stream other than the latest seen stream");
    return false;
  }
  ZX_DEBUG_ASSERT(stream_lifetime_ordinal == future_stream_lifetime_ordinal_);

  // The lock_ is held, ensuring closing_stream stays alive until after call to
  // SetFutureDiscarded().
  if (stream_queue_.empty()) {
    // Latest stream already failed and removed by StreamControl domain.
    return true;
  }
  ZX_DEBUG_ASSERT(!stream_queue_.empty());
  Stream* closing_stream = stream_queue_.back().get();
  ZX_DEBUG_ASSERT(closing_stream->stream_lifetime_ordinal() == stream_lifetime_ordinal);
  // It is permitted to see a FlushEndOfStreamAndCloseStream() before a CloseCurrentStream(). This
  // can make sense if a client just wants to inform the server of all stream closes (despite the
  // redundancy in this case), or if the client wants to release_input_buffers or
  // release_output_buffers after the flush is done.
  //
  // If we didn't previously flush, then this close is discarding.
  closing_stream->AssertHeld(this);
  if (!closing_stream->future_flush_end_of_stream()) {
    closing_stream->SetFutureDiscarded();
  }

  future_stream_lifetime_ordinal_++;
  ZX_DEBUG_ASSERT(future_stream_lifetime_ordinal_ % 2 == 0);
  return true;
}

// This is called on Output ordering domain (FIDL thread) any time a flush is
// seen.
//
// More complete protocol validation happens on StreamControl ordering domain.
// The validation here is just to validate to degree needed to not break our
// stream_queue_ and future_stream_lifetime_ordinal_.
bool CodecImpl::EnsureFutureStreamFlushSeenLocked(uint64_t stream_lifetime_ordinal) {
  if (stream_lifetime_ordinal != future_stream_lifetime_ordinal_) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("FlushCurrentStream() stream_lifetime_ordinal inconsistent");
    return false;
  }

  // The lock_ is held, ensuring flushing_stream stays alive until after call to
  // SetFutureFlushEndOfStream().
  if (stream_queue_.empty()) {
    // Latest stream already failed and removed by StreamControl domain.
    return false;
  }
  ZX_DEBUG_ASSERT(!stream_queue_.empty());
  Stream* flushing_stream = stream_queue_.back().get();
  // Thanks to the above future_stream_lifetime_ordinal_ check, we know the
  // future stream is not discarded yet.
  flushing_stream->AssertHeld(this);
  ZX_DEBUG_ASSERT(!flushing_stream->future_discarded());
  flushing_stream->AssertHeld(this);
  if (flushing_stream->future_flush_end_of_stream()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_ClientProtocolError);
    FailLocked("FlushCurrentStream() used twice on same stream");
    return false;
  }

  // We don't future-verify that we have a QueueInputEndOfStream(). We'll verify
  // that later when StreamControl catches up to this stream.

  // Remember the flush so we later know that a close doesn't imply discard.
  flushing_stream->SetFutureFlushEndOfStream();

  // A FlushEndOfStreamAndCloseStream() is also a close, after the flush.  This
  // keeps future_stream_lifetime_ordinal_ consistent.
  if (!EnsureFutureStreamCloseSeenLocked(stream_lifetime_ordinal)) {
    return false;
  }
  return true;
}

// This method is only called when buffer_constraints_action_required will be
// true in an OnOutputConstraints() message sent shortly after this method call.
//
// Even if the client is switching streams rapidly without configuring output,
// this method and GenerateAndSendNewOutputConstraints() with
// buffer_constraints_action_required true always run in pairs.
//
// If the client is in the middle of configuring output, we'll start ignoring
// the client's messages re. the old buffer_lifetime_ordinal and old
// buffer_constraints_version_ordinal until the client catches up to the new
// last_required_buffer_constraints_version_ordinal_[kOutputPort].
void CodecImpl::StartIgnoringClientOldOutputConfig(ScopedLock& lock) {
  ZX_DEBUG_ASSERT(IsFidl());

  // The buffer_lifetime_ordinal_[kOutputPort] can be even on entry due to at
  // least two cases: 0, and when the client is switching streams repeatedly
  // without setting a new buffer_lifetime_ordinal_[kOutputPort].
  if (buffer_lifetime_ordinal_[kOutputPort] % 2 == 1) {
    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[kOutputPort] % 2 == 1);
    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[kOutputPort] ==
                    port_settings_[kOutputPort]->buffer_lifetime_ordinal());
    buffer_lifetime_ordinal_[kOutputPort]++;
    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[kOutputPort] % 2 == 0);
    ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[kOutputPort] ==
                    port_settings_[kOutputPort]->buffer_lifetime_ordinal() + 1);
  }

  // When buffer_constraints_action_required true, we can assert in
  // GenerateAndSendNewOutputConstraints() that this value is still the
  // next_output_buffer_constraints_version_ordinal_ in that method.
  last_required_buffer_constraints_version_ordinal_[kOutputPort] =
      next_output_buffer_constraints_version_ordinal_;
  if (snapped_buffer_constraints_version_ordinal_[kOutputPort].has_value() &&
      snapped_buffer_constraints_version_ordinal_[kOutputPort]
              ->buffer_constraints_version_ordinal() <
          last_required_buffer_constraints_version_ordinal_[kOutputPort]) {
    // This isn't necessary for correctness. We don't need these constraints any more now that the
    // new last_required_buffer_constraints_version_ordinal_[kOutputPort] will prevent any asking
    // for the old snapped constraints.
    snapped_buffer_constraints_version_ordinal_[kOutputPort].reset();
  }
}

void CodecImpl::TryFillDynamicStreamBufferConstraintsFields(
    CodecPort port, fuchsia::media::StreamBufferConstraints& stream_buffer_constraints) {
  // We may not get constraints this way, and we prefer not to fall back to
  // CoreCodecGetBufferCollectionConstraints2 here because that'd be checken-and-egg between
  // StreamBufferConstraints and a parameter to CoreCodecGetBufferCollectionConstraints2. The fields
  // we're filling out using this info only need to be filled out if supports_dynamic_buffers, and
  // supports_dynamic_buffers implies the CodecAdapter implements
  // CoreCodecGetBufferCollectionConstraints3, so when we need to fill out the fields we'll get the
  // info we need. As more CodecAdapters implement CoreCodecGetBufferCollectionConstraints3 we'll
  // fill out the fields in more cases (supports_dynamic_buffers or not).
  auto maybe_constraints_and_version = CoreCodecGetBufferCollectionConstraints3(port);
  ZX_DEBUG_ASSERT(!is_supports_dynamic_buffers_ || maybe_constraints_and_version.has_value());
  if (maybe_constraints_and_version.has_value()) {
    auto& buffer_collection_constraints = maybe_constraints_and_version->constraints;
    ZX_DEBUG_ASSERT(buffer_collection_constraints.min_buffer_count_for_camping().has_value());
    stream_buffer_constraints.set_buffer_count_for_server_current(
        std::max(*buffer_collection_constraints.min_buffer_count_for_camping(),
                 buffer_collection_constraints.min_buffer_count().has_value()
                     ? *buffer_collection_constraints.min_buffer_count()
                     : 1) +
        (buffer_collection_constraints.min_buffer_count_for_dedicated_slack().has_value()
             ? *buffer_collection_constraints.min_buffer_count_for_dedicated_slack()
             : 0));
    if (buffer_collection_constraints.image_format_constraints().has_value()) {
      ZX_DEBUG_ASSERT(!buffer_collection_constraints.image_format_constraints()->empty());
      auto& format_zero = buffer_collection_constraints.image_format_constraints()->at(0);
      ZX_DEBUG_ASSERT(format_zero.required_min_size().has_value());
      // intentional clone, just for clarity
      auto min_size = *format_zero.required_min_size();
      if (format_zero.size_alignment().has_value()) {
        // This increases the chance that stream_buffer_constraints.size will match the 2d size
        // sysmem ends up allocating, though this isn't sufficient to guarantee they'll match in all
        // potential situations, depending on other fields and other participants. The CodecAdapter
        // should not rely on sysmem to necessarily allocate buffer(s) with `size` equal to the
        // `min_size` computed here.
        auto& size_alignment = *format_zero.size_alignment();
        min_size.width() = fbl::round_up(min_size.width(), size_alignment.width());
        min_size.height() = fbl::round_up(min_size.height(), size_alignment.height());
      }
      stream_buffer_constraints.set_size({min_size.width(), min_size.height()});
      ZX_DEBUG_ASSERT(format_zero.pixel_format().has_value());
      auto& pixel_format = *format_zero.pixel_format();
      stream_buffer_constraints.set_pixel_format(
          fuchsia::images2::PixelFormat{static_cast<uint32_t>(pixel_format)});
    }
  }
}

void CodecImpl::GenerateAndSendNewOutputConstraints(ScopedLock& lock,
                                                    std::shared_ptr<PausedOutput> paused_output) {
  ZX_DEBUG_ASSERT(IsStreamControl() || IsFidl());
  lock.AssertHeld(lock_);

  uint64_t current_stream_lifetime_ordinal = stream_lifetime_ordinal_;
  uint64_t new_output_buffer_constraints_version_ordinal =
      next_output_buffer_constraints_version_ordinal_++;

  // If buffer_constraints_action_required true, the caller bumped the
  // last_required_buffer_constraints_version_ordinal_[kOutputPort] before
  // calling this method (using StartIgnoringClientOldOutputConfig()), to
  // ensure any output config messages from the client are ignored until the
  // client catches up to at least
  // last_required_buffer_constraints_version_ordinal_.
  ZX_DEBUG_ASSERT(last_required_buffer_constraints_version_ordinal_[kOutputPort] ==
                  new_output_buffer_constraints_version_ordinal);

  auto output_constraints = std::make_unique<fuchsia::media::StreamOutputConstraints>();
  output_constraints->set_stream_lifetime_ordinal(current_stream_lifetime_ordinal);
  output_constraints->set_buffer_constraints_action_required(true);
  auto& buffer_constraints = *output_constraints->mutable_buffer_constraints();
  buffer_constraints.set_buffer_constraints_version_ordinal(
      new_output_buffer_constraints_version_ordinal);
  {  // scope unlock
    ScopedUnlock unlock(*this);
    TryFillDynamicStreamBufferConstraintsFields(kOutputPort, buffer_constraints);
  }
  lock.AssertHeld(lock_);

  // We only call GenerateAndSendNewOutputConstraints() from contexts that won't
  // be changing the stream_lifetime_ordinal_, so the fact that we released the
  // lock above doesn't mean the stream_lifetime_ordinal_ could have changed, so
  // we can assert here that it's still the same as above.
  ZX_DEBUG_ASSERT(current_stream_lifetime_ordinal == stream_lifetime_ordinal_);

  output_constraints_ = std::move(output_constraints);

  // Stay under lock after setting output_constraints_, to get proper ordering
  // of sent messages even if a hostile client deduces the content of this
  // message before we've sent it and manages to get the server to send another
  // subsequent OnOutputConstraints().

  ZX_DEBUG_ASSERT(sent_buffer_constraints_version_ordinal_[kOutputPort] + 1 ==
                  new_output_buffer_constraints_version_ordinal);

  // Setting this within same lock hold interval as we queue the message to be
  // sent in order vs. other OnOutputConstraints() messages.  This way we can
  // verify that the client's incoming messages are not trying to configure with
  // respect to a buffer_constraints_version_ordinal that is newer than we've
  // actually sent the client.
  sent_buffer_constraints_version_ordinal_[kOutputPort] =
      new_output_buffer_constraints_version_ordinal;

  // We snap the most recent CodecAdapter constraints_version we know about at
  // this point. Later, in an AddBuffer failure case, this helps decide whether
  // we can unambiguously blame the client, or we need to let the client catch
  // up to a new buffer_constraints_version_ordinal.
  //
  // This isn't literally "sent", but it corresponds to
  // sent_buffer_constraints_version_ordinal_[kOutputPort], which is sent.
  //
  // This doesn't prevent the CodecAdapter's constraints_version from being a
  // larger value during ParticipateInBufferAllocation. In that case the client
  // may get a "second chance" in a sense due to this value being more stale
  // than in ParticipateInBufferAllocation, but if the client blows its second
  // chance as well, then we'll blame the badly-behaving client. We don't snap
  // this in ParticipateInBufferAllocation because it's not a strict protocol
  // requirement for ParticipateInBufferAllocation to be sent on the same
  // StreamProcessor instance as AddBuffer. Typically it will be, but that's not
  // strictly required.
  //
  // During AddBuffer, if GetVmoInfo constraints check fails, the CodecAdapter's
  // constraints_version hasn't changed since this value, we can blame the
  // client. But if the CodecAdapter's constraints_version has changed since
  // this value, we give the client another chance to catch up.
  last_sent_codec_adapter_output_constraints_version_ =
      last_noticed_codec_adapter_output_constraints_version_;

  lock.AssertHeld(stream_->parent_->lock_);
  // This gets posted to the output queue to be ordered after any previous output. When this runs
  // on the output queue it pauses the output queue until the client has provided new buffers that
  // conform to the new constraints.
  PostStreamOutputLocked([this, output_constraints = fidl::Clone(*output_constraints_),
                          shared_stream_future_discarded = stream_->shared_future_discarded(),
                          paused_output = std::move(paused_output)]() mutable {
    if (IsStopping()) {
      return;
    }
    if (shared_stream_future_discarded->load(std::memory_order_seq_cst)) {
      return;
    }
    // See "is_bound_checks" comment up top.
    if (binding_.is_bound()) {
      binding_.events().OnOutputConstraints(std::move(output_constraints));
    }
    // Else the current lambda wouldn't be running.
    ZX_DEBUG_ASSERT(!maybe_weak_paused_output_.has_value());
    // shared_ptr -> weak_ptr; this is where the pausing of output actually starts. Unpause will
    // happen as soon as the client successfully configures output with new buffers (to degree
    // required by is_dynamic_buffers_), or as soon as StreamControl ordering domain deletes the
    // PausedOutput instance
    maybe_weak_paused_output_ = paused_output;
    // ~paused_output - the StreamControl ordering domain _may_ still have a shared_ptr to the
    // PausedOutput instance (in the still-connected and success path, it does)
  });
}

void CodecImpl::MidStreamOutputConstraintsChange(uint64_t stream_lifetime_ordinal) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  VLOGF("CodecImpl::MidStreamOutputConstraintsChange - stream: %lu", stream_lifetime_ordinal);
  {  // scope lock
    ScopedLock lock(lock_);
    VLOGF("lock aquired 1");
    if (stream_lifetime_ordinal < stream_lifetime_ordinal_) {
      // ignore; The codec_adapter_meh_output_buffer_constraints_version_ordinal_ took care of it.
      VLOGF("CodecImpl::MidStreamOutputConstraintsChange - stale stream");
      return;
    }
    ZX_DEBUG_ASSERT(stream_lifetime_ordinal == stream_lifetime_ordinal_);
    ZX_DEBUG_ASSERT(stream_);

    // We can work through the mid-stream output constraints change step by step
    // using this thread.

    // This is what starts the interval during which we'll ignore any
    // in-progress client output config until the client catches up.
    VLOGF("StartIngoringClientOldOutputConfig()...");
    if (!RunSyncOnSharedFidlForStream(lock, [this] {
          ScopedLock lock(lock_);
          StartIgnoringClientOldOutputConfig(lock);
        })) {
      ZX_DEBUG_ASSERT(IsStoppingLocked());
      VLOGF("CodecImpl::MidStreamOutputConstraintsChange IsStoppingLocked() (1)");
      return;
    }
    lock.AssertHeld(stream_->parent_->lock_);
    if (stream_->future_discarded()) {
      VLOGF("CodecImpl::MidStreamOutputConstraintsChange stream_->future_discarded() (1)");
      return;
    }

    if (!is_supports_dynamic_buffers_) {
      ScopedUnlock unlock(*this);
      VLOGF("CoreCodecMidStreamOutputBufferReConfigPrepare()...");
      CoreCodecMidStreamOutputBufferReConfigPrepare();
    }
    lock.AssertHeld(lock_);

    // When is_supports_dynamic_buffers_ && !*is_dynamic_buffers_[kOutputPort], the CodecAdapter can
    // generate output as soon as it has a single output buffer, but output using the new buffers is
    // not permitted until CompleteOutputBufferPartialSettings. While this case is the only reason
    // we pause output, we go ahead and pause output regardless of the values of
    // is_supports_dynamic_buffers_ and *is_dynamic_buffers_[kOutputPort] to get more coverage on
    // the pausing/unpausing mechanism.
    //
    // If by the time we unpause, IsStopping() is already true or stream_->future_discarded() is
    // already true, the queued output closures will notice that directly and avoid sending the
    // output. This is important because in those cases, the client may not have ever sent
    // CompleteOutputBufferPartialSettings, so in that case we can't send stream output subsequent
    // to OnOutputConstraints to the client. If the CodecImpl isn't stopping and it's just the
    // stream that's future_discarded() true, then the client's new stream can receive output (the
    // lack of CompleteOutputBufferPartialSettings wrt this stream doesn't prevent output from a new
    // stream).
    //
    // This doesn't actually pause the output until/unless installed as a weak ptr in
    // maybe_weak_paused_output_. In the success path, this occurs just after OnOutputConstraints is
    // sent, as that message must be ordered after output so far, but we can't send any further
    // output until output is configured again via client setting up new output buffers.
    auto paused_output = std::make_shared<PausedOutput>(*this);

    if (!RunSyncOnSharedFidlForStream(
            lock, [this, paused_output = std::move(paused_output)]() mutable {
              ScopedLock lock(lock_);
              VLOGF("EnsureBuffersNotConfigured()...");
              EnsureBuffersNotConfigured(lock, kOutputPort, false);
              VLOGF("GenerateAndSendNewOutputConstraints()...");
              GenerateAndSendNewOutputConstraints(lock, std::move(paused_output));
            })) {
      ZX_DEBUG_ASSERT(IsStoppingLocked());
      VLOGF("CodecImpl::MidStreamOutputConstraintsChange IsStoppingLocked() (2)");
      return;
    }
    if (stream_->future_discarded()) {
      VLOGF("CodecImpl::MidStreamOutputConstraintsChange stream_->future_discarded() (2)");
      return;
    }

    // Now we can wait for the client to catch up to the current output constraints or for the
    // client to tell the server to discard the current stream.
    //
    // This runs completions for WaitForAllBuffersAllocated in the case of
    // SetInputBufferPartialSettings or SetOutputbufferPartialSettings.
    VLOGF("RunAnySysmemCompletionsOrWait()...");
    stream_->AssertHeld(this);
    while (!IsStoppingLocked() && !stream_->future_discarded() && !IsOutputConfiguredLocked()) {
      RunAnySysmemCompletionsOrWait(lock);
    }
    if (IsStoppingLocked()) {
      VLOGF("CodecImpl::MidStreamOutputConstraintsChange IsStoppingLocked() (3)");
      return;
    }
    if (stream_->future_discarded()) {
      VLOGF("CodecImpl::MidStreamOutputConstraintsChange stream_->future_discarded() (3)");
      return;
    }

    // ~paused_output will un-pause output (including kick to send any queued output). When
    // !*is_dynamic_buffers_[kOutputPort], IsOutputConfiguredLocked() means
    // CompleteOutputBufferPartialSettings has been received (the need to wait until that message to
    // send output using the new buffers when is_supports_dynamic_buffers_ is the only reason we
    // pause output in the first place). If !IsOutputConfiguredLocked(), we know that
    // IsStoppingLocked() || stream_->future_discarded(), and either of those will prevent the
    // output despite unpausing here.
  }  // ~lock

  if (!is_supports_dynamic_buffers_) {
    VLOGF("CoreCodecMidStreamOutputBufferReConfigFinish()...");
    CoreCodecMidStreamOutputBufferReConfigFinish();
  }

  VLOGF("Done with mid-stream format change.");
}

bool CodecImpl::FixupBufferCollectionConstraintsLocked(
    CodecPort port, fuchsia_sysmem2::BufferCollectionConstraints* buffer_collection_constraints) {
  if (!buffer_collection_constraints->usage().has_value()) {
    buffer_collection_constraints->usage().emplace();
  }
  fuchsia_sysmem2::BufferUsage& usage = *buffer_collection_constraints->usage();

  if (IsCoreCodecMappedBufferUseful(port)) {
    // Not surprisingly, both decoders and encoders read from input and write to
    // output.
    if (port == kInputPort) {
      uint32_t cpu_usage = 0;
      if (usage.cpu().has_value()) {
        cpu_usage = *usage.cpu();
      }
      if (cpu_usage & ~(fuchsia_sysmem2::kCpuUsageRead | fuchsia_sysmem2::kCpuUsageReadOften)) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
        FailLocked("Core codec set disallowed CPU usage bits (input port).");
        return false;
      }
      if (!IsPortSecureRequired(kInputPort)) {
        if (!usage.cpu().has_value()) {
          usage.cpu().emplace(0);
        }
        *usage.cpu() |= fuchsia_sysmem2::kCpuUsageRead | fuchsia_sysmem2::kCpuUsageReadOften;
      } else {
        usage.cpu().reset();
      }
    } else {
      uint32_t cpu_usage = 0;
      if (usage.cpu().has_value()) {
        cpu_usage = *usage.cpu();
      }
      if (cpu_usage & ~(fuchsia_sysmem2::kCpuUsageWrite | fuchsia_sysmem2::kCpuUsageWriteOften)) {
        LogEvent(
            media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
        FailLocked("Core codec set disallowed CPU usage bit(s) (output port).");
        return false;
      }
      if (!IsPortSecureRequired(kOutputPort)) {
        if (!usage.cpu().has_value()) {
          usage.cpu().emplace(0);
        }
        *usage.cpu() |= fuchsia_sysmem2::kCpuUsageWrite | fuchsia_sysmem2::kCpuUsageWriteOften;
      } else {
        usage.cpu().reset();
      }
    }
  } else {
    if (usage.cpu().has_value()) {
      LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
      FailLocked("Core codec set usage.cpu despite !IsCoreCodecMappedBufferUseful()");
      return false;
    }
    // The CPU won't touch the buffers at all.
    usage.cpu().reset();
  }
  if (usage.vulkan().has_value()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
    FailLocked("Core codec set usage.vulkan bits");
    return false;
  }
  ZX_DEBUG_ASSERT(!usage.vulkan().has_value());
  if (usage.display().has_value()) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
    FailLocked("Core codec set usage.display bits");
    return false;
  }
  ZX_DEBUG_ASSERT(!usage.display().has_value());
  if (IsDecryptor()) {
    // DecryptorAdapter should not be setting video usage bits.
    if (usage.video().has_value()) {
      LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
      FailLocked("Core codec set disallowed video usage bits for decryptor");
      return false;
    }
    if (port == kOutputPort) {
      usage.video().emplace(fuchsia_sysmem2::kVideoUsageDecryptorOutput);
    }
  } else if (IsCoreCodecHwBased(port)) {
    // Let's see if we can deprecate videoUsageHwProtected, since it's redundant
    // with secure_required.
    uint32_t video_usage = 0;
    if (usage.video().has_value()) {
      video_usage = *usage.video();
    }
    uint32_t allowed_video_usage_bits =
        IsDecoder() ? fuchsia_sysmem2::kVideoUsageHwDecoder : fuchsia_sysmem2::kVideoUsageHwEncoder;
    if (video_usage & ~allowed_video_usage_bits) {
      LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
      FailLocked(
          "Core codec set disallowed video usage bit(s) - port: %d, usage: "
          "0x%08x, allowed: 0x%08x",
          port, video_usage, allowed_video_usage_bits);
      return false;
    }
    if (IsDecoder()) {
      if (!usage.video().has_value()) {
        usage.video().emplace(0);
      }
      *usage.video() |= fuchsia_sysmem2::kVideoUsageHwDecoder;
    } else if (IsEncoder()) {
      if (!usage.video().has_value()) {
        usage.video().emplace(0);
      }
      *usage.video() |= fuchsia_sysmem2::kVideoUsageHwEncoder;
    }
  } else {
    // Despite being a video decoder or encoder, a SW decoder or encoder doesn't
    // count as videoUsageHwDecoder or videoUsageHwEncoder.
    usage.video().reset();
  }

  if (!buffer_collection_constraints->min_buffer_count_for_camping().has_value() ||
      buffer_collection_constraints->min_buffer_count_for_camping().value() < 1) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
    FailLocked("Core codec set min_buffer_count_for_camping to 0; must set to at least 1.");
    return false;
  }

  if (!buffer_collection_constraints->buffer_memory_constraints().has_value()) {
    // Leaving all fields set to their defaults is fine if that's really true, but this encourages
    // CodecAdapter implementations to set fields in here.
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
    FailLocked("Core codec must set has_buffer_memory_constraints");
    return false;
  }
  fuchsia_sysmem2::BufferMemoryConstraints& buffer_memory_constraints =
      buffer_collection_constraints->buffer_memory_constraints().value();

  // Sysmem will fail the BufferCollection if the core codec provides constraints that are
  // inconsistent, but we need to check here that the core codec is being consistent with
  // SecureMemoryMode, since sysmem doesn't know about SecureMemoryMode.  Essentially
  // SecureMemoryMode translates into secure_required and secure_permitted in sysmem.  The former
  // is just a bool.  The latter is indicated by listing at least one secure heap.

  // secure_required consistency check
  //
  // CoreCodecSetSecureMemoryMode() informed the core codec of the mode previously.
  bool secure_required = false;
  if (buffer_memory_constraints.secure_required().has_value()) {
    secure_required = *buffer_memory_constraints.secure_required();
  }
  if (!!IsPortSecureRequired(port) != !!secure_required) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
    FailLocked("Core codec secure_required inconsistent with SecureMemoryMode");
    return false;
  }

  // secure_permitted consistency check
  //
  // If secure is permitted, then the core codec must support at least one non-SYSTEM_RAM heap, as
  // specifying support for a secure heap is how sysmem knows secure_permitted.  We can't directly
  // tell that the non-RAM heap is secure, so this is an approximate check.  In any case
  // secure_required by any sysmem participant will be enforced by sysmem with respect to specific
  // heaps and whether they're secure.  The approximate-ness is ok since this only comes from
  // in-proc, so the check is just for trying to notice if the core codec is filling out
  // inconsistent constraints in a way that sysmem wouldn't otherwise notice.
  bool is_non_ram_heap_found = false;
  if (buffer_memory_constraints.permitted_heaps().has_value()) {
    for (uint32_t iter = 0; iter < buffer_memory_constraints.permitted_heaps()->size(); ++iter) {
      if (buffer_memory_constraints.permitted_heaps()->at(iter).heap_type().value() !=
          bind_fuchsia_sysmem_heap::HEAP_TYPE_SYSTEM_RAM) {
        is_non_ram_heap_found = true;
        break;
      }
    }
  }
  if (IsPortSecurePermitted(port) && !is_non_ram_heap_found) {
    LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_UnreachableError);
    FailLocked("Core codec must specify at least one non-RAM heap when secure_required");
    return false;
  }

  if (port == kOutputPort && is_force_output_buffers_fixed_image_size_ &&
      buffer_collection_constraints->image_format_constraints().has_value()) {
    // The CodecAdapter is the producer, so we could tell the CodecAdapter to force new buffers if
    // the image size changes, or tell the CodecAdapter to generate constraints that have min equal
    // max for both width and height, but we can do that here instead to avoid the hassle for each
    // CodecAdapter, since we know that each CodecAdapter will set the min to the needed size.
    auto& all_image_constraints = *buffer_collection_constraints->image_format_constraints();
    for (auto& image_constraints : all_image_constraints) {
      if (!image_constraints.min_size().has_value()) {
        image_constraints.min_size() = {64, 64};
      }
      // Unconditionally set/replace max_size with min_size provided by CodecAdapter (or the default
      // potentially set just above).
      //
      // intentional copy/clone
      image_constraints.max_size() = *image_constraints.min_size();
    }
  }

  // The rest of the constraints are entirely up to the core codec, and it's up to the core codec
  // to specify self-consistent constraints.  Sysmem will perform additional consistency checks on
  // the constraints.

  return true;
}

void CodecImpl::SendFreeInputPacketLocked(fuchsia::media::PacketHeader header) {
  // We allow calling this method on StreamControl or core codec/InputData
  // ordering domain.
  ZX_DEBUG_ASSERT(IsStreamControl() || IsCoreCodec());

  auto& protocol_packets_by_ordinal = protocol_packets_by_protocol_packet_index_[kInputPort];
  auto protocol_packets_by_ordinal_iter =
      protocol_packets_by_ordinal.find(header.buffer_lifetime_ordinal());
  if (protocol_packets_by_ordinal_iter != protocol_packets_by_ordinal.end()) {
    auto& protocol_packets_by_index = protocol_packets_by_ordinal_iter->second;
    protocol_packets_by_index.erase(header.packet_index());
  }

  // We only send using fidl ordering domain.
  PostToSharedFidl([this, header = std::move(header)]() mutable {
    // See "is_bound_checks" comment up top.
    if (binding_.is_bound()) {
      binding_.events().OnFreeInputPacket(std::move(header));
    }
  });
}

bool CodecImpl::IsInputConfiguredLocked() { return IsPortConfiguredCommonLocked(kInputPort); }

bool CodecImpl::IsOutputConfiguredLocked() {
  if (!IsPortConfiguredCommonLocked(kOutputPort)) {
    return false;
  }
  ZX_DEBUG_ASSERT(port_settings_[kOutputPort]);
  ZX_DEBUG_ASSERT(is_dynamic_buffers_[kOutputPort].has_value());
  if (!*is_dynamic_buffers_[kOutputPort] &&
      !port_settings_[kOutputPort]->is_complete_seen_output()) {
    return false;
  }
  return true;
}

bool CodecImpl::IsPortConfiguredCommonLocked(CodecPort port) {
  // In addition to what we're able to assert here, when
  // is_port_configured_[port], the CodecAdapter also has the port
  // configured.
#if ZX_DEBUG_ASSERT_IMPLEMENTED
  auto* buffers = all_buffers(port);
  ZX_DEBUG_ASSERT(!is_port_configured_[port] || is_dynamic_buffers_[port] ||
                  (buffer_lifetime_ordinal_[port] % 2 == 1) && port_settings_[port] && buffers &&
                      buffers->size() >= port_settings_[port]->min_buffer_count());
#endif
  return is_port_configured_[port];
}

bool CodecImpl::IsPortAtLeastPartiallyConfiguredLocked(CodecPort port) {
  if (IsPortConfiguredCommonLocked(port)) {
    return true;
  }
  if (!port_settings_[port]) {
    return false;
  }
  ZX_DEBUG_ASSERT(port_settings_[port]);
  ZX_DEBUG_ASSERT(buffer_lifetime_ordinal_[port] % 2 == 1);
  return true;
}

void CodecImpl::Fail(const char* format, ...) {
  va_list args;
  va_start(args, format);
  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);
    vFailLocked(false, format, args);
  }  // ~lock
  // "this" can be deallocated by this point (as soon as ~lock above).
  va_end(args);
}

void CodecImpl::FailLocked(const char* format, ...) {
  va_list args;
  va_start(args, format);
  vFailLocked(false, format, args);
  va_end(args);
  // At this point know "this" is still allocated only because we still hold
  // lock_.  As soon as lock_ is released by the caller, "this" can immediately
  // be deallocated by another thread, if this isn't currently the fidl ordering
  // domain.
}

void CodecImpl::FailFatal(const char* format, ...) {
  va_list args;
  va_start(args, format);
  // This doesn't return.
  vFail(true, format, args);
  va_end(args);
}

void CodecImpl::vFail(bool is_fatal, const char* format, va_list args) {
  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);
    vFailLocked(is_fatal, format, args);
  }  // ~lock
}

// Only meant to be called from Fail() and FailLocked().  Only meant to be
// called for async failure cases after was_logically_bound_ has become true.
// Failures before that point are handled separately.
void CodecImpl::vFailLocked(bool is_fatal, const char* format, va_list args) {
  // Let's not have a buffer on the stack, not because it couldn't be done
  // safely, but because we'd potentially run into stack size vs. message length
  // tradeoffs, stack expansion granularity fun, or whatever else.

  va_list args2;
  va_copy(args2, args);

  size_t buffer_bytes = vsnprintf(nullptr, 0, format, args) + 1;

  // ~buffer never actually runs since this method never returns
  std::unique_ptr<char[]> buffer(new char[buffer_bytes]);

  size_t buffer_bytes_2 = vsnprintf(buffer.get(), buffer_bytes, format, args2) + 1;
  (void)buffer_bytes_2;
  // sanity check; should match so go ahead and assert that it does.
  ZX_DEBUG_ASSERT(buffer_bytes == buffer_bytes_2);
  va_end(args2);

  const char* message = is_fatal ? "devhost will fail" : "Codec channel will close async";

  LogEvent(media_metrics::
               StreamProcessorEvents2MigratedMetricDimensionEvent_StreamProcessorFailureAnyReason);
  if (is_fatal) {
    // Default logging to stderr for both driver and non-driver clients
    LOG(ERROR, "%s -- %s", buffer.get(), message);

    abort();
  } else {
    // Default logging to stderr for both driver and non-driver clients
    LOG(WARN, "%s -- %s", buffer.get(), message);

    UnbindLocked();
  }

  // At this point we know "this" is still allocated only because we still hold
  // lock_.  As soon as lock_ is released by the caller, "this" can immediately
  // be deallocated by another thread, if this isn't currently the fidl ordering
  // domain.
}

void CodecImpl::PostSerial(async_dispatcher_t* async, fit::closure to_run) {
  zx_status_t result = async::PostTask(async, std::move(to_run));
  ZX_ASSERT(result == ZX_OK);
}

// The implementation of PostToSharedFidl() permits queuing lambdas that use
// "this", despite the fact that the client can call ~CodecImpl at any time
// using the fidl ordering domain.  If ~CodecImpl is called before the lambda
// runs, the lambda will be deleted instead of run, and the deletion will occur
// during ~CodecImpl while essentially all of CodecImpl is still valid (in case
// ~lambda itself touches any of CodecImpl).
void CodecImpl::PostToSharedFidl(fit::closure to_run) {
  // If shared_fidl_queue_.is_stopped(), then to_run will just be deleted here.
  shared_fidl_queue_.Enqueue(std::move(to_run));
}

bool CodecImpl::RunSyncOnSharedFidlForStream(ScopedLock& lock, fit::closure to_run) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  lock.AssertHeld(lock_);
  ZX_DEBUG_ASSERT(stream_);
  ZX_DEBUG_ASSERT(stream_->stream_lifetime_ordinal() == stream_lifetime_ordinal_);
  // The synchronization here is a bit tricky because when ~CodecImpl runs on fidl ordering domain
  // the fidl ordering domain waits for StreamControl ordering domain to exit after IsStopping() is
  // true, so this means any waiting by StreamControl on fidl ordering domain needs to give up on
  // waiting if IsStopping() true. In addition, we want to_run to either not run and get deleted, or
  // run knowing that StreamControl is waiting here for to_run to complete. Once the lambda posted
  // below starts to run, we know we're not running ~CodecImpl on fidl ordering domain so we're ok
  // to unconditionally wait for to_run to be done.
  //
  // Overall, we want to lock in the decision whether to skip running to_run if IsStopping() true
  // wins, or run to_run to completion and wait for it here if to_run wins.
  //
  // Progression is either kUndecided -> kRunAndWait -> kDone, or kUndecided -> kCancelled.
  enum class DecisionStatus : uint32_t {
    // undecided so far
    kUndecided = 0,
    // run to_run to completion and wait for it to be done here
    kRunAndWait = 1,
    // to_run is done
    kDone = 2,
    // to_run will not run and will not be waited on here
    kCancelled = 3,
  };
  auto decision_status = std::make_shared<std::atomic<DecisionStatus>>(DecisionStatus::kUndecided);
  auto& ds = *decision_status;
  PostToSharedFidl([this, stream_lifetime_ordinal = stream_lifetime_ordinal_,
                    to_run = std::move(to_run), decision_status]() mutable {
    auto& ds = *decision_status;
    while (true) {
      DecisionStatus old_value = ds.load();
      DecisionStatus new_value;
      switch (old_value) {
        case DecisionStatus::kUndecided:
          new_value = DecisionStatus::kRunAndWait;
          break;
        case DecisionStatus::kCancelled:
          return;
        default:
          ZX_PANIC("impossible ds value: %u", static_cast<uint32_t>(old_value));
      }
      if (ds.compare_exchange_strong(old_value, new_value)) {
        break;
      }
    }
    ZX_DEBUG_ASSERT(ds.load() == DecisionStatus::kRunAndWait);
    ZX_DEBUG_ASSERT(stream_lifetime_ordinal == stream_lifetime_ordinal_);
    ZX_DEBUG_ASSERT(stream_);
    ZX_DEBUG_ASSERT(stream_lifetime_ordinal_ == stream_->stream_lifetime_ordinal());
    std::move(to_run)();
    {
      ScopedLock lock(lock_);
      ds.store(DecisionStatus::kDone);
      wake_stream_control_condition_.notify_all();
    }
  });
  while (!IsStoppingLocked() && ds.load() != DecisionStatus::kDone) {
    wake_stream_control_condition_.wait(lock.unique_lock());
  }
  if (IsStoppingLocked()) {
    // attempt cancel, or failing that, wait for to_run to be done
    while (true) {
      DecisionStatus old_value = ds.load();
      switch (old_value) {
        case DecisionStatus::kUndecided:
          if (ds.compare_exchange_strong(old_value, DecisionStatus::kCancelled)) {
            ZX_DEBUG_ASSERT(ds.load() == DecisionStatus::kCancelled);
            return false;
          }
          continue;
        case DecisionStatus::kRunAndWait:
          wake_stream_control_condition_.wait(lock.unique_lock());
          continue;
        case DecisionStatus::kDone:
          ZX_DEBUG_ASSERT(ds.load() == DecisionStatus::kDone);
          return true;
        default:
          ZX_PANIC("impossible ds value: %u", static_cast<uint32_t>(old_value));
      }
    }
  }
  ZX_DEBUG_ASSERT(ds.load() == DecisionStatus::kDone);
  return true;
}

// The implementation of PostToStreamControl() doesn't strongly need to guard
// against ~CodecImpl because ~CodecImpl will do
// stream_control_loop_.Shutdown(), which deletes any tasks that haven't already
// run on StreamControl.  We use a ClosureQueue anyway, for at least a couple
// reasons.
//
// Not very importantly, by using a ClosureQueue here, we eliminate a window
// between is_stream_control_done_ = true and the lambda posted to FIDL thread
// shortly after that, during which hypothetically many FIDL dispatches could
// queue to StreamControl without them being consumed by StreamControl.
//
// More importantly, assuming we add an over-full threshold detection to
// ClosureQueue, that can help avoid the server being overwhelmed by a
// badly-behaving client that queues more messages than make any sense given the
// StreamProcessor protocol (which overall limits the number of concurrent
// messages that are allowed / make any sense, but any given message isn't
// necessarily checked for making sense until we're on StreamControl).
void CodecImpl::PostToStreamControl(fit::closure to_run) {
  // If stream_control_queue_.is_stopped(), then to_run will just be deleted
  // here.
  stream_control_queue_.Enqueue(std::move(to_run));
}

void CodecImpl::PostToStreamControlForOutput(fit::closure to_run) {
  bool is_post_needed;
  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);
    bool expected = false;
    is_post_needed = stream_control_for_output_queue_pending_.compare_exchange_strong(
        expected, true, std::memory_order_seq_cst);
    stream_control_for_output_queue_.emplace(std::move(to_run));
  }  // ~lock
  // This wake is in case StreamControl is waiting for output EOS, and should
  // run to_run before that wait for output EOS completes.
  wake_stream_control_condition_.notify_all();
  // We also post, which is the more common way that to_run gets called.
  if (!is_post_needed) {
    return;
  }
  stream_control_queue_.Enqueue([this] { ProcessStreamControlForOutputQueue(); });
}

void CodecImpl::ProcessStreamControlForOutputQueue() {
  stream_control_for_output_queue_pending_.store(false, std::memory_order_seq_cst);
  while (true) {
    fit::closure to_run;
    {  // scope lock
      std::lock_guard<std::mutex> lock(lock_);
      if (stream_control_for_output_queue_.empty()) {
        return;
      }
      to_run = std::move(stream_control_for_output_queue_.front());
      stream_control_for_output_queue_.pop();
    }  // ~lock
    std::move(to_run)();
  }
}

void CodecImpl::PostStreamOutputLocked(fit::closure to_run) {
  // to_run must get called after any previously-posted PostToSharedFidl closures, but can get
  // called after a subsequently-posted PostToSharedFidl closure if output is paused
  //
  // At least one reason for the above no-calling-before rule is OnFreeInputPacket needing to get
  // called before OnOutputTimestampHasNoOutput (per OnOutputTimestampHasNoOutput docs), and to
  // avoid adding more orderings that a StreamProcessor client would need to handle).
  VLOGF("PostStreamOutputLocked top");

  // Any known ordering domain is fine. Arbitrary threads are not necessarily fine, so disallow
  // unknown threads.
  ZX_DEBUG_ASSERT(IsStreamControl() || IsFidl() || IsCoreCodec());

  // We intentionally do not require stream_ != nullptr here, because we need RemoveBuffer responses
  // to order after any prior stream output, regardless of whether the previous stream is still
  // active server-side.

  // The common case is that output is not currently paused and no prior PostStreamOutputLocked is
  // still in flight, so go ahead and send to_run to shared fidl unconditionally here; there is no
  // risk of queueing too many as output buffers must be recycled before reuse. This will decide on
  // fidl ordering domain whether to actually send output yet.
  //
  // This is called with lock_ held (at least for now); this isn't a potential thread ping-pong when
  // the CodecAdapter is sharing the core fidl dispatcher.
  PostToSharedFidl([this, to_run = std::move(to_run)]() mutable {
    VLOGF("PostStreamOutputLocked posted task top");
    // This queue and pausing of output allows to_run to move later than subsequent direct uses of
    // PostToSharedFidl, but not before prior direct uses of PostToSharedFidl.
    output_queue_.emplace(std::move(to_run));
    while (true) {
      if (maybe_weak_paused_output_.has_value()) {
        if (maybe_weak_paused_output_->lock()) {
          // Later PostStreamOutputLocked([]{}) will get called, after the associated shared_ptr is
          // reset().
          return;
        }
        maybe_weak_paused_output_.reset();
      }
      if (output_queue_.empty()) {
        return;
      }
      auto local_to_run = std::move(output_queue_.front());
      output_queue_.pop();
      ZX_DEBUG_ASSERT(!!local_to_run);
      std::move(local_to_run)();
      // local_to_run may have set maybe_weak_paused_output_ at this point
    }
  });
}

bool CodecImpl::IsStoppingLocked() {
  // The only stores are under lock_, so we can rely on any other thread having released lock_
  // after the store, and this thread having acquired the lock_, to do a relaxed load here.
  return was_unbind_started_.load(std::memory_order_relaxed);
}

bool CodecImpl::IsStopping() {
  // The only stores are seq_cst, so we don't need to acquire lock_ to see any prior store.
  return was_unbind_started_.load(std::memory_order_seq_cst);
}

bool CodecImpl::IsDecoder() const { return params_.index() == 0; }

bool CodecImpl::IsEncoder() const { return params_.index() == 1; }

bool CodecImpl::IsDecryptor() const { return params_.index() == 2; }

const fuchsia::mediacodec::CreateDecoder_Params& CodecImpl::decoder_params() const {
  ZX_DEBUG_ASSERT(IsDecoder());
  return std::get<fuchsia::mediacodec::CreateDecoder_Params>(params_);
}

const fuchsia::mediacodec::CreateEncoder_Params& CodecImpl::encoder_params() const {
  ZX_DEBUG_ASSERT(IsEncoder());
  return std::get<fuchsia::mediacodec::CreateEncoder_Params>(params_);
}

const fuchsia::media::drm::DecryptorParams& CodecImpl::decryptor_params() const {
  ZX_DEBUG_ASSERT(IsDecryptor());
  return std::get<fuchsia::media::drm::DecryptorParams>(params_);
}

void CodecImpl::LogEvent(
    media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent event_code) const {
  if (!codec_metrics_) {
    return;
  }
  if (!codec_metrics_implementation_dimension_) {
    return;
  }
  codec_metrics_->LogEvent(codec_metrics_implementation_dimension_.value(), event_code);
}

bool CodecImpl::IsFidl() {
  // caller doesn't need potential error std::string
  //
  // The docs of is_synchronized() are a bit vague; this checks whether this call site is running on
  // the serialized dispatcher associated with checker_fidl_.
  //
  // This checks whether the caller is on the fidl ordering domain, which may optionally be the same
  // ordering domain as the core codec ordering domain; see also SetSharingFidlDomainForCoreCodec.
  return std::holds_alternative<std::monostate>(checker_fidl_.is_synchronized());
}

bool CodecImpl::IsStreamControl() {
  ZX_DEBUG_ASSERT(checker_stream_control_.has_value());
  // caller doesn't need potential error std::string
  //
  // The docs of is_synchronized() are a bit vague; this checks whether this call site is running on
  // the serialized dispatcher associated with checker_stream_control_.
  //
  // This checks whether the caller is on the StreamControl ordering domain.
  return std::holds_alternative<std::monostate>(checker_stream_control_->is_synchronized());
}

bool CodecImpl::IsCoreCodec() {
  // If there are no bugs in calling code, this lock will _never_ be contended and will never need
  // to wait. If there are bugs in calling code, this allows checker_core_codec_ to be captured
  // without (local) UB despite those bugs. This lock doesn't prevent UB elsewhere due to the caller
  // being on the incorrect sequence; that's why we have the checking in the first place.
  if (is_sharing_fidl_domain_for_core_codec_.load(std::memory_order_relaxed)) {
    return IsFidl();
  }
  // When !is_sharing_fidl_domain_for_core_codec_, the client code should call
  // CaptureCoreCodecOrderingDomain asap, in which case checker_core_codec_ will be set.
  std::lock_guard<std::mutex> lock(checker_core_codec_lock_);
  if (checker_core_codec_.has_value()) {
    // The caller just wants the answer, not the potential error std::string for log output. The
    // caller may assert or complain, but the caller doesn't need the std::string in that case as
    // the implications of a "false" return are clear.
    //
    // The docs of is_synchronized() are a bit vague; this checks whether this call site is running
    // on the serialized dispatcher associated with checker_core_codec_.
    //
    // This checks whether the caller is on the core codec ordering domain.
    return std::holds_alternative<std::monostate>(checker_core_codec_->is_synchronized());
  }
  // This is asserting that the not-ideal case of not having checker_core_codec_ (yet?) is at least
  // self-consistent.
  ZX_DEBUG_ASSERT(!is_sharing_fidl_domain_for_core_codec_ &&
                  !is_capture_core_codec_ordering_domain_called_);
  // We don't have the checker_core_codec_ (yet, or potentially ever), so we just check whether it's
  // not any of the other known sequences/ordering domains/threads. This will (unfortunately) return
  // true if IsCoreCodec() is called from some arbitrary unknown thread/sequence that isn't actually
  // the core codec thread/sequence; this is why CaptureCoreCodecOrderingDomain should be called
  // asap so that we can check more robustly (see above) instead.
  //
  // We currently tolerate older CodecAdapter(s) never calling CaptureCoreCodecOrderingDomain, in
  // which case this path continues to be used.
  return !IsStreamControl() && !IsFidl();
}

void CodecImpl::HandlePendingInputFormatDetails() {
  ZX_DEBUG_ASSERT(IsStreamControl());
  const fuchsia::media::FormatDetails* input_details = nullptr;
  if (stream_->input_format_details()) {
    input_details = stream_->input_format_details();
  } else {
    input_details = initial_input_format_details_;
  }
  ZX_DEBUG_ASSERT(input_details);
  CoreCodecQueueInputFormatDetails(*input_details);
}

std::string CodecImpl::GetBufferName(CodecPort port) {
  std::string buffer_name = codec_adapter_->CoreCodecGetName();
  switch (port) {
    case kInputPort:
      buffer_name += "Input";
      break;
    case kOutputPort:
      buffer_name += "Output";
      break;
    default:
      buffer_name += "Unknown";
      break;
  }
  return buffer_name;
}

void CodecImpl::onCoreCodecFailCodec(const char* format, ...) {
  LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_CoreFailureAnyReason);
  std::string local_format = std::string("onCoreCodecFailCodec() called -- ") + format;
  va_list args;
  va_start(args, format);
  vFail(false, local_format.c_str(), args);
  // "this" can be deallocated by this point (as soon as ~lock above).
  va_end(args);
}

void CodecImpl::onCoreCodecFailStream(fuchsia::media::StreamError error) {
  {  // scope lock
    LogEvent(
        media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_StreamFailureAnyReason);
    ScopedLock lock(lock_);
    if (IsStoppingLocked()) {
      // This CodecImpl is already stopping due to a previous FailLocked(),
      // which will result in the Codec channel getting closed soon.  So don't
      // send OnStreamFailed().
      return;
    }

    // We rely on the CodecAdapter and the rest of CodecImpl to only call this method when there's a
    // current stream.
    ZX_DEBUG_ASSERT(stream_ && stream_->stream_lifetime_ordinal() == stream_lifetime_ordinal_);

    if (stream_->output_end_of_stream()) {
      // Tolerate a CodecAdapter failing the stream after output EndOfStream
      // seen, and avoid notifying the client of a stream failure that's too
      // late to matter.
      return;
    }

    if (stream_->failure_seen()) {
      // We already know.  We don't auto-close the stream because the client is
      // in control of stream lifetime, so it's plausible that a CodecAdapter
      // could notify of stream failure more than once.  We can ignore the
      // redundant stream failure to avoid sending OnStreamFailed() again.
      return;
    }
    stream_->SetFailureSeen();
    // Make sure FlushEndOfStreamAndCloseStream_StreamControl doesn't get stuck.
    wake_stream_control_condition_.notify_all();

    if (IsStreamErrorRecoverable(error)) {
      LOG(INFO, "Stream %lu failed: %s. %s", stream_lifetime_ordinal_, ToString(error),
          GetStreamErrorAdditionalHelpText(error));
    } else {
      LOG(ERROR, "Stream %lu failed: %s", stream_lifetime_ordinal_, ToString(error));
    }

    PostToStreamControl([this, stream_lifetime_ordinal = stream_lifetime_ordinal_] {
      ZX_DEBUG_ASSERT(IsStreamControl());
      ScopedLock lock(lock_);
      if (IsStoppingLocked()) {
        return;
      }
      if (stream_lifetime_ordinal != stream_lifetime_ordinal_) {
        ZX_DEBUG_ASSERT(stream_lifetime_ordinal_ > stream_lifetime_ordinal);
        return;
      }
      // CodecImpl will preserve the ordering of onCoreCodecInputPacketDone() and
      // onCoreCodecFailStream() when sending messages to the client.  Some CodecAdapter(s) _may_
      // order these messages to free input packets which were successfully processed before stream
      // failure, and free input packets which were not successfully processed after stream failure.
      //
      // Other CodecAdapter(s) may simply free all pending input packets, either before
      // onCoreCodecFailStream() or after onCoreCodecFailStream(), without any particular ordering
      // with respect to onCoreCodecFailStream().  In particular, a CodecAdapter may free an input
      // packet before that input packet's data is fully processed so that the input packet can be
      // re-filled with new data (a good thing for performance and efficiency), then later the data
      // that was obtained from that input packet may turn out to cause a processing error later.
      // In such a pipelined CodecAdapter, it's not really feasible or desireable for the
      // CodecAdapter to free an input packet only when the input packet has been fully processed
      // into any output it may generate.
      //
      // If a client needs to determine which input packets generated output packets, and possibly
      // also which output packets those are, use of timstamp_ish values is recommended, as the
      // timstamp_ish mechanism is designed to establish correspondence between input packets and
      // output packets for all StreamProcessor implementations, without imposing any onerous
      // requirement that the CodecAdapter hold onto an input packet until the input packet's data
      // has been fully processed through a processing pipeline.  In short, the relative ordering of
      // OnFreeInputPacket() and OnStreamFailed() varies among StreamProcessor implementations, and
      // should not be relied on in general.  For some specific scenarios with specific known
      // StreamProcessor implementations, a client _may_ be able to reason about their relative
      // order without interference by CodecImpl, but any reliance on their relative order is
      // discouraged and deprecated.
      //
      // To assist the CodecAdapter in freeing any remaining input packets back to the client after
      // a stream failure, we call CoreCodecStopStream().  The CodecAdapter may choose to return any
      // pending input packets during CoreCodecStopStream(), or it's also fine for the CodecAdapter
      // to free any pending input packets after onCoreCodecFailStream() and before the CodecAdapter
      // sees CoreCodecStopStream().  By the end of CoreCodecStopStream(), the CodecAdapter must
      // have called onCoreCodecInputPacketDone() on all previously-pending input packets and must
      // be holding zero pending input packets.
      //
      // For some CodecAdapter implementations, it is acceptable but not preferred for some of the
      // input packets which led to stream failure to be freed before onCoreCodecFailStream(), but
      // again this is not preferred and not recommended.
      EnsureCoreCodecStreamStopped(lock);
    });

    // We're failing the current stream.  We should still queue to the output
    // ordering domain to ensure ordering vs. any previously-sent output on this
    // stream that was sent directly from codec processing thread.
    //
    // This failure is async, in the sense that the client may still be sending
    // input data, and the core codec is expected to just hold onto those
    // packets until the client has moved on from this stream.

    stream_->AssertHeld(this);
    if (stream_->future_discarded()) {
      // No reason to report a stream failure to the client for an obsolete stream.  The client has
      // already moved on from the current stream anyway.  This path won't be taken if the client
      // flushed the stream before moving on.  This permits core codecs to indicate
      // onCoreCodecFailStream() on a stream being cancelled due to a newer stream, without that
      // causing FailLocked() of the whole codec (important), and without sending an extraneous
      // OnStreamFailed() (less important since the client is expected to ignore messages for an
      // obsolete stream).  Ideally a core codec wouldn't trigger onCoreCodecFailStream() during
      // CoreCodecStopStream(), but this path tolerates it.
      return;
    }

    if (!is_on_stream_failed_enabled_) {
      FailLocked(
          "onStreamFailed() with a client that didn't send "
          "EnableOnStreamFailed(), so closing the Codec channel instead.");
      return;
    }

    // The client needs to move on from the failed stream to a new stream, or close the Codec
    // channel.
    //
    // Because we're holding lock continously to this point, we know that this message will be
    // queued to the shared fidl thread before any returned input packets from CodecAdapter after
    // onCoreCodecFailedStream(), and before any messages queued to the fidl thread by the post to
    // StreamControl above.
    PostToSharedFidl([this, stream_lifetime_ordinal = stream_lifetime_ordinal_, error] {
      // See "is_bound_checks" comment up top.
      if (binding_.is_bound()) {
        binding_.events().OnStreamFailed(stream_lifetime_ordinal, error);
      }
    });
  }  // ~lock
}

void CodecImpl::onCoreCodecResetStreamAfterCurrentFrame() {
  {  // scope lock
    ScopedLock lock(lock_);
    // Calls to onCoreCodecResetStreamAfterCurrentFrame() must be fenced out (by the core codec)
    // during CoreCodecStopStream(), so we know we still have the current stream here.
    ZX_DEBUG_ASSERT(stream_);
    // By the time we post over to StreamControl however, the current stream may no longer be
    // current.  If we've moved on to another stream, it's fine to just ignore the reset stream
    // request for a stream that's no longer current.
    uint64_t stream_lifetime_ordinal = stream_->stream_lifetime_ordinal();
    PostToStreamControl([this, stream_lifetime_ordinal] {
      ZX_DEBUG_ASSERT(IsStreamControl());
      {  // scope lock
        std::lock_guard<std::mutex> lock(lock_);

        // Only StreamControl messes with stream_.
        if (!stream_) {
          return;
        }
        ZX_DEBUG_ASSERT(stream_);
        if (stream_->stream_lifetime_ordinal() != stream_lifetime_ordinal) {
          return;
        }
        ZX_DEBUG_ASSERT(stream_->stream_lifetime_ordinal() == stream_lifetime_ordinal);
        stream_->AssertHeld(this);
        if (stream_->future_discarded()) {
          // Ignore since this stream will be gone soon anyway.
          return;
        }
        if (stream_->failure_seen()) {
          // Ignore since this stream has already failed anyway.
          return;
        }
        ZX_DEBUG_ASSERT(is_core_codec_stream_started_);
      }  // ~lock
      CoreCodecResetStreamAfterCurrentFrame();
      return;
    });
  }  // ~lock
}

void CodecImpl::onCoreCodecMidStreamOutputConstraintsChange2(uint64_t constraints_version) {
  onCoreCodecMidStreamOutputConstraintsChangeInternal(constraints_version);
}

void CodecImpl::onCoreCodecMidStreamOutputConstraintsChange(bool output_re_config_required) {
  // Passing false for this parameter is deprecated (and not done by any current CodecAdapter).
  // The old use of false here has been superseded by onCoreCodecOutputFormatChange().
  //
  // We have an output constraints change that does demand output buffer re-config before more
  // output data.
  ZX_ASSERT(output_re_config_required);

  onCoreCodecMidStreamOutputConstraintsChangeInternal(std::nullopt);
}

void CodecImpl::onCoreCodecMidStreamOutputConstraintsChangeInternal(
    std::optional<uint64_t> constraints_version) {
  VLOGF("CodecImpl::onCoreCodecMidStreamOutputConstraintsChangeInternal()");

  // Mid-stream output constraints change requires that the caller/CodecAdapter (on the input
  // processing / core codec thread) self-pauses and suspends processing operations before
  // initiating re-configuration (not dequeuing any further input items or holding any output
  // buffers/packets in flight). CodecImpl then tears down old buffers via
  // CoreCodecEnsureBuffersNotConfigured(kOutputPort), negotiates and delivers new buffers via
  // CoreCodecAddBuffer/CoreCodecConfigureBuffers, and finally signals completion via
  // CoreCodecMidStreamOutputBufferReConfigFinish, at which point the CodecAdapter initializes the
  // new buffer set and resumes input processing.
  //
  // Adapters such as DecryptorAdapter do not implement this self-pausing / resume sequencing. See
  // b/525124400.
  if (!is_supports_mid_stream_output_constraints_change_) {
    ZX_PANIC("CodecAdapter does not support mid-stream output constraints changes");
  }

  // For now, the core codec thread is the only thread this gets called from.
  ZX_DEBUG_ASSERT(IsCoreCodec());

  // Must be set when is_supports_dynammic_buffers_.
  ZX_DEBUG_ASSERT(!is_supports_dynamic_buffers_ || constraints_version.has_value());

  // The buffer_lifetime_ordinal would be buffer_lifetime_ordinal_[port] here, so nullopt is fine
  // and avoids reading buffer_lifetime_ordinal_[port] outside lock_.
  EnsureMidStreamOutputConstraintsChange(constraints_version, std::nullopt);
}

void CodecImpl::EnsureMidStreamOutputConstraintsChange(
    std::optional<uint64_t> constraints_version, std::optional<uint64_t> buffer_lifetime_ordinal) {
  ZX_DEBUG_ASSERT(IsCoreCodec() || IsFidl());
  ZX_DEBUG_ASSERT(is_supports_dynamic_buffers_ || IsCoreCodec());

  // We post over to StreamControl domain because we need to synchronize
  // with any changes to stream state that might be driven by the client.
  // When we get over there to StreamControl, we'll check if we're still
  // talking about the same stream_lifetime_ordinal, and if not, we ignore
  // the event, because a new stream may or may not have the same output
  // settings, and we'll be re-generating an OnOutputConstraints() as needed
  // from current/later core codec output constraints anyway.
  uint64_t local_stream_lifetime_ordinal;
  {  // scope lock
    std::lock_guard<std::mutex> lock(lock_);

    // The core codec is only allowed to call this mehtod while there's an
    // active stream.
    ZX_DEBUG_ASSERT(IsStreamActiveLocked());
    if (!IsStreamActiveLocked()) {
      FailLocked("onCoreCodecMidStreamOutputConstraintsChange called without active stream.");
      return;
    }

    ZX_DEBUG_ASSERT(!buffer_lifetime_ordinal.has_value() ||
                    *buffer_lifetime_ordinal <= buffer_lifetime_ordinal_[kOutputPort]);
    if (buffer_lifetime_ordinal.has_value() &&
        *buffer_lifetime_ordinal != buffer_lifetime_ordinal_[kOutputPort]) {
      // The client has already moved on to a new buffer_lifetime_ordinal that's newer than the
      // buffer that was problematic. Let a buffer of the newest buffer_lifetime_ordinal trigger
      // this instead, as needed. We don't want to assume that the client's latest buffers need to
      // be replaced during async completion of a now-stale AddBuffer.
      return;
    }

    ZX_DEBUG_ASSERT(is_supports_dynamic_buffers_ ||
                    last_noticed_codec_adapter_output_constraints_version_ == 0);
    if (is_supports_dynamic_buffers_) {
      ZX_DEBUG_ASSERT(constraints_version.has_value());
      ZX_DEBUG_ASSERT(*constraints_version >=
                      last_noticed_codec_adapter_output_constraints_version_);
      if (*constraints_version <= last_noticed_codec_adapter_output_constraints_version_) {
        return;
      }
      last_noticed_codec_adapter_output_constraints_version_ = *constraints_version;
    }

    // This part is not speculative.  The core codec has indicated that it's at
    // least meh about the current output config, so ensure we do a required
    // OnOutputConstraints() before the next stream starts, even if the client
    // moves on to a new stream such that the speculative part below becomes
    // stale.
    codec_adapter_meh_output_buffer_constraints_version_ordinal_ =
        port_settings_[kOutputPort]
            ? port_settings_[kOutputPort]->buffer_constraints_version_ordinal()
            : 0;

    // When called on core codec thread, we can assert IsStreamActiveLocked(). But when called from
    // fidl thread, the CodecAdapter may not have a current stream at this point. If there's no
    // stream, then codec_adapter_meh_output_buffer_constraints_version_ordinal_ set above will
    // trigger re-allocating output buffers if/when a new stream starts (if that ever happens).
    ZX_DEBUG_ASSERT(IsStreamActiveLocked() || IsFidl());
    if (!IsStreamActiveLocked()) {
      return;
    }
    ZX_DEBUG_ASSERT(stream_);

    // The client is allowed to essentially forget what the format is on any
    // mid-stream buffer config change, so remember to re-send the format to the
    // client before the next output packet of this stream.
    stream_->SetOutputFormatPending();

    // Speculative part - this part is speculative, in that we don't know if
    // this post over to StreamControl will beat any client driving to a new
    // stream.  So we snap the stream_lifetime_ordinal so we know whether to
    // ignore the post once it reaches StreamControl.
    local_stream_lifetime_ordinal = stream_lifetime_ordinal_;
  }  // ~lock
  PostToStreamControlForOutput([this, stream_lifetime_ordinal = local_stream_lifetime_ordinal] {
    MidStreamOutputConstraintsChange(stream_lifetime_ordinal);
  });
}

void CodecImpl::onCoreCodecOutputFormatChange() {
  ZX_DEBUG_ASSERT(IsCoreCodec());
  std::lock_guard<std::mutex> lock(lock_);
  ZX_DEBUG_ASSERT(IsStreamActiveLocked());
  if (!IsStreamActiveLocked()) {
    FailLocked("onCoreCodecOutputFormatChange called without active stream.");
    return;
  }
  // Next time the core codec asks to output a packet, we'll send the format
  // first.
  stream_->SetOutputFormatPending();
}

void CodecImpl::onCoreCodecInputPacketDone(CodecPacket* packet_param) {
  CodecPacket* packet = const_cast<CodecPacket*>(packet_param);
  uint64_t buffer_lifetime_ordinal = packet->buffer_lifetime_ordinal();
  uint32_t allocated_packet_index = packet->allocated_packet_index();
  uint32_t protocol_packet_index = packet->protocol_packet_index();
  {  // scope lock
    ScopedLock lock(lock_);
    // The CodecAdapter says the buffer-referencing in-flight lifetime of this
    // packet is over. We'll set the buffer again when this packet gets used by
    // the client again to deliver more input data.
    //
    // This SetBuffer(nullptr) is permitted to be redundant with a
    // SetBuffer(nullptr) already performed by the calling CodecAdapter. This
    // onCoreCodecInputPacketDone isn't allowed to assume that the buffer is still
    // set on the packet at this point.
    packet->SetBuffer(nullptr);
    // We have to insist that the CodecAdapter not call
    // onCoreCodecInputPacketDone() arbitrarily late because we need to know
    // when it's safe to deallocate binding_, and the CodecAdapter, etc.  So the
    // rule is the CodecAdapter needs to ensure that all calls to stream-related
    // callbacks have completed (to structure-touching degree; not
    // code-unloading degree) before CoreCodecStopStream() returns.
    ZX_DEBUG_ASSERT(is_core_codec_stream_started_);
    // QueueInputPacket is only allowed at latest buffer_lifetime_ordinal, so we
    // don't need to track free_input_packets_ at older buffer_lifetime_ordinal.
    // However, we do still indicate to the client that the old packet is done,
    // to let a client know when the associated input buffer is no longer being
    // read from. The client could also use RemoveBuffer for that, but it can be
    // convenient for OnFreeInputPacket to continue indicating input packets are
    // done despite the client having created a new buffer_lifetime_ordinal.
    if (buffer_lifetime_ordinal == buffer_lifetime_ordinal_[kInputPort]) {
      auto& packet = all_packets(kInputPort)[allocated_packet_index];
      ZX_DEBUG_ASSERT(!packet->is_free());
      packet->SetFree(true);
      if (*is_dynamic_buffers_[kInputPort]) {
        // We only ever scan free_input_packets_ in debug. This won't be a huge scan because of
        // GetDynamicBuffersMax(kInputPort) not being a particularly large number.
        ZX_DEBUG_ASSERT(std::find(free_input_packets_.begin(), free_input_packets_.end(),
                                  packet.get()) == free_input_packets_.end());
        packet->ClearProtocolPacketIndex();
        free_input_packets_.push_back(packet.get());
      }
    }

    fuchsia::media::PacketHeader header;
    header.set_buffer_lifetime_ordinal(buffer_lifetime_ordinal);
    header.set_packet_index(protocol_packet_index);
    SendFreeInputPacketLocked(std::move(header));
  }  // ~lock
}

void CodecImpl::onCoreCodecOutputPacket(CodecPacket* packet, bool error_detected_before,
                                        bool error_detected_during) {
  ZX_DEBUG_ASSERT(IsCoreCodec());

  {  // scope lock
    ScopedLock lock(lock_);

    // The core codec shouldn't output a packet until after
    // CoreCodecStartStream() and input data availability in the case that
    // output buffer config was already suitable, or until after
    // CoreCodecMidStreamOutputBufferReConfigFinish() in the case that output
    // buffer config wasn't suitable (not configured or not suitable) or
    // changed mid-stream.  See also comments in codec_adapter.h.
    ZX_DEBUG_ASSERT(IsOutputConfiguredLocked());
    if (!IsOutputConfiguredLocked()) {
      FailLocked("onCoreCodecOutputPacket called when output is not configured.");
      return;
    }

    // Before we send the packet, we check whether the stream has output format
    // pending, which means we need to send the output format before the output
    // packet (and clear the pending state).
    ZX_DEBUG_ASSERT(IsStreamActiveLocked());
    if (!IsStreamActiveLocked()) {
      FailLocked("onCoreCodecOutputPacket called when stream is not active.");
      return;
    }

    // The CodecAdapter is only allowed to send a packet referencing a buffer_lifetime_ordinal if
    // the CodecAdapter still holds a duplicate handle to the buffer. Otherwise CodecImpl will
    // delete all the packets too soon. If the CodecAdapter is behaving badly here, we may crash
    // before the assert fires.
    uint64_t buffer_lifetime_ordinal = packet->buffer_lifetime_ordinal();
    ZX_DEBUG_ASSERT(active_packets_[kOutputPort].find(buffer_lifetime_ordinal) !=
                    active_packets_[kOutputPort].end());
    // The number of packets for a given port and buffer_lifetime_ordinal only increases, until
    // they're all deleted at once, which hasn't happened yet.
    ZX_DEBUG_ASSERT(packet->allocated_packet_index() <
                    active_packets_[kOutputPort][buffer_lifetime_ordinal].size());
    uint32_t allocated_packet_index = packet->allocated_packet_index();

    // If we end up deciding not to send the packet to the client, we still need to recycle the
    // allocated_packet_index back to the CodecAdapter. This can happen due to the client not
    // wanting to receive output with an old buffer_lifetime_ordinal after receiving output with a
    // new buffer_lifetime_ordinal. Alternately this can happen if the stream is future_discarded
    // by the time this packet is popped from output_queue_; in this case, we must ensure that the
    // packet is not sent after a mid-stream constraints change for which the client never achieved
    // IsOutputConfiguredLocked() true, due to the client just moving on to a new stream instead.
    // This is accomplished by not un-pausing output until after marking the stream
    // future_discarded.
    auto short_circuit_packet = [this, buffer_lifetime_ordinal, allocated_packet_index](
                                    bool dec_in_flight_count, bool set_is_free) {
      VLOGF("output short_circuit_packet");
      ZX_DEBUG_ASSERT(IsFidl());
      CodecPacket* packet;
      {  // scope lock
        ScopedLock lock(lock_);
        auto& packets_by_ordinal = active_packets_[kOutputPort];
        auto packets_by_ordinal_iter = packets_by_ordinal.find(buffer_lifetime_ordinal);
        if (packets_by_ordinal_iter == packets_by_ordinal.end()) {
          // This is not an error. It just means the CodecAdapter dropped all its handles to all
          // buffers of the old buffer_lifetime_ordinal before we got here on the output ordering
          // domain (aka fidl thread), so at this point there's no packet to recycle.
          //
          // This is analogous to ignoring a StreamProcessor.RecycleOutputPacket that specifies a
          // no-longer-allocated CodecPacket.
          VLOGF("packets_by_ordinal_iter == packets_by_ordinal.end()");
          return;
        }
        // The number of packets for a given port and buffer_lifetime_ordinal only increases, and
        // the CodecPacket pointers remain the same, until they're all deleted at once, which
        // hasn't happened yet (per above check).
        ZX_DEBUG_ASSERT(allocated_packet_index < packets_by_ordinal_iter->second.size());
        packet = packets_by_ordinal_iter->second[allocated_packet_index].get();
        if (set_is_free) {
          packet->SetFree(true);
        }
        if (dec_in_flight_count) {
          --packet->buffer()->output_in_flight_count_;
        }
      }
      // We're ok making this call outside the lock because we're on the output ordering domain
      // (aka fidl thread), so we know the packet remains valid here (now that we've verified it
      // still exists above, and we're still on the same thread that would potentially be deleting
      // it since we checked above).
      VLOGF(
          "short circuiting onCoreCodecOutputPacket to recycle output packet ptr: %p index: %u buffer: %p index: %u",
          packet, packet->packet_index(), packet->buffer(), packet->buffer()->index());
      CoreCodecRecycleOutputPacket(packet);
    };

    // This check relies on the buffer_lifetime_ordinal_ being the next even value when there's a
    // server-driven mid-stream output constraints change. For client-driven buffer reallocation,
    // the client has to tolerate receiving some old output that crosses on the wire, but this
    // check avoids the output switching back to the old buffers after the output moves to new
    // buffers, unless the client has opted in to receiving older buffers after newer buffers.
    if (!is_enable_old_output_buffers_ &&
        (buffer_lifetime_ordinal < buffer_lifetime_ordinal_[kOutputPort])) {
      VLOGF(
          "!is_enable_old_output_buffers_ && (buffer_lifetime_ordinal < buffer_lifetime_ordinal_[kOutputPort]");
      // Recycle the output packet without the client ever being aware of it.
      PostToSharedFidl([short_circuit_packet = std::move(short_circuit_packet)] {
        std::move(short_circuit_packet)(false, false);
      });
      // We're not sending the packet to the client; just recycling it back to the CodecAdapter. We
      // intentionally never mark the packet used from a protocol point of view, since it's not.
      return;
    }

    if (!is_enable_same_output_buffer_concurrently_in_flight_ &&
        packet->buffer()->output_in_flight_count_ > 0) {
      VLOGF(
          "!is_enable_same_output_buffer_concurrently_in_flight_ && packet->buffer()->output_in_flight_count_ > 0");
      PostToSharedFidl([short_circuit_packet = std::move(short_circuit_packet)] {
        std::move(short_circuit_packet)(false, false);
      });
      return;
    }

    ++packet->buffer()->output_in_flight_count_;

    // At this point we know we will be queuing the output packet (still may not get sent if stream
    // future_discarded).
    lock.AssertHeld(stream_->parent_->lock_);
    auto stream_shared_future_discarded = stream_->shared_future_discarded();

    if (stream_->output_format_pending()) {
      VLOGF("stream_->output_format_pending()");
      stream_->ClearOutputFormatPending();
      uint64_t stream_lifetime_ordinal = stream_lifetime_ordinal_;
      uint64_t new_output_format_details_version_ordinal =
          next_output_format_details_version_ordinal_++;
      fuchsia::media::StreamOutputFormat output_format;
      {  // scope unlock
        ScopedUnlock unlock(*this);
        VLOGF("calling CoreCodecGetOutputFormat");
        output_format = CoreCodecGetOutputFormat(stream_lifetime_ordinal,
                                                 new_output_format_details_version_ordinal);
      }  // ~unlock
      lock.AssertHeld(lock_);
      ZX_DEBUG_ASSERT(output_format.has_format_details());
      // Stream change while unlocked above won't happen because we're on
      // InputData domain which is fenced as part of stream switch.
      ZX_DEBUG_ASSERT(stream_lifetime_ordinal == stream_lifetime_ordinal_);
      ZX_DEBUG_ASSERT(new_output_format_details_version_ordinal ==
                      next_output_format_details_version_ordinal_ - 1);
      ZX_DEBUG_ASSERT(sent_format_details_version_ordinal_[kOutputPort] + 1 ==
                      new_output_format_details_version_ordinal);
      sent_format_details_version_ordinal_[kOutputPort] = new_output_format_details_version_ordinal;
      VLOGF("posting to call OnOutputFormat");
      // This must order correctly wrt output packets, so use same queue as output packets.
      PostStreamOutputLocked([this, output_format = std::move(output_format),
                              stream_shared_future_discarded]() mutable {
        VLOGF("posted OnOutputFormat task running");
        if (IsStopping()) {
          VLOGF("IsStopping()");
          return;
        }
        if (stream_shared_future_discarded->load(std::memory_order_seq_cst)) {
          VLOGF("stream_shared_future_discarded->load(std::memory_order_seq_cst)");
          return;
        }
        // See "is_bound_checks" comment up top.
        if (binding_.is_bound()) {
          VLOGF("calling OnOutputFormat");
          binding_.events().OnOutputFormat(std::move(output_format));
        }
      });
    }

    // This helps verify that packet lifetimes are coherent, but we don't do this for buffer_index
    // because VP9 has show_existing_frame which is allowed to output the same buffer repeatedly
    // using separate packets in flight concurrently referencing the same buffer.
    ZX_DEBUG_ASSERT(
        packet ==
        active_packets_[kOutputPort][buffer_lifetime_ordinal][packet->allocated_packet_index()]
            .get());

    ZX_DEBUG_ASSERT(is_dynamic_buffers_[kOutputPort].has_value());
    uint32_t protocol_packet_index;
    if (!*is_dynamic_buffers_[kOutputPort]) {
      protocol_packet_index = allocated_packet_index;
      ZX_DEBUG_ASSERT(packet->protocol_packet_index() == allocated_packet_index);
    } else {
      ZX_DEBUG_ASSERT(*is_dynamic_buffers_[kOutputPort]);
      auto& packets_by_protocol_packet_index =
          protocol_packets_by_protocol_packet_index_[kOutputPort][buffer_lifetime_ordinal];
      // Every other packet, replace the protocol packet_index with a random value. Otherwise,
      // use/reuse the existing protocol packet_index value.
      //
      // This ensures the client tolerates but doesn't require low values, and tolerates reuse, and
      // tolerates non-reuse, and doesn't require 0 to be the first emitted protocol packet_index.
      //
      // We avoid collisions. Maybe with an LFSR instead we wouldn't care to check, but we still
      // technically should even if we used an LFSR.
      while (true) {
        protocol_packet_index = packet->protocol_packet_index();
        auto val = ++output_protocol_packet_index_counter_;
        if (val % 2 != 0 || packets_by_protocol_packet_index.find(protocol_packet_index) !=
                                packets_by_protocol_packet_index.end()) {
          protocol_packet_index = uniform_uint32_(prng_);
        }
        if (packets_by_protocol_packet_index.find(protocol_packet_index) !=
            packets_by_protocol_packet_index.end()) {
          continue;
        }
        packet->ClearProtocolPacketIndex();
        packet->SetProtocolPacketIndex(protocol_packet_index);
        packets_by_protocol_packet_index.insert(std::make_pair(protocol_packet_index, packet));
        break;
      }
    }

    packet->SetFree(false);

    if (IsCoreCodecHwBased(kOutputPort) &&
        port_settings_[kOutputPort]->coherency_domain() == fuchsia_sysmem2::CoherencyDomain::kCpu) {
      // This invalidates only the portion of the buffer that the packet is referencing.
      packet->CacheFlushAndInvalidate();
    }

    ZX_DEBUG_ASSERT(packet->has_start_offset());
    ZX_DEBUG_ASSERT(packet->has_valid_length_bytes());
    // packet->has_timestamp_ish() is optional even if
    // promise_separate_access_units_on_input is true.  We do want to enforce
    // that the client gets no set timestamp_ish values if the client didn't
    // promise_separate_access_units_on_input.
    bool has_timestamp_ish =
        (!IsDecoder() || (decoder_params().has_promise_separate_access_units_on_input() &&
                          decoder_params().promise_separate_access_units_on_input())) &&
        packet->has_timestamp_ish();
    fuchsia::media::Packet p;
    p.mutable_header()->set_buffer_lifetime_ordinal(buffer_lifetime_ordinal);
    p.mutable_header()->set_packet_index(protocol_packet_index);
    p.set_buffer_index(packet->buffer()->index());
    p.set_stream_lifetime_ordinal(stream_lifetime_ordinal_);
    p.set_start_offset(packet->start_offset());
    p.set_valid_length_bytes(packet->valid_length_bytes());
    if (has_timestamp_ish) {
      p.set_timestamp_ish(packet->timestamp_ish());
    }
    if (packet->has_key_frame()) {
      p.set_key_frame(packet->key_frame());
    }
    p.set_start_access_unit(true);
    p.set_known_end_access_unit(true);
    VLOGF("posting to call OnOutputPacket - packet ptr: %p index: %u", packet,
          packet->packet_index());
    // This same queue is used for OnOutputFormat and RemoveBuffer completions, so those will order
    // correctly wrt output packets.
    PostStreamOutputLocked([this, p = std::move(p), error_detected_before, error_detected_during,
                            stream_shared_future_discarded,
                            short_circuit_packet = std::move(short_circuit_packet)]() mutable {
      VLOGF("posted OnOutputPacket task running");
      if (IsStopping()) {
        VLOGF("IsStopping()");
        // Despite CodecImpl and CodecAdapter going away soon, it's important to
        // short_circuit_packet here when dynamic buffers are supported by the CodecAdapter, because
        // the CodecAdapter's CoreCodecStopStream can depend on it.
        //
        // recycling of packets is not per-stream, so recycle the packet here
        std::move(short_circuit_packet)(true, true);
        return;
      }
      if (stream_shared_future_discarded->load(std::memory_order_seq_cst)) {
        VLOGF("stream_shared_future_discarded->load(std::memory_order_seq_cst)");
        // recycling of packets is not per-stream, so recycle the packet here
        std::move(short_circuit_packet)(true, true);
        return;
      }
      // See "is_bound_checks" comment up top.
      if (!binding_.is_bound()) {
        VLOGF("!binding_.is_bound()");
        // Despite CodecImpl and CodecAdapter going away soon, it's important to
        // short_circuit_packet here when dynamic buffers are supported by the CodecAdapter, because
        // the CodecAdapter's CoreCodecStopStream can depend on it.
        //
        // recycling of packets is not per-stream, so recycle the packet here
        std::move(short_circuit_packet)(true, true);
        return;
      }

      ZX_DEBUG_ASSERT(binding_.is_bound());
      if (kLogTimestampDelay) {
        LOG(INFO, "output timestamp: has: %d value: 0x%" PRIx64, p.has_timestamp_ish(),
            p.has_timestamp_ish() ? p.timestamp_ish() : 0);
      }
      VLOGF("calling OnOutputPacket");
      binding_.events().OnOutputPacket(std::move(p), error_detected_before, error_detected_during);
    });
  }  // ~lock
}

void CodecImpl::onCoreCodecOutputTimestampHasNoOutput(uint64_t timestamp_ish) {
  // This call is only permitted from CodecAdapter(s) that support dynamic buffers.
  ZX_ASSERT(is_supports_dynamic_buffers());
  if constexpr (!::codec_impl::internal::kEnableDynamicBuffers) {
    return;
  }
  LOG(DEBUG, " timestamp_ish: %" PRIu64 " stream_lifetime_ordinal: %" PRIu64, timestamp_ish,
      stream_lifetime_ordinal_);
  VLOGF("CodecImpl::onCoreCodecOutputTimestampHasNoOutput %" PRId64, timestamp_ish);
  // The CodecAdapter is responsible for only calling onCoreCodecOutputTimestampHasNoOutput after
  // the first input is sent to the CodecAdapter for the stream, and before CoreCodecStopStream has
  // returned for the stream.
  ZX_ASSERT(is_core_codec_stream_started_.load(std::memory_order_seq_cst));
  {  // scope lock
    ScopedLock lock(lock_);
    ZX_DEBUG_ASSERT(IsStreamActiveLocked());
    lock.AssertHeld(stream_->parent_->lock_);
    PostStreamOutputLocked([this, stream_lifetime_ordinal = stream_lifetime_ordinal_, timestamp_ish,
                            stream_shared_future_discarded = stream_->shared_future_discarded()] {
      if (IsStopping()) {
        return;
      }
      if (stream_shared_future_discarded->load(std::memory_order_seq_cst)) {
        return;
      }
      if (binding_.is_bound()) {
        fuchsia::media::StreamProcessorOnOutputTimestampHasNoOutputRequest event;
        event.set_stream_lifetime_ordinal(stream_lifetime_ordinal);
        event.set_timestamp_ish(timestamp_ish);
        binding_.events().OnOutputTimestampHasNoOutput(std::move(event));
      }
    });
  }  // ~lock
}

void CodecImpl::onCoreCodecOutputEndOfStream(bool error_detected_before) {
  VLOGF("CodecImpl::onCoreCodecOutputEndOfStream()");
  LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_CoreEndOfStreamOuput);
  {  // scope lock
    ScopedLock lock(lock_);
    ZX_DEBUG_ASSERT(IsStreamActiveLocked());
    if (!IsStreamActiveLocked()) {
      FailLocked("onCoreCodecOutputEndOfStream called when stream is not active.");
      return;
    }
    // See comment in lambda posted below for why it's ok that we're calling SetOutputEndOfStream
    // before the lambda runs.
    stream_->SetOutputEndOfStream();
    wake_stream_control_condition_.notify_all();
    lock.AssertHeld(stream_->parent_->lock_);
    // output EOS is ordered on same queue as output packets (and related)
    PostStreamOutputLocked([this, stream_lifetime_ordinal = stream_lifetime_ordinal_,
                            error_detected_before,
                            stream_shared_future_discarded = stream_->shared_future_discarded()] {
      if (IsStopping()) {
        return;
      }
      // If a FlushEndOfStreamAndCloseStream was sent by the client before doing anything that would
      // normally discard the stream (were it not for FlushEndOfStreamAndCloseStream), then the
      // stream won't be marked as discarded here. This is why it's ok that we called
      // stream_->SetOutputEndOfStream() before posting the present lambda.
      if (stream_shared_future_discarded->load(std::memory_order_seq_cst)) {
        return;
      }
      // See "is_bound_checks" comment up top.
      if (binding_.is_bound()) {
        LogEvent(media_metrics::
                     StreamProcessorEvents2MigratedMetricDimensionEvent_StreamEndOfStreamOutput);
        binding_.events().OnOutputEndOfStream(stream_lifetime_ordinal, error_detected_before);
      }
    });
  }  // ~lock
}

void CodecImpl::onCoreCodecLogEvent(
    media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent event_code) {
  // A CodecAdapter sub-class that ever calls LogEvent() must override
  // CoreCodecMetricsImplementation() and must not return std::nullopt.
  ZX_DEBUG_ASSERT(codec_metrics_implementation_dimension_);
  LogEvent(event_code);
}

CodecImpl::Stream::Stream(const CodecImpl* const parent, uint64_t stream_lifetime_ordinal)
    : parent_(parent), stream_lifetime_ordinal_(stream_lifetime_ordinal) {
  // nothing else to do here
}

void CodecImpl::Stream::AssertHeld(const CodecImpl* const parent) {
  ZX_ASSERT(parent == parent_);
  // And of course the lock is the same lock also.
  ZX_ASSERT(&parent->lock_ == &parent_->lock_);
}

uint64_t CodecImpl::Stream::stream_lifetime_ordinal() { return stream_lifetime_ordinal_; }

void CodecImpl::Stream::SetFutureDiscarded() {
  ZX_DEBUG_ASSERT(!future_discarded_->load());
  // This store is both under lock_ and seq_cst. This allows loads under lock_ to be relaxed, and
  // allows loads outside lock_ to see this write if this write occurred previously, despite the
  // lack of lock_ acquire.
  future_discarded_->store(true, std::memory_order_seq_cst);
}

bool CodecImpl::Stream::future_discarded() {
  // future_discarded is __TA_REQUIRES(lock_), and all writes to this field are while holding lock_,
  // so a relaxed load is fine here
  return future_discarded_->load(std::memory_order_relaxed);
}

std::shared_ptr<std::atomic<bool>> CodecImpl::Stream::shared_future_discarded() {
  return future_discarded_;
}

void CodecImpl::Stream::SetFutureFlushEndOfStream() {
  ZX_DEBUG_ASSERT(!future_flush_end_of_stream_);
  future_flush_end_of_stream_ = true;
}

bool CodecImpl::Stream::future_flush_end_of_stream() { return future_flush_end_of_stream_; }

CodecImpl::Stream::~Stream() {
  VLOGF("~Stream() stream_lifetime_ordinal: %lu", stream_lifetime_ordinal_);
}

void CodecImpl::Stream::SetInputFormatDetails(
    std::unique_ptr<fuchsia::media::FormatDetails> input_format_details) {
  // This is allowed to happen multiple times per stream.
  input_format_details_ = std::move(input_format_details);
}

const fuchsia::media::FormatDetails* CodecImpl::Stream::input_format_details() {
  return input_format_details_.get();
}

void CodecImpl::Stream::SetOobConfigPending(bool pending) {
  // SetOobConfigPending(true) is legal regardless of current state, but
  // SetOobConfigPending(false) is only legal if the state is currently true.
  ZX_DEBUG_ASSERT(pending || oob_config_pending_);
  oob_config_pending_ = pending;
}

bool CodecImpl::Stream::oob_config_pending() { return oob_config_pending_; }

void CodecImpl::Stream::SetInputEndOfStream() {
  ZX_DEBUG_ASSERT(!input_end_of_stream_);
  input_end_of_stream_ = true;
}

bool CodecImpl::Stream::input_end_of_stream() { return input_end_of_stream_; }

void CodecImpl::Stream::SetOutputEndOfStream() {
  ZX_DEBUG_ASSERT(!output_end_of_stream_);
  output_end_of_stream_ = true;
}

bool CodecImpl::Stream::output_end_of_stream() { return output_end_of_stream_; }

void CodecImpl::Stream::SetFailureSeen() {
  ZX_DEBUG_ASSERT(!failure_seen_);
  failure_seen_ = true;
}

bool CodecImpl::Stream::failure_seen() { return failure_seen_; }

void CodecImpl::Stream::SetOutputFormatPending() { output_format_pending_ = true; }

void CodecImpl::Stream::ClearOutputFormatPending() { output_format_pending_ = false; }

bool CodecImpl::Stream::output_format_pending() { return output_format_pending_; }

CodecImpl::PortSettings::PortSettings(CodecImpl* parent, CodecPort port,
                                      fuchsia::media::StreamBufferPartialSettings partial_settings)
    : parent_(parent),
      port_(port),
      partial_settings_(std::make_unique<fuchsia::media::StreamBufferPartialSettings>(
          std::move(partial_settings))),
      buffer_constraints_version_ordinal_(partial_settings_->buffer_constraints_version_ordinal()),
      buffer_lifetime_ordinal_(partial_settings_->buffer_lifetime_ordinal()) {}

CodecImpl::PortSettings::PortSettings(CodecImpl* parent, CodecPort port,
                                      uint64_t buffer_constraints_version_ordinal,
                                      uint64_t buffer_lifetime_ordinal)
    : parent_(parent),
      port_(port),
      buffer_constraints_version_ordinal_(buffer_constraints_version_ordinal),
      buffer_lifetime_ordinal_(buffer_lifetime_ordinal) {}

CodecImpl::PortSettings::~PortSettings() {
  // To be safe, the unbind needs to occur on the FIDL thread.  In addition, we want to send a clean
  // Close() to avoid causing the LogicalBufferCollection to fail.  Since we're not a crashing
  // process, this is a clean close by definition.
  //
  // TODO(https://fxbug.dev/42112876): Consider _not_ sending Close() for unexpected failures
  // initiated by the server. Consider whether to have a Close() on StreamProcessor to disambiguate
  // clean vs. unexpected StreamProcessor channel close.
  if (!parent_->IsFidl()) {
    parent_->PostToSharedFidl([buffer_collection = std::move(buffer_collection_)] {
      // Sysmem will notice the Close() before the PEER_CLOSED.
      if (!!buffer_collection && buffer_collection->is_valid()) {
        // ignore potential one-way send failure
        (void)(*buffer_collection)->Release();
      }
      // ~buffer_collection on FIDL thread
    });
    ZX_DEBUG_ASSERT(!buffer_collection_);
  } else {
    if (!!buffer_collection_) {
      // ignore potential one-way send failure
      (void)(*buffer_collection_)->Release();
    }
  }
}

void CodecImpl::PortSettings::SetBufferCollectionInfo(
    fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info) {
  ZX_DEBUG_ASSERT(!buffer_collection_info_);
  buffer_collection_info_ =
      std::make_unique<fuchsia_sysmem2::BufferCollectionInfo>(std::move(buffer_collection_info));
}

void CodecImpl::PortSettings::ClearBufferCollectionInfo() { buffer_collection_info_.reset(); }

const fuchsia_sysmem2::BufferCollectionInfo& CodecImpl::PortSettings::buffer_collection_info()
    const {
  ZX_DEBUG_ASSERT(buffer_collection_info_);
  return *buffer_collection_info_;
}

uint64_t CodecImpl::PortSettings::buffer_lifetime_ordinal() { return buffer_lifetime_ordinal_; }

uint64_t CodecImpl::PortSettings::buffer_constraints_version_ordinal() {
  return buffer_constraints_version_ordinal_;
}

uint32_t CodecImpl::PortSettings::packet_count() {
  ZX_DEBUG_ASSERT(!is_dynamic_);
  ZX_DEBUG_ASSERT(buffer_collection_info_);
  ZX_DEBUG_ASSERT(buffer_collection_info_->buffers().has_value());
  ZX_DEBUG_ASSERT(!buffer_collection_info_->buffers()->empty());
  return static_cast<uint32_t>(buffer_collection_info_->buffers()->size());
}

uint32_t CodecImpl::PortSettings::min_buffer_count() {
  // The term "fully configured" in the comments below means the CodecAdapter can attempt to begin
  // processing.
  if (is_dynamic_) {
    // Dynamic buffer mode doesn't require any current buffers to be considered fully configured.
    // The client is expectecd to add sufficient buffers to allow processing to actually proceed.
    //
    // When the client uses StreamProcessor.AddBuffer, that doesn't immediately add the buffer to
    // the CodecAdapter. Instead, we start an async sequence that involves a round trip to/from
    // sysmem before the buffer can be added to the CodecAdapter. This round trip can be in progress
    // for multiple buffers being added concurrently (the async-ness is transparent to the client
    // and just looks like the CodecAdapter hasn't generated output yet).
    //
    // When is_dynamic_, the client can intentionally reduce the number of current buffers
    // mid-stream, potentially even down to zero if the CodecAdapter happens to allow that. The
    // CodecAdapter is not required to allow that, but should allow 0 current buffers if 0 buffers
    // are needed to maintain necessary stream processing context.
    //
    // The CodecAdapter will make progress when sufficient input is available and there's a free
    // output buffer to emit output into. Having zero current buffers doesn't prevent telling the
    // CodecAdapter to process (when is_dynamic_).
    return 0;
  }
  // When !is_dynamic_, this should only be called if buffer_collection_info_ is already present,
  // and that buffer_collection_info_ will have the buffers field set, and the number of buffers
  // will be at least 1. We require all the buffers from sysmem to be configured for the port to be
  // considered fully configured, so the min_buffer_count is the number of buffers from sysmem.
  ZX_DEBUG_ASSERT(buffer_collection_info_ && buffer_collection_info_->buffers().has_value() &&
                  !buffer_collection_info_->buffers()->empty());
  return static_cast<uint32_t>(buffer_collection_info_->buffers()->size());
}

fuchsia_sysmem2::CoherencyDomain CodecImpl::PortSettings::coherency_domain() {
  ZX_ASSERT(buffer_collection_info_);
  ZX_ASSERT(buffer_collection_info_->settings().has_value());
  ZX_ASSERT(buffer_collection_info_->settings()->buffer_settings().has_value());
  ZX_ASSERT(buffer_collection_info_->settings()->buffer_settings()->coherency_domain().has_value());
  return buffer_collection_info_->settings()->buffer_settings()->coherency_domain().value();
}

const fuchsia::media::StreamBufferPartialSettings& CodecImpl::PortSettings::partial_settings() {
  return *partial_settings_;
}

fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> CodecImpl::PortSettings::TakeToken() {
  ZX_DEBUG_ASSERT(!partial_settings_->has_sysmem_token());
  ZX_DEBUG_ASSERT(partial_settings_->has_sysmem2_token());
  auto token = fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(
      partial_settings_->mutable_sysmem2_token()->TakeChannel());
  partial_settings_->clear_sysmem2_token();
  return token;
}

zx::vmo CodecImpl::PortSettings::TakeVmo(uint32_t buffer_index) {
  ZX_DEBUG_ASSERT(!is_dynamic_);
  ZX_DEBUG_ASSERT(buffer_collection_info_);
  ZX_DEBUG_ASSERT(buffer_index < buffer_collection_info_->buffers()->size());
  return std::move(buffer_collection_info_->buffers()->at(buffer_index).vmo().value());
}

fidl::ServerEnd<fuchsia_sysmem2::BufferCollection>
CodecImpl::PortSettings::NewBufferCollectionRequest(
    async_dispatcher_t* dispatcher,
    CodecImpl::Client<fuchsia_sysmem2::BufferCollection>::ErrorFunction on_error) {
  ZX_DEBUG_ASSERT(parent_->IsFidl());
  ZX_DEBUG_ASSERT(!buffer_collection_);
  auto collection_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollection>();
  ZX_ASSERT(collection_endpoints.is_ok());
  buffer_collection_ = std::make_unique<Client<fuchsia_sysmem2::BufferCollection>>();
  buffer_collection_->Bind(std::move(collection_endpoints->client), dispatcher,
                           std::move(on_error));
  return std::move(collection_endpoints->server);
}

std::unique_ptr<CodecImpl::Client<fuchsia_sysmem2::BufferCollection>>&
CodecImpl::PortSettings::buffer_collection() {
  ZX_DEBUG_ASSERT(parent_->IsFidl());
  return buffer_collection_;
}

void CodecImpl::PortSettings::UnbindBufferCollection() {
  ZX_DEBUG_ASSERT(parent_->IsFidl());
  // Unbind even if there are outstanding requests; delete context for those if they exist.
  buffer_collection_.reset();
}

bool CodecImpl::PortSettings::is_complete_seen_output() {
  ZX_DEBUG_ASSERT(port_ == kOutputPort);
  if (is_dynamic_) {
    // when dynamic buffers, the PortSettings instance won't exist until we have at least one buffer
    return true;
  } else {
    return is_complete_seen_output_;
  }
}

void CodecImpl::PortSettings::SetCompleteSeenOutput() {
  ZX_DEBUG_ASSERT(port_ == kOutputPort);
  ZX_DEBUG_ASSERT(parent_->IsFidl());
  ZX_DEBUG_ASSERT(!is_complete_seen_output_);
  // not called when is_dynamic(); the message gets rejected before this
  ZX_DEBUG_ASSERT(!is_dynamic_);
  is_complete_seen_output_ = true;
}

uint64_t CodecImpl::PortSettings::vmo_usable_start(uint32_t buffer_index) {
  ZX_DEBUG_ASSERT(!is_dynamic_);
  ZX_DEBUG_ASSERT(buffer_collection_info_);
  ZX_DEBUG_ASSERT(buffer_index < buffer_collection_info_->buffers()->size());
  return buffer_collection_info_->buffers()->at(buffer_index).vmo_usable_start().value();
}

uint64_t CodecImpl::PortSettings::vmo_usable_size() {
  ZX_DEBUG_ASSERT(buffer_collection_info_);
  return buffer_collection_info_->settings()->buffer_settings()->size_bytes().value();
}

bool CodecImpl::PortSettings::is_secure() {
  ZX_DEBUG_ASSERT(buffer_collection_info_);
  return buffer_collection_info_->settings()->buffer_settings()->is_secure().value();
}

CodecImpl::PausedOutput::PausedOutput(CodecImpl& parent) : parent_(parent) {}

CodecImpl::PausedOutput::~PausedOutput() {
  VLOGF("~PausedOutput top");
  // Any std::weak_ptr<PausedOutput>(s) are already unable to successfully
  // lock() by this point. The inability to lock() has already released any
  // output emitted subsequent to OnOutputConstraints by the CodecAdapter.
  // In the case of *is_dynamic_buffers_ false, we can't send this output
  // until the client has sent CompleteOutputBufferPartialSettings, which is
  // one of the conditions that lets IsOutputConfiguredLocked return true.
  //
  // Now that the output is already released, make sure it gets sent. Posting a
  // nop lambda will run all previously-posted lambdas as well now that output
  // isn't paused.
  parent_.PostStreamOutputLocked([] {});
}

//
// CoreCodec wrappers, for the asserts.  These asserts, and the way we ensure
// at compile time that this class has a method for every method of
// CodecAdapter, are essentially costing a double vtable call instead of a
// single vtable call.  If we don't like that at some point, we can remove the
// private CodecAdapter inheritance from CodecImpl and have these be normal
// methods instead of virtual methods.
//

void CodecImpl::CoreCodecInit(const fuchsia::media::FormatDetails& initial_input_format_details) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  codec_adapter_->CoreCodecInit(initial_input_format_details);
}

void CodecImpl::CoreCodecSetSecureMemoryMode(
    CodecPort port, fuchsia::mediacodec::SecureMemoryMode secure_memory_mode) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  codec_adapter_->CoreCodecSetSecureMemoryMode(port, secure_memory_mode);
}

void CodecImpl::CoreCodecSetForceNewBuffersOnNewDimensions(bool force) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  codec_adapter_->CoreCodecSetForceNewBuffersOnNewDimensions(force);
}

std::optional<CodecAdapter::CoreCodecGetBufferCollectionConstraints3Result>
CodecImpl::CoreCodecGetBufferCollectionConstraints3(CodecPort port) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  auto result = codec_adapter_->CoreCodecGetBufferCollectionConstraints3(port);
  // When is_supports_dynamic_buffers_, must has_value(). When !is_supports_dynamic_buffers_, may
  // has_value().
  ZX_DEBUG_ASSERT(!is_supports_dynamic_buffers_ || result.has_value());
  return result;
}

fuchsia_sysmem2::BufferCollectionConstraints CodecImpl::CoreCodecGetBufferCollectionConstraints2(
    CodecPort port, const fuchsia::media::StreamBufferConstraints& stream_buffer_constraints,
    const fuchsia::media::StreamBufferPartialSettings& partial_settings) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  // We don't intend to send the sysmem token to the core codec directly, just
  // because it doesn't really need to participate directly that way, and this
  // lets us keep direct interaction with sysmem in CodecImpl instead of each
  // core codec.
  ZX_DEBUG_ASSERT(!partial_settings.has_sysmem2_token());
  ZX_DEBUG_ASSERT(!partial_settings.has_sysmem_token());
  return codec_adapter_->CoreCodecGetBufferCollectionConstraints2(port, stream_buffer_constraints,
                                                                  partial_settings);
}

uint64_t CodecImpl::CoreCodecGetConstraintsVersion(CodecPort port) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  return codec_adapter_->CoreCodecGetConstraintsVersion(port);
}

void CodecImpl::CoreCodecSetBufferCollectionInfo(
    CodecPort port, const fuchsia_sysmem2::BufferCollectionInfo& buffer_collection_info) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  codec_adapter_->CoreCodecSetBufferCollectionInfo(port, buffer_collection_info);
}

void CodecImpl::CoreCodecAddBuffer(CodecPort port, const CodecBuffer* buffer) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  codec_adapter_->CoreCodecAddBuffer(port, buffer);
}

void CodecImpl::CoreCodecConfigureBuffers(
    CodecPort port, const std::vector<std::unique_ptr<CodecPacket>>& packets) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  codec_adapter_->CoreCodecConfigureBuffers(port, packets);
}

void CodecImpl::CoreCodecEnsureBuffersNotConfigured(CodecPort port) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() ||
                  port == kOutputPort && (IsFidl() || IsStreamControl()));
  // We shouldn't be calling CoreCodecEnsureBuffersNotConfigured for a CodecAdapter that supports
  // dynamic buffers. Instead we want to be calling CoreCodecRemoveBuffer for each relevant buffer.
  ZX_DEBUG_ASSERT(!is_supports_dynamic_buffers_);
  codec_adapter_->CoreCodecEnsureBuffersNotConfigured(port);
}

void CodecImpl::CoreCodecStartStream() {
  ZX_DEBUG_ASSERT(IsStreamControl());
  LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_StreamCreated);
  codec_adapter_->CoreCodecStartStream();
}

void CodecImpl::CoreCodecQueueInputFormatDetails(
    const fuchsia::media::FormatDetails& per_stream_override_format_details) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  codec_adapter_->CoreCodecQueueInputFormatDetails(per_stream_override_format_details);
}

void CodecImpl::CoreCodecQueueInputPacket(CodecPacket* packet) {
  ZX_DEBUG_ASSERT(IsStreamControl());
  codec_adapter_->CoreCodecQueueInputPacket(packet);
}

void CodecImpl::CoreCodecQueueInputEndOfStream() {
  ZX_DEBUG_ASSERT(IsStreamControl());
  LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_CoreEndOfStreamInput);
  codec_adapter_->CoreCodecQueueInputEndOfStream();
}

void CodecImpl::CoreCodecStopStream() {
  ZX_DEBUG_ASSERT(IsStreamControl());
  LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent_StreamDeleted);
  codec_adapter_->CoreCodecStopStream();
}

void CodecImpl::CoreCodecResetStreamAfterCurrentFrame() {
  ZX_DEBUG_ASSERT(IsStreamControl());
  codec_adapter_->CoreCodecResetStreamAfterCurrentFrame();
}

std::optional<media_metrics::StreamProcessorEvents2MigratedMetricDimensionImplementation>
CodecImpl::CoreCodecMetricsImplementation() {
  ZX_DEBUG_ASSERT(IsFidl());
  return codec_adapter_->CoreCodecMetricsImplementation();
}

bool CodecImpl::IsCoreCodecRequiringOutputConfigForFormatDetection() {
  ZX_DEBUG_ASSERT(IsFidl() || IsStreamControl());
  return codec_adapter_->IsCoreCodecRequiringOutputConfigForFormatDetection();
}

bool CodecImpl::IsCoreCodecMappedBufferUseful(CodecPort port) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  return codec_adapter_->IsCoreCodecMappedBufferUseful(port);
}

bool CodecImpl::IsCoreCodecHwBased(CodecPort port) {
  return codec_adapter_->IsCoreCodecHwBased(port);
}

zx::unowned_bti CodecImpl::CoreCodecBti() {
  ZX_DEBUG_ASSERT(IsCoreCodecHwBased(kInputPort) || IsCoreCodecHwBased(kOutputPort));
  return codec_adapter_->CoreCodecBti();
}

fuchsia::media::StreamOutputFormat CodecImpl::CoreCodecGetOutputFormat(
    uint64_t stream_lifetime_ordinal, uint64_t new_output_format_details_version_ordinal) {
  ZX_DEBUG_ASSERT(IsCoreCodec());
  fuchsia::media::StreamOutputFormat format = codec_adapter_->CoreCodecGetOutputFormat(
      stream_lifetime_ordinal, new_output_format_details_version_ordinal);
  ZX_DEBUG_ASSERT(format.has_stream_lifetime_ordinal());
  ZX_DEBUG_ASSERT(format.stream_lifetime_ordinal() == stream_lifetime_ordinal);
  ZX_DEBUG_ASSERT(format.has_format_details());
  ZX_DEBUG_ASSERT(format.format_details().has_format_details_version_ordinal());
  ZX_DEBUG_ASSERT(format.format_details().format_details_version_ordinal() ==
                  new_output_format_details_version_ordinal);
  return format;
}

void CodecImpl::CoreCodecMidStreamOutputBufferReConfigPrepare() {
  ZX_DEBUG_ASSERT(IsStreamControl());
  codec_adapter_->CoreCodecMidStreamOutputBufferReConfigPrepare();
}

void CodecImpl::CoreCodecMidStreamOutputBufferReConfigFinish() {
  ZX_DEBUG_ASSERT(IsStreamControl());
  codec_adapter_->CoreCodecMidStreamOutputBufferReConfigFinish();
}

void CodecImpl::CoreCodecRecycleOutputPacket(CodecPacket* packet) {
  ZX_DEBUG_ASSERT(IsFidl());
  codec_adapter_->CoreCodecRecycleOutputPacket(packet);
}

void CodecImpl::CoreCodecSetStreamControlProfile(zx::unowned_thread stream_control_thread) {
  codec_adapter_->CoreCodecSetStreamControlProfile(std::move(stream_control_thread));
}

void CodecImpl::CoreCodecRemoveBuffer(CodecPort port, const CodecBuffer* buffer) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  codec_adapter_->CoreCodecRemoveBuffer(port, buffer);
}

void CodecImpl::CoreCodecCloseBufferLifetimeOrdinal(CodecPort port,
                                                    uint64_t buffer_lifetime_ordinal) {
  ZX_DEBUG_ASSERT(port == kInputPort && IsStreamControl() || port == kOutputPort && IsFidl());
  codec_adapter_->CoreCodecCloseBufferLifetimeOrdinal(port, buffer_lifetime_ordinal);
  // At this point the CodecAdapter no longer holds any CodecPacket pointers under the
  // buffer_lifetime_ordinal.
}

std::string CodecImpl::CoreCodecGetSchedulerProfileName(OrderingDomain ordering_domain) {
  ZX_DEBUG_ASSERT(IsFidl());
  return codec_adapter_->CoreCodecGetSchedulerProfileName(ordering_domain);
}
