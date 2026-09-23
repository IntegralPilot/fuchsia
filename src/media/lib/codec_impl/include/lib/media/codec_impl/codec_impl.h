// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_IMPL_H_
#define SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_IMPL_H_

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fuchsia/media/drm/cpp/fidl.h>
#include <fuchsia/mediacodec/cpp/fidl.h>
#include <fuchsia/sysmem/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/closure-queue/closure_queue.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/media/codec_impl/codec_adapter.h>
#include <lib/media/codec_impl/codec_adapter_events.h>
#include <lib/media/codec_impl/codec_admission_control.h>
#include <lib/media/codec_impl/codec_buffer.h>
#include <lib/media/codec_impl/codec_diagnostics.h>
#include <lib/media/codec_impl/codec_packet.h>
#include <lib/media/codec_impl/fake_map_range.h>
#include <lib/thread-safe-deleter/thread_safe_deleter.h>
#include <zircon/compiler.h>

#ifndef CODEC_IMPL_INTERNAL_ENABLE_DYNAMIC_BUFFERS
#define CODEC_IMPL_INTERNAL_ENABLE_DYNAMIC_BUFFERS 0
#endif

namespace codec_impl::internal {
constexpr bool kEnableDynamicBuffers = (CODEC_IMPL_INTERNAL_ENABLE_DYNAMIC_BUFFERS != 0);
}  // namespace codec_impl::internal

#include <atomic>
#include <list>
#include <mutex>
#include <queue>
#include <random>

#include <fbl/macros.h>

#include "src/media/lib/codec_impl/dispatcher.h"

// The CodecImpl class can be used for both SW and HW codecs.
//
// Roughly speaking, this class converts the Codec FIDL interface which has
// cross-process pipelining of stream switches into a more synchronous
// in-process CodecAdapter interface which only has input and output data
// handled async, with stream control handled sync for the most part.
//
// This class also handles Codec protocol checks applicable to any Codec server.
//
// TODO(dustingreen): Pull CodecImpl out to a source_set, to be used by
// omx_codec_runner.h/cc also.

// Lifetime:
//
// A CodecImpl is created, either Bind()ed or destructed, and if Bind()ed, then
// later when the channel fails or there's a protocol error, calls the owner's
// error handler and the owner deletes the CodecImpl.  There is intentionally no
// way to reuse a CodecImpl for another Codec channel.

// Error handling:
//
// There are two types of errors, per-CodecImpl errors and per-devhost-process
// errors.
//
// Per-CodecImpl:
//
// We handle per-Codec protocol errors and the like by calling Unbind() on the
// CodecImpl, which fairly soon results in ~CodecImpl async, but does not exit
// the whole devhost process.  We also handle a few per-CodecImpl errors this
// way even though some of those are closer to being caused by per-devhost
// conditions, just in case.
//
// Per-devhost-process:
//
// In contrast, per-devhost error conditions, like the inability to post work to
// the shared_fidl_thread(), are handled by exiting the devhost, because those
// conditions are not really unique to any one CodecImpl.

// "Ordering Domain":
//
// The term "ordering domain" is intended to mean "sequence" when using a sequence-capable
// dispatcher, or "thread" when using a non-sequence-capable dispatcher. Any lingering instances of
// just "thread" can be assumed to actually mean "ordering domain".
//
// Regardless of sequence vs. thread, posted tasks run in posted order and on one thread at a time.
//
// If all remaining async_dispatcher_t(s) are eventually updated to support sequence ops, we'll want
// to update the CodecImpl comments to just say "sequence", but currently saying "sequence" would be
// inaccurate for client code using async::Loop::dispatcher(), so we still say "ordering domain".
//
// There are currently 3 ordering domains:
//   * fidl (1 process-wide)
//     * all FIDL handling/sending + output port activity
//     * non-blocking (intent; may not fully match reality for older CodecImpl client code)
//   * StreamControl (1 per StreamProcessor server)
//     * input port activity
//     * input packets transit this domain
//     * handles stream state transitions such as starting/stopping the up-to-one
//       CodecAdapter-visible stream
//     * coordinates mid-stream buffer reallocation using sequential blocking code
//     * allowed to block
//     * separate instance per CodecImpl instance (per StreamProcessor server)
//   * core codec / InputData (1 process-wide)
//     * this can optionally share with fidl, if certain requirements are met; see
//       SetSharingFidlDomainForCoreCodec
//     * ideally non-blocking
//       * this ideal is subject to caveats however
//         * older CodecImpl client code may not be ideal
//         * FW/HW that's not as capable may demand the CPU spin-wait for long-ish durations (see
//           below)
//         * SW-based codecs don't "block" but do need to use the CPU to process the bulk input data
//           (see below).
//     * SW-based codecs must not call SetSharingFidlDomainForCoreCodec unless the SW-based codec
//       uses separate threads (not visible to CodecImpl) for the bulk decode/encode on the CPU.
//     * new HW-based codecs should consider calling SetSharingFidlDomainForCoreCodec if
//       requirements can be met, as this avoids some thread switching.
//       * If the FW/HW requires any long-duration spin-waits by the CPU subsequent to initial
//         driver initialization, at any time when other streams may be active, consider splitting
//         up those waits with timer-driven polling (or use an interrupt if available from the
//         FW/HW, though this isn't always available unfortunately). Or, don't call
//         SetSharingFidlDomainForCoreCodec so that any such CPU spin-waits are kept off the fidl
//         thread (likely the simpler option, given such FW/HW). This sort of FW/HW is a main reason
//         we retain the option to not call SetSharingFidlDomainForCoreCodec and use separate
//         concurrent core codec ordering domain.
//     * Motivating considerations:
//       * Quick stream switching when a StreamProcessor client cancels an old stream and creates a
//         new stream to replace it.
//       * Non-interference among multiple StreamProcessor servers multiplexing the HW.
//         * not a concern for SW-based StreamProcessor servers as these intentionally run a
//           separate isolated process per StreamProcessor server, aside from overall system CPU
//           load which is not currently mitigated (as is typical for nearly all low-layer media
//           stacks capable of running SW-based codecs fwiw)

// Potential refactoring ideas:
//
// Each odd buffer_lifetime_ordinal value could have an instance of a BufferLifetimeOrdinal class.
// We have enough members like std::unordered_map<uint64_t, stuff> foo_[kPortCount] to make this
// refactor worthwhile. This would allow replacing those members with a single map that owns
// BufferLifetimeOrdinal(s). The ~BufferLifetimeOrdinal would be a nice place to put common ordinal
// cleanup / reset. Current member fields that track aspects of only the latest ordinal wouldn't
// strictly need to go in BufferLifetimeOrdinal, but we could still put them there and clear them
// out when the BufferLifetimeOrdinal is no longer current. Essentially all real streams have few
// concurrently-allocated ordinals, so there's no concern re. wasted space for fields that are only
// needed for the latest ordinal.
//
// CodecBuffer could potentially be created as soon as AddBufferInternal, with the caveat that we'd
// need to be careful not to trust the VMO too much until GetVmoInfo succeeds. This caveat seems
// fairly significant, so keeping AddingBuffer for now.

class ScopedLock;
// CodecImpl is final; CodecAdapter interface is not final and get sub-classed by CodecImpl client
// code to implement a specific codec. See also CodecAdapter.
class CodecImpl final : public fuchsia::media::StreamProcessor,
                        public CodecAdapterEvents,
                        private CodecAdapter {
 public:
  using StreamProcessorParams =
      std::variant<fuchsia::mediacodec::CreateDecoder_Params,
                   fuchsia::mediacodec::CreateEncoder_Params, fuchsia::media::drm::DecryptorParams>;

  // The CodecImpl will take care of doing set_error_handler() on the sysmem
  // connection.  The sysmem connection should be set up to use the
  // shared_fidl_dispatcher.
  //
  // The shared_fidl_thread parameter is deprecated and ignored and may be
  // thrd_t{} or just {} at the call site.
  //
  // The shared_fidl_dispatcher must be a single-threaded dispatcher (such as
  // async::Loop::dispatcher()) or a synchronized dispatcher (such as
  // fdf::SynchronizedDispatcher::async_dispatcher()), meaning it will only run
  // one task at a time, in the same order as tasks are posted. CodecImpl does
  // support synchronized dispatchers that don't always call using the same
  // thread, just the same sequence/ordering domain. Fwiw, this is why we called
  // them "ordering domain"s in the first place. Ideally all
  // async_dispatcher_t(s) would be updated to support "sequence" ops so we
  // could just call it "sequence" instead of sequence/ordering domain/thread,
  // but anyway.
  //
  // The calling thread must be running under the shared_fidl_dispatcher
  // (sequence/thread).
  CodecImpl(fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem,
            std::unique_ptr<CodecAdmission> codec_admission,
            async_dispatcher_t* shared_fidl_dispatcher, thrd_t shared_fidl_thread,
            StreamProcessorParams params,
            fidl::InterfaceRequest<fuchsia::media::StreamProcessor> request);

  // If the client code calls SetSharingFidlDomainForCoreCodec or returns true from
  // IsSupportsDynamicBuffers, ~CodecImpl must not be called until CodecImpl has called the
  // error_handler passed to BindAsync (during is fine, a bit later async is fine).
  //
  // The client code of CodecImpl can cause the error handler to be called asap async by calling
  // UnbindAsync.
  //
  // For drivers, the above requirements also implicitly require the driver framework to support
  // async stop, which DFv2 PrepareStop does support. In contrast, DFv1 initially didn't have async
  // stop which necessitated supporting ~CodecImpl at any time from the fidl thread, and we still
  // currently support ~CodecImpl at any time for the existing DFv1 drivers (despite DFv1's current
  // support for async DdkUnbind), but only when/while SetSharingFidlDomainForCoreCodec isn't used
  // and !IsSupportsDynamicBuffers.
  //
  // DFv2 is strongly recommended for any driver wanting to call SetSharingFidlDomainForCoreCodec or
  // return true from IsSupportsDynamicBuffers, but it may be technically possible to use DFv1 if
  // DFv1 can/does (now) fully support async stop in all cases short of driver process abort().
  //
  // Client code which doesn't call SetSharingFidlDomainForCoreCodec and doesn't return true from
  // IsSupportsDynamicBuffers may call ~CodecImpl on the fidl thread at any time, but support for
  // this is only retained for legacy reasons, so support for this may be removed at some point. New
  // or updated client code should strongly prefer to use UnbindAsync and only run ~CodecImpl in
  // response to the BindAsync error_handler (even if the new client code doesn't call
  // SetSharingFidlDomainForCoreCodec and doesn't return true from IsSupportsDynamicBuffers).
  ~CodecImpl();

  // If called, must be called during setup shortly after construction. This informs CodecImpl that
  // the shared_fidl_dispatcher thread will also be used as the core codec processing thread / input
  // domain.
  //
  // In ~CodecImpl (called on fidl thread), we enforce that if this method was called, we've already
  // completed the UnbindAsync sequence, so that during ~CodecImpl the fidl thread won't need to
  // wait on the StreamControl thread which in turn is allowed to wait on core codec processing
  // which would be a potential deadlock if core codec processing is also on the fidl thread (fidl
  // thread could end up waiting for itself).
  void SetSharingFidlDomainForCoreCodec() {
    std::lock_guard<std::mutex> lock(checker_core_codec_lock_);
    // CaptureCoreCodecOrderingDomain or SetSharingFidlDomainForCoreCodec, not both.
    ZX_DEBUG_ASSERT(!is_capture_core_codec_ordering_domain_called_);
    is_sharing_fidl_domain_for_core_codec_ = true;
  }

  // Callers that call SetSharingFidlDomainForCoreCodec must not call this method.
  //
  // Callers that don't call SetSharingFidlDomainForCoreCodec() _should_ call this method to set the
  // core codec dispatcher, if available. If called, this must be called on the core codec sequence
  // or thread, and should be called asap after CodecImpl construction (async is fine; just be aware
  // that fully robust checking in CodecImpl won't happen until this is called).
  //
  // The calling_core_codec_dispatcher must be both (a) the core codec ordering domain dispatcher
  // and (b) the dispatcher managing the current calling thread.
  //
  // The calling_core_codec_dispatcher must remain valid until ~CodecAdapter. CodecImpl won't post
  // any tasks to the calling_core_codec_dispatcher; this is only used for checking synchronization
  // of inbound calls (on CodecImpl directly as appropriate and via CodecAdapterEvents).
  void CaptureCoreCodecOrderingDomain(async_dispatcher_t* calling_core_codec_dispatcher);

  // When either SetSharingFidlDomainForCoreCodec is called or IsSupportsDynamicBuffers() returns
  // true, this is the only valid way for client code to head toward ~CodecImpl. Just calling
  // ~CodecImpl from the FIDL thread without the BindAsync error handler having been called is not
  // allowed unless SetSharingFidlDomainForCoreCodec was never called and
  // !IsSupportsDynamicBuffers() (and only for legacy reasons).
  //
  // After calling this, the caller can later run ~CodecImpl safely in the error_handler passed to
  // BindAsync or shortly after async. This is only valid to call after calling BindAsync. The
  // error_handler may run because BindAsync failed async, or because UnbindAsync sequence is
  // calling the error_handler, but the caller doesn't need to care which. If called, must be called
  // on the fidl thread.
  void UnbindAsync();

  // This is only intended for use by LocalCodecFactory in creating the appropriate CodecAdapter.
  [[nodiscard]] std::mutex& lock();

  void SetLifetimeTracking(std::vector<zx::eventpair> lifetime_tracking_eventpair);

  // The LocalCodecFactory optionally calls this method between construction and
  // SetCoreCodecAdapter().  If this method is not called, CodecImpl will treat
  // any call to onCoreCodecLogEvent() as a nop, and will not require that the
  // CodecAdapter sub-class implement CoreCodecMetricsImplementation().
  void SetCodecMetrics(CodecMetrics* codec_metrics);

  // The LocalCodecFactory calls this method once just after CodecImpl
  // construction and just before BindAsync().
  //
  // There's only one CodecAdapter for the lifetime of the CodecImpl.  This
  // mechanism intentionally doesn't permit switching input format to a
  // completely different format, and a CodecAdapter is free to reject any
  // format change it wants to reject.  Before giving up, a client that uses
  // per-stream input format overrides should go around one more time with a
  // freshly created Codec created directly with the new format if the client
  // gets a Codec failure having overridden the input format on a stream of a
  // Codec such that the stream's input format doesn't exactly match the Codec's
  // input format (at least for now).
  void SetCoreCodecAdapter(std::unique_ptr<CodecAdapter> codec_adapter);

  // The LocalCodecFactory optionally calls this method after SetCoreCodecAdapter() and before
  // CoreCodecInit(). This method is a passthrough to the underlying
  // CodecAdapter::SetCodecDiagnostics() method. Note that the codec does not retain any ownership
  // of the CodecDiagnostics. The pointer is guaranteed to live longer than the codec_impl and if
  // this method is called, the pointer will not be nullptr.
  void SetCodecDiagnostics(CodecDiagnostics* codec_diagnostics) override;

  // BindAsync()
  //
  // This enables serving Codec (soon).
  //
  // Must be called on shared_fidl_thread.
  //
  // It remains permitted to cause ~CodecImpl (on shared_fidl_thread) after this call.
  //
  // The core codec initialization and actual binding occur shortly later async after the start of
  // this call, possibly after this call has returned.  This is to avoid core codec initialization
  // slowing down the shared_fidl_thread() which may be handling other stream data for a different
  // CodecImpl instance.
  //
  // Any error, including those encountered before binding is fully complete, will call
  // error_handler on a clean stack on shared_fidl_thread(), after this call (also on
  // shared_fidl_thread()) returns.  If the client code runs ~CodecImpl on shared_fidl_thread
  // instead (before error_handler has run on shared_fidl_thread), the error_handler will be deleted
  // without being run.
  //
  // The error_handler runs on the fidl dispatcher (see CodecImpl constructor) and is expected to
  // trigger ~CodecImpl to run, either synchronously during error_handler(), or shortly after async.
  // In other words it's the responsibility of client code to delete the CodecImpl in a timely
  // manner during or soon after error_handler().  Until ~CodecImpl, the CodecAdmission won't be
  // released, and the channel itself won't be closed (intentionally, to ensure the old instance is
  // cleaned up before a new instance is created based on a client retry triggered by server channel
  // closure).
  //
  // When either SetSharingFidlDomainForCoreCodec is called or IsSupportsDynamicBuffers() returns
  // true, client code must not run ~CodecImpl until error_handler is called. See also UnbindAsync
  // and ~CodecImpl.
  void BindAsync(fit::closure error_handler);

  //
  // Codec interface
  //
  void EnableOnStreamFailed() override;
  void SetInputBufferPartialSettings(
      fuchsia::media::StreamBufferPartialSettings input_settings) override;
  void SetOutputBufferPartialSettings(
      fuchsia::media::StreamBufferPartialSettings output_settings) override;
  void CompleteOutputBufferPartialSettings(uint64_t buffer_lifetime_ordinal) override;
  void FlushEndOfStreamAndCloseStream(uint64_t stream_lifetime_ordinal) override;
  void CloseCurrentStream(uint64_t stream_lifetime_ordinal, bool release_input_buffers,
                          bool release_output_buffers) override;
  void Sync(SyncCallback callback) override;
  void RecycleOutputPacket(fuchsia::media::PacketHeader available_output_packet) override;
  void QueueInputFormatDetails(uint64_t stream_lifetime_ordinal,
                               fuchsia::media::FormatDetails format_details) override;
  void QueueInputPacket(fuchsia::media::Packet packet) override;
  void QueueInputEndOfStream(uint64_t stream_lifetime_ordinal) override;
  //
  // These are not sent by correctly-operating clients unless
  // fuchsia.mediacodec/DetailedCodecDescription.supports_dynamic_buffers is set
  // to true. See codec_factory.fidl and stream_processor.fidl.
  //
  void ParticipateInBufferAllocation(
      fuchsia::media::StreamProcessorParticipateInBufferAllocationRequest request) override;
  void AddBuffer(fuchsia::media::StreamProcessorAddBufferRequest request) override;
  void RemoveBuffer(fuchsia::media::StreamProcessorRemoveBufferRequest request,
                    RemoveBufferCallback callback) override;
  void EnableOldOutputBuffers() override;
  void EnableSameOutputBufferConcurrentlyInFlight() override;
  void EnableForceOutputBuffersFixedImageSize() override;
  // Currently this will log and close the channel, because currently for a
  // protocol whose clients and servers are both "platform" and "external", the
  // abi_compat tool prevents adding a strict message, forcing all new
  // StreamProcessor messages to be added as "flexible". So currently we have to
  // assume every unrecognized message is logically strict despite not being
  // "strict" in FIDL.
  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override;

  // These are public so that CodecBuffer doesn't have to be a friend of CodecImpl.

  // This way CodecBuffer doesn't use the core_codec_bti_ directly.
  [[nodiscard]] zx_status_t Pin(uint32_t options, const zx::vmo& vmo, uint64_t offset,
                                uint64_t size, zx_paddr_t* addrs, size_t addrs_count, zx::pmt* pmt);

  // Complain sync, then Unbind() async.  Even if more than one caller
  // complains, the async Unbind() work will only run once (but in such cases it
  // can be nice to see all the complaining in case multiple things fail at
  // once).  While more than one source of failure can complain, only one will
  // actually trigger Unbind() work, and the rest will just return knowing that
  // Unbind() work is started.  The Unbind() work itself will synchronize such
  // that other-thread sources of failure are no longer possible (can no longer
  // even complain) before deallocating "this".
  //
  // Callers to Fail() must not be holding lock_.  On return from Fail(), "this"
  // must not be touched as it can already be deallocated.
  void Fail(const char* format, ...) __TA_EXCLUDES(lock_);

  // Callers to FailLocked() must hold lock_ during the call.  On return from
  // FailLocked(), the caller can know that "this" is still allocated only up
  // to the point where the caller releases lock_.  Callers are encouraged not
  // to touch "this" after the call to FailLocked() besides releasing lock_,
  // for consistency with how Fail() is used; that said, the unlock itself is
  // safe.
  void FailLocked(const char* format, ...) __TA_REQUIRES(lock_);
  // Report a devhost-fatal error.  This method never returns - instead we
  // fault the whole process.  This should only be used in cases where we
  // don't really expect an error, and where a client can't unilaterally induce
  // the error - but in case the error happens despite not being expected, we
  // want nice output that's easy to debug.
  void FailFatal(const char* format, ...) __TA_EXCLUDES(lock_);

  [[nodiscard]] bool is_supports_dynamic_buffers() const {
    return ::codec_impl::internal::kEnableDynamicBuffers && is_supports_dynamic_buffers_;
  }

 private:
  class AddingBuffer;

  using BuffersByIndex = std::unordered_map<uint32_t, std::unique_ptr<CodecBuffer>>;
  using BuffersByOrdinal = std::unordered_map<uint64_t, BuffersByIndex>;
  // The index in the vector is the allocated_packet_index, not the protocol_packet_index.
  using PacketsByIndex = std::vector<std::unique_ptr<CodecPacket>>;
  using PacketsByOrdinal = std::unordered_map<uint64_t, PacketsByIndex>;
  using FakeMapRangesByOrdinal = std::unordered_map<uint64_t, std::unique_ptr<FakeMapRange>>;
  using AddingBuffersByIndex = std::unordered_map<uint32_t, std::shared_ptr<AddingBuffer>>;
  using AddingBuffersByOrdinal = std::unordered_map<uint64_t, AddingBuffersByIndex>;
  using ProtocolPacketsByIndex = std::unordered_map<uint32_t, CodecPacket*>;
  using ProtocolPacketsByOrdinal = std::unordered_map<uint64_t, ProtocolPacketsByIndex>;

  // This can't be defined separately from CodecImpl because TA annotations aren't (yet?) up to the
  // task of creating a "dual" of ScopedLock or "MutexLocker". This is because TA annotations can
  // only deal with aliases within SCOPED_CAPABILITY, not for any other annotation. Also, because
  // inline-defined methods (including inline-defined destructors) don't get looked into for
  // purposes of TA annotation analysis. See also the clang documentation here:
  // https://clang.llvm.org/docs/ThreadSafetyAnalysis.html#no-alias-analysis
  //
  // So, rather than engage in quixotic battle with TA annotations (as they exist as of this
  // comment), we can define this ScopedUnlock within CodecImpl and directly annotate that the
  // constructor unlocks lock_ and the destructor locks lock_. This makes ScopedUnlock completely
  // useelss for anything involving any other lock, but also makes it work within the limitations of
  // TA annotations, which we do want to keep using, so seems a good tradeoff (until TA annotations
  // can support the more generically applicable version of this).
  //
  // This can still confuse the TA analysis at the usage site regarding the locking status of lock_
  // after ~ScopedLock (due to lack of alias analysis for any case other than the specific pattern
  // of SCOPED_CAPABILITY), but in those cases a ScopedLock.AssertHeld(lock_) can be added after
  // ~ScopedUnlock at the usage site.
  class ScopedUnlock {
   public:
    explicit ScopedUnlock(CodecImpl& parent) noexcept __TA_RELEASE(parent.lock_) : parent_(parent) {
      parent_.lock_.unlock();
    }

    // It's counterproductive to put a __TA_ACQUIRE(parent_.lock_) here because that just confuses
    // the TA annotations more than it helps, due to lack of TA alias analysis (to date). The call
    // site can use ScopedLock::AssertHeld(lock_) after this destructor to get TA analysis back in
    // sync with reality.
    ~ScopedUnlock() noexcept { parent_.lock_.lock(); }

   private:
    CodecImpl& parent_;

    DISALLOW_COPY_ASSIGN_AND_MOVE(ScopedUnlock);
  };

  class AddingBuffer : public std::enable_shared_from_this<AddingBuffer> {
   public:
    AddingBuffer(zx::vmo unverified_vmo) : unverified_vmo_(std::move(unverified_vmo)) {}

    AddingBuffer(const AddingBuffer& to_copy) = delete;
    AddingBuffer& operator=(const AddingBuffer& to_copy) = delete;
    AddingBuffer(AddingBuffer&& to_move) = default;
    AddingBuffer& operator=(AddingBuffer&& to_move) = default;

    // If this is set, when GetVmoInfo completes, the add will drop the buffer
    // (and all tracking of the buffer) and complete the remove instead of
    // completing the rest of the add. We disallow another AddBuffer of the same
    // buffer_index until a subsequent RemoveBuffer of that buffer_index has
    // completed, so the memory usage of this doesn't grow without bound under
    // adverse client behavior. The same could not be said for any approach that
    // leaves GetVmoInfo in flight while completing the remove back to the
    // client - we intentionally don't want to do that. Prevention of unbounded
    // memory is more important than minimizing latency of a RemoveBuffer sent
    // shortly after the corresponding AddBuffer. Once we start the GetVmoInfo,
    // there's no way in idiomatic FIDL to cancel that, so we must wait for it
    // to be done before completing the remove, as that's the only way to clean
    // up the memory usage before declaring the remove done.
    //
    // This is part of preventing a synchronous round-trip to/from sysmem for
    // each added buffer (vs calling GetVmoInfo synchronously one at a time).
    //
    // This starts un-set, and is only set if a RemoveBuffer comes in before the
    // AddBuffer work is done.
    fit::function<void(ScopedLock&)> continue_remove_;

    // To handle must_match_vmo field set, we need a VMO handle corresponding to
    // a previous AddBuffer which may not yet be done with GetVmoInfo, so we put
    // the adding buffer's original handle here so it can be found and
    // duplicated while GetVmoInfo is in progress. If this wasn't a
    // sysmem-provided VMO in the first place, then that'll cause both the
    // GetVmoInfo and the new allocation setting must_match_vmo to fail, which
    // is fine. The client can avoid this possibility (if it really needs to) by
    // using GetVmoInfo itself before sending a VMO handle to AddBuffer (at the
    // cost of an extra round trip to/from sysmem).
    zx::vmo unverified_vmo_;

    // Any other information needed to continue the add is via closure captures
    // established by the AddBuffer handler.
  };

  template <typename Protocol>
  class AsyncEventHandler : public fidl::AsyncEventHandler<Protocol> {
   public:
    using ErrorFunction = fit::function<void(fidl::UnbindInfo)>;
    explicit AsyncEventHandler(ErrorFunction error_function = nullptr)
        : error_function_(std::move(error_function)) {}
    void set_error_handler(ErrorFunction error_function) {
      error_function_ = std::move(error_function);
    }
    [[nodiscard]] bool is_error_handler_set() { return !!error_function_; }

   private:
    void on_fidl_error(fidl::UnbindInfo error) override {
      // Client code must set an error function before binding.
      ZX_DEBUG_ASSERT(error_function_);
      // move locally so doesn't get deallocated while running
      auto local_error_function = std::move(error_function_);
      local_error_function(error);
    }
    ErrorFunction error_function_;
  };

  // the order of base classes is significant; the AsyncEventHandler is a base instead of a member
  // to make sure the destructor runs after ~fidl::Client
  template <typename Protocol>
  class Client : private AsyncEventHandler<Protocol>, public fidl::Client<Protocol> {
   public:
    using ErrorFunction = typename AsyncEventHandler<Protocol>::ErrorFunction;
    Client() = default;

    // No move because AsyncEventHandler* is held by fidl::Client.
    Client(Client&& to_move) = delete;
    Client& operator=(Client&& to_move) = delete;
    // No copy
    Client(const Client& to_copy) = delete;
    Client& operator=(const Client& to_copy) = delete;

    void Bind(fidl::ClientEnd<Protocol> client_end, async_dispatcher_t* dispatcher,
              typename AsyncEventHandler<Protocol>::ErrorFunction on_error) {
      ZX_DEBUG_ASSERT(client_end.is_valid());
      ZX_DEBUG_ASSERT(dispatcher);
      ZX_DEBUG_ASSERT(!!on_error);
      AsyncEventHandler<Protocol>::set_error_handler(std::move(on_error));
      fidl::Client<Protocol>::Bind(std::move(client_end), dispatcher, this);
    }

    void handle_unknown_event(fidl::UnknownEventMetadata<Protocol> metadata) override {
      // old clients aren't required to pay attention to any too-new events; ignore
      return;
    }
  };

  // We keep a queue of Stream objects rather than just a single current stream
  // object, so we can track which streams are future-discarded and which are
  // not yet known to be future-discarded.  This difference matters because
  // clients are not required to process OnOutputConstraints() with
  // stream_lifetime_ordinal of a stream that the client has since told the
  // server to discard, so we don't want StreamControl ordering domain getting
  // stuck waiting on a client to catch up to an output config that the client
  // won't process.  Instead, the StreamControl ordering domain can ignore any
  // additional messages related to the discarded stream until the stream
  // discarding message is reached at which point the core codec's mid-stream
  // output config change is cancelled/forgotten when we reset the core codec.
  //
  // In addition, if we're behind, we can catch up by skipping past some
  // messages for future-discarded streams to catch up to non-discarded stream
  // input quicker.  Theoretically we could do even better by having the FIDL
  // thread delete messages previously queued to the StreamControl domain
  // regarding a stream that is now known to be discarded by the FIDL thread,
  // and collapse/combine CloseCurrentStream() messages, but that's unlikely to
  // help much in practice and would make the implementation more difficult to
  // read, and we can mitigate unbounded queuing by demanding that clients not
  // get too far ahead else we close the channel.  While forcing a client to
  // wait isn't great, if we don't, we can't impose a circuit-breaker limit on
  // the count and/or size of queued channel messages either - ideally setting
  // such a limit should be possible for any protocol, so at some convenient
  // point the client needs to wait or postpone, but only if the client is
  // written to be able to get far ahead in the first place.
  //
  // We also keep some stream-specific tracking information in here as a
  // reasonably clean way to ensure that a new stream's tracking info is
  // initialized properly.
  class Stream {
   public:
    // These mutations occur in Output ordering domain (shared_fidl_thread()):
    explicit Stream(const CodecImpl* const parent, uint64_t stream_lifetime_ordinal);
    void AssertHeld(const CodecImpl* const parent) __TA_REQUIRES(parent->lock_)
        __TA_ASSERT(parent_->lock_);

    [[nodiscard]] uint64_t stream_lifetime_ordinal();
    void SetFutureDiscarded() __TA_REQUIRES(parent_->lock_);
    __WARN_UNUSED_RESULT bool future_discarded() __TA_REQUIRES(parent_->lock_);
    // writing to the bool via the returned value is not permitted
    __WARN_UNUSED_RESULT std::shared_ptr<std::atomic<bool>> shared_future_discarded()
        __TA_REQUIRES(parent_->lock_);
    void SetFutureFlushEndOfStream() __TA_REQUIRES(parent_->lock_);
    __WARN_UNUSED_RESULT bool future_flush_end_of_stream() __TA_REQUIRES(parent_->lock_);

    // These mutations occur in StreamControl ordering domain:
    ~Stream();
    // This can be called 0-N times for a given stream, and each call replaces
    // any previously-set details.
    void SetInputFormatDetails(std::unique_ptr<fuchsia::media::FormatDetails> input_format_details);
    // Can be nullptr if no per-stream details have been set, in which case the
    // caller should look at CodecImpl::initial_input_format_details_
    // instead.  The returned pointer is only valid up until the next call to to
    // SetInputFormatDetails() or when the stream is deleted, whichever comes
    // first.  This is only meant to be called on stream_control_thread_.
    [[nodiscard]] const fuchsia::media::FormatDetails* input_format_details();
    // We send oob_bytes (if any) to the core codec just before sending a
    // packet to the core codec, but only when the stream has OOB data pending.
    // A new stream has OOB data initially pending, and it becomes pending again
    // if SetInputFormatDetails() is used and the oob_bytes don't match
    // the effective oob_bytes before.  This way we avoid causing extra
    // input format changes for the core codec.
    void SetOobConfigPending(bool pending);
    __WARN_UNUSED_RESULT bool oob_config_pending();
    void SetInputEndOfStream();
    __WARN_UNUSED_RESULT bool input_end_of_stream();
    void SetOutputEndOfStream();
    __WARN_UNUSED_RESULT bool output_end_of_stream();
    void SetFailureSeen();
    __WARN_UNUSED_RESULT bool failure_seen();

    // These methods are called on the core codec processing domain.  See also
    // comments on output_format_pending_.
    void SetOutputFormatPending();
    void ClearOutputFormatPending();
    __WARN_UNUSED_RESULT bool output_format_pending();

   private:
    friend class CodecImpl;

    // The parent_ field is only for __TA_GUARDED() usage below.
    const CodecImpl* const parent_ = nullptr;

    const uint64_t stream_lifetime_ordinal_ = 0;

    // This is accessed at arbitrary times from output thread (FIDL thread) and StreamControl
    // thread, so we need to be holding lock_ to access the field itself.
    //
    // We drop output items associated with a stream that's been future_discarded, mainly to allow
    // paused_output_.reset() without incorrectly sending output of a stream which saw a mid-stream
    // constraints change but never achieved IsOutputConfiguredLocked() true before the client moved
    // on to a new stream instead. This is why this is a shared_ptr<bool>, so that output items that
    // have been released by paused_output_.reset() can determine whether to send or self-cancel.
    //
    // The shared_ptr-ness also avoids sending output that the client doesn't care about any more,
    // but clients must tolerate old output from a stream the client knows won't exist once the
    // server catches up, so this aspect isn't the main reason for the shared_ptr.
    std::shared_ptr<std::atomic<bool>> future_discarded_ __TA_GUARDED(parent_->lock_) =
        std::make_shared<std::atomic<bool>>(false);
    bool future_flush_end_of_stream_ __TA_GUARDED(parent_->lock_) = false;

    // Starts as nullptr for each new stream with implicit fallback to
    // initial_input_format_details_, but can be overridden on a per-stream
    // basis with QueueInputFormatDetails().
    std::unique_ptr<fuchsia::media::FormatDetails> input_format_details_;
    // This defaults to _true_, so that we send the OOB bytes to the HW for each
    // stream, if we have any oob_bytes to send.
    bool oob_config_pending_ = true;
    bool input_end_of_stream_ = false;
    bool output_end_of_stream_ = false;
    bool failure_seen_ = false;

    // This defaults to _true_, so that we send OnOutputFormat() before the
    // first OnOutputFormat() of a stream.  We also set this back to true any
    // time the core codec indicates onOutputFormat(), and any time the core
    // codec indicates onCoreCodecMidStreamOutputConstraintsChange() with action
    // required true.
    bool output_format_pending_ = true;
  };

  // PortSettings
  //
  // When not using dynamic buffers, the PortSettings wraps the port settings
  // specified in StreamBufferPartialSettings.
  //
  // When using dynamic buffers, we determine important settings from the first
  // buffer using sysmem GetVmoInfo, and confirm that other buffers have
  // matching un when not using dynamic buffers.
  class PortSettings {
   public:
    // Used when not dynamic buffers.
    PortSettings(CodecImpl* parent, CodecPort port,
                 fuchsia::media::StreamBufferPartialSettings partial_settings);

    // Used when dynamic buffers. When the first buffer completes GetVmoInfo, we
    // set a CodecImpl-synthesized BufferCollectionInfo that has the
    // sysmem-provided SingleBufferSettings with coherency_domain and is_secure.
    PortSettings(CodecImpl* parent, CodecPort port, uint64_t buffer_constraints_version_ordinal,
                 uint64_t buffer_lifetime_ordinal);

    // False if the first constructor above was used. True if the second
    // constructor above was used.
    [[nodiscard]] bool is_dynamic() { return is_dynamic_; }

    ~PortSettings();

    [[nodiscard]] uint64_t buffer_lifetime_ordinal();

    [[nodiscard]] uint64_t buffer_constraints_version_ordinal();

    // This only constrains packet_index values when not using dynamic buffers.
    [[nodiscard]] uint32_t packet_count();

    // This is the number of buffers in buffer_collection_info_ when not using
    // dynamic buffers, so in that case it's also the max.
    //
    // When using dynamic buffers, this is 1, but the client should ensure that
    // sufficient buffers are available soon to the codec per
    // buffer_count_for_server_current or
    // dynamic_buffers_video_decoder_output_safe.
    [[nodiscard]] uint32_t min_buffer_count();

    // When not using dynamic buffers, this is the coherency domain from the one
    // buffer_collection_info_.
    //
    // When using dynamic buffers, CodecImpl ensures this coherency domain is
    // (and remains) the same for all buffers added with AddBuffer under the
    // same buffer_lifetime_ordinal, else we close the StreamProcessor channel.
    [[nodiscard]] fuchsia_sysmem2::CoherencyDomain coherency_domain();

    // only called when not dynamic buffers
    [[nodiscard]] const fuchsia::media::StreamBufferPartialSettings& partial_settings();

    // only called when not dynamic buffers
    [[nodiscard]] fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> TakeToken();

    // The caller should std::move() in the buffer_collection_info.  This call
    // is only valid if this method hasn't been called before on this instance.
    void SetBufferCollectionInfo(fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info);
    void ClearBufferCollectionInfo();
    [[nodiscard]] const fuchsia_sysmem2::BufferCollectionInfo& buffer_collection_info() const;

    // only called when not dynamic buffers
    //
    // We use SetBufferCollectionInfo(), but then take the VMOs back.  This
    // just happens to be more convenient than taking the VMOs before doing
    // SetBufferCollectionInfo() (when not dynamic buffers).
    [[nodiscard]] zx::vmo TakeVmo(uint32_t buffer_index);

    // only called when not dynamic buffers
    [[nodiscard]] uint64_t vmo_usable_start(uint32_t buffer_index);
    // only called when not dynamic buffers
    [[nodiscard]] uint64_t vmo_usable_size();

    // When not dynamic buffers, called only after SetBufferCollectionInfo.
    //
    // When dynamic buffers, value set by parameter to the constructor.
    [[nodiscard]] bool is_secure();

    // only called when not dynamic buffers
    //
    // Only call from FIDL thread.
    [[nodiscard]] fidl::ServerEnd<fuchsia_sysmem2::BufferCollection> NewBufferCollectionRequest(
        async_dispatcher_t* dispatcher,
        CodecImpl::Client<fuchsia_sysmem2::BufferCollection>::ErrorFunction on_error);

    // only called when not dynamic buffers
    //
    // Only call from FIDL thread.
    [[nodiscard]] std::unique_ptr<Client<fuchsia_sysmem2::BufferCollection>>& buffer_collection();

    // only called when not dynamic buffers
    //
    // Only call from FIDL thread.
    void UnbindBufferCollection();

    // When not dynamic buffers, this condition is necessary (but not
    // sufficient) for IsOutputConfiguredLocked() to return true.
    //
    // When dynamic buffers, this will just return true since PortSettings won't exist until we
    // have at least one buffer.
    [[nodiscard]] bool is_complete_seen_output();
    void SetCompleteSeenOutput();

   private:
    bool is_dynamic_ = false;

    CodecImpl* parent_ = nullptr;
    CodecPort port_ = kInvalidPort;

    std::unique_ptr<fuchsia::media::StreamBufferPartialSettings> partial_settings_;

    uint64_t buffer_constraints_version_ordinal_ = 0;
    uint64_t buffer_lifetime_ordinal_ = 0;

    // This is in a unique_ptr<> because ~PortSettings does an async post to the fidl thread to send
    // a Release().
    std::unique_ptr<Client<fuchsia_sysmem2::BufferCollection>> buffer_collection_;

    // In the case of partial_settings_, the remainder of the settings arrive
    // from sysmem in a BufferCollectionInfo_2.  When that arrives from
    // sysmem, we move the VMOs into CodecBuffer(s), and the remainder of the
    // settings get stored here.
    std::unique_ptr<fuchsia_sysmem2::BufferCollectionInfo> buffer_collection_info_;

    bool is_complete_seen_output_ = false;
  };

  struct SysmemBufferId {
    // the order of these fields is consistent with little-endian storage of a __uint128_t, putting
    // buffer_collection_id in the high-order 64 bits as we do below. However, we never use a union
    // or rely on this ordering in the source. This ordering may or may not help the compiler skip
    // some steps during pack/unpack when building for little-endian.
    uint64_t sysmem_buffer_index;
    uint64_t sysmem_buffer_collection_id;
  };
  // We pack <buffer_collection_id, padding, buffer_index> into a __uint128_t (gcc and clang support
  // this type) to avoid unnecessary extra code for comparison, hashing, etc.
  using PackedSysmemBufferId = __uint128_t;
  [[nodiscard]] static PackedSysmemBufferId PackedBufferIdFrom(SysmemBufferId buffer_id) {
    // padding isn't mentioned here but it ends up 0
    return static_cast<PackedSysmemBufferId>(buffer_id.sysmem_buffer_collection_id) << 64 |
           static_cast<PackedSysmemBufferId>(buffer_id.sysmem_buffer_index);
  }
  [[nodiscard]] static SysmemBufferId BufferIdFrom(PackedSysmemBufferId packed_buffer_id) {
    return SysmemBufferId{
        .sysmem_buffer_index =
            static_cast<uint64_t>(packed_buffer_id & std::numeric_limits<uint64_t>::max()),
        .sysmem_buffer_collection_id = static_cast<uint64_t>(packed_buffer_id >> 64),
    };
  }

  // While we list this first in the member variables to hint that this gets
  // destructed last, the actual mechanism of destruction of the CodecAdmission
  // is via posting to the shared_fidl_thread(), because if we add more stuff in
  // various base classes of this class we want the destruction of
  // CodecAdmission to happen last.  The close processing won't be considered
  // done until after this is destructed.
  //
  // See codec_admission_control.h for comments re. how we'll avoid failing a
  // create that is requested by a client shortly after the client closes the
  // previous Codec channel, when there's a concurrency cap of 1 (for example).
  std::unique_ptr<CodecAdmission> codec_admission_;

  Client<fuchsia_sysmem2::Allocator> sysmem_;

  async_dispatcher_t* shared_fidl_dispatcher_ = nullptr;
  // Nearly every task we post to shared_fidl_dispatcher_ is actually posted via
  // this ClosureQueue, which is how we avoid running previously-queued lambdas
  // that capture "this" or part of "this" after "this" is already gone.  The
  // ~CodecImpl ensures that task deletion occurs _before_ most of ~CodecImpl by
  // calling shared_fidl_queue_.StopAndClear().
  ClosureQueue shared_fidl_queue_;
  std::atomic<bool> is_sharing_fidl_domain_for_core_codec_ = false;

  // This is a class rather than a std::monostate because the destructor kicks
  // output. This isn't actually pausing output until/unless a weak pointer to
  // an instance of this class is put in maybe_weak_paused_output_.
  class PausedOutput : std::enable_shared_from_this<PausedOutput> {
   public:
    PausedOutput(CodecImpl& parent);
    ~PausedOutput();

    // PausedOutput is always allocated as a shared_ptr<PausedOutput>; no
    // copying or moving.
    PausedOutput(const PausedOutput& to_copy) = delete;
    PausedOutput& operator=(const PausedOutput& to_copy) = delete;
    PausedOutput(PausedOutput&& to_move) = delete;
    PausedOutput& operator=(PausedOutput&& to_move) = delete;

    CodecImpl& parent_;
  };
  std::queue<fit::closure> output_queue_;
  // Only read or written on shared_fidl_thread_. The ability to lock() depends
  // whether the shared_ptr keeping output paused is still held, or dropped.
  std::optional<std::weak_ptr<PausedOutput>> maybe_weak_paused_output_;

  // Parts of CodecImpl are accessed from shared_fidl_thread(),
  // stream_control_thread_, and decoder thread(s) such as interrupt handling
  // thread(s).
  //
  // FXL_GUARDED_BY() is not directly usable in this class because this class
  // takes advantage of for example being able to read outside the lock from
  // something that can only be modified on the current thread.  Also, which
  // thread is relevant can vary by port, while FXL_GUARDED_BY() doesn't have
  // any way to tag indexes of an array differently.
  //
  // TODO(dustingreen): Implement some lock-like contexts including reader vs.
  // writer aspects so we can use FXL_GUARDED_BY() (just not with the lock
  // directly).
  //
  // TODO(dustingreen): Switch to fbl::Mutex and fbl::ConditionVariable, because
  // they complain instead of blocking if repeated acquisition is attempted, and
  // because one can check whether the current thread holds the lock (for assert
  // purposes).
  std::mutex lock_;

  // These async::synchronization_checker(s) are checked via is_synchronized()
  // in a lot of cases. We may mark __TA_REQUIRES() using these, for some
  // methods whose correct sequence is not port-dependent. However, for methods
  // whose correct sequence (ordering domain) is port-dependent, we'll continue
  // to use is_synchronized(), as known alternatives so far don't seem to offer
  // a good tradeoff (beware the "nerd snipe"; it's a trap, IMHO).
  //
  // When is_sharing_fidl_domain_for_core_codec_, this and the
  // checker_core_codec_ are checking the same thing.
  async::synchronization_checker checker_fidl_;
  std::optional<async::synchronization_checker> checker_stream_control_;
  // This leaf lock is held for short intervals to protect checker_core_codec_
  // and directly-related bools in case calling code has bugs and is calling
  // IsCoreCodec() from various threads concurrently; protects directly-related
  // checks in SetSharingFidlDomainForCoreCodec and
  // CaptureCoreCodecOrderingDomain.
  //
  // Assuming zero bugs in other code, this lock would have no purpose. This is
  // only meant to avoid the checking code creating (additional) UB when we
  // assume existence of which-thread bugs in other code. This lock does nothing
  // (at least not intentionally) to prevent other potential UB due to bugs in
  // other code calling methods on the wrong thread(s). The potential for such
  // UB is why we have the checking in the first place.
  std::mutex checker_core_codec_lock_;
  // If !is_sharing_fidl_domain_for_core_codec_, this will be set the first and
  // only time when we get the first call from the core codec that we know is
  // supposed to be on the core codec sequence / ordering domain / thread.
  std::optional<async::synchronization_checker> checker_core_codec_
      __TA_GUARDED(checker_core_codec_lock_);

  //
  // Setup/teardown aspects.
  //

  // This is where we encapsulate the condition choosing which unbind we do and whether ~CodecImpl
  // at any time on fidl thread is allowed (!IsLegacyUnbind() case).
  //
  // At some point we may want to allow older client code that doesn't share the fidl thread and
  // doesn't support dynamic buffers to select fully async unbind, but we don't yet need to support
  // that.
  [[nodiscard]] bool IsLegacyUnbind() {
    return !is_sharing_fidl_domain_for_core_codec_ && !is_supports_dynamic_buffers();
  }

  // This starts unbinding. When unbinding is done and CodecImpl is ready to be destructed,
  // client_error_handler_ is called (unless calling from ~CodecImpl with IsLegacyUnbind() true in
  // which case client_error_handler_ never runs).
  //
  // UnbindLocked() can be called in response to a channel error (in which case the binding_ itself
  // is already unbound), or can be called in response to a protocol error.  It can be called on any
  // thread.
  //
  // On the caller's release of lock_ after this call, "this" may be deallocated, depending on which
  // thread this is called from. For consistency, all callers should avoid touching any part of
  // "this" after return from this method other than releasing lock_.
  //
  // If the reason for un-binding is a failure, call Fail() or FailLocked() instead, which will log
  // an error before calling UnbindLocked.
  //
  // This internally chooses whether to call UnbindLockedInternalAsync or LegacyUnbindLockedInternal
  // depending on IsLegacyUnbind.
  void UnbindLocked() __TA_REQUIRES(lock_);

  void UnbindLockedInternalAsync() __TA_REQUIRES(lock_);
  // These are the async steps initiated by UnbindLockedInternalAsync. Each step is responsible for
  // triggering the next step, until error_handler is called which completes the last step.
  void AsyncShutdownStepEndStreamAndRemoveInputBuffers() __TA_EXCLUDES(lock_);
  void AsyncShutdownStepWaitForZeroInputBuffersAndEnsureZeroOutputBuffers() __TA_EXCLUDES(lock_);
  void AsyncShutdownStepQuitStreamControl() __TA_EXCLUDES(lock_);

  // This is an older way to accomplish unbind, and is not sufficiently async to be compatible with
  // is_sharing_fidl_domain_for_core_codec_ || is_supports_dynamic_buffers_ (unless UnbindLocked
  // already ran previously and the current stack is in response to the BindAsync error handler
  // being called by CodecImpl). This exists to support older CodecImpl client code which expects to
  // be able to run ~CodecImpl on FIDL thread at any time synchronously. This way exists because of
  // DFv1's historical lack of support for async stop (only initially; the current DFv1 DdkUnbind
  // can complete async now). Instead of calling this method, see UnbindLocked (for internal call
  // sites) and UnbindAsync (for client code to call).
  void LegacyUnbindLockedInternal() __TA_REQUIRES(lock_);

  // Like UnbindLocked(), but acquires the lock so the caller doesn't have to.
  // On return from this method, "this" may already have been deleted.
  void Unbind();
  // Part of the implementation of UnbindLocked() and ~CodecImpl, which ensures
  // that all relevant FIDL bindings are un-bound.  Calls to this method must
  // only occur on the FIDL thread.
  void EnsureUnbindCompleted();

  // TODO(https://fxbug.dev/42110593): This isn't fully hooked up yet, so doesn't actually yet
  // indicate whether buffers are secure.  Enforce that
  // port_settings_[X].is_secure() is consistent with these.
  [[nodiscard]] fuchsia::mediacodec::SecureMemoryMode OutputSecureMemoryMode();
  [[nodiscard]] fuchsia::mediacodec::SecureMemoryMode InputSecureMemoryMode();
  [[nodiscard]] fuchsia::mediacodec::SecureMemoryMode PortSecureMemoryMode(CodecPort port);
  [[nodiscard]] bool IsPortSecureRequired(CodecPort port);
  [[nodiscard]] bool IsPortSecurePermitted(CodecPort port);

  void AddBufferInternal(CodecPort port, uint64_t buffer_constraints_version_ordinal,
                         uint64_t buffer_lifetime_ordinal, uint32_t buffer_index, zx::vmo buffer);
  void RemoveBufferInternal(CodecPort port, uint64_t buffer_lifetime_ordinal, uint32_t buffer_index,
                            RemoveBufferCallback callback);

  // This gets deleted outside lock_ by the caller of MaybeDeleteBufferLifetimeOrdinal. The
  // fake_map_range_to_delete is the main motivation for this struct.
  struct BufferLifetimeOrdinalCleanupOutsideLock {
    fit::deferred_callback callback_to_run_on_delete;
    std::optional<FakeMapRangesByOrdinal::node_type> fake_map_range_to_delete;
    std::optional<AddingBuffersByOrdinal::node_type> adding_buffers_node_handle_to_delete;
    std::optional<BuffersByOrdinal::node_type> buffers_node_handle_to_delete;
    std::optional<PacketsByOrdinal::node_type> packets_node_handle_to_delete;
    std::optional<ProtocolPacketsByOrdinal::node_type> protocol_packets_node_handle_to_delete;

    BufferLifetimeOrdinalCleanupOutsideLock() noexcept = default;
    BufferLifetimeOrdinalCleanupOutsideLock(
        BufferLifetimeOrdinalCleanupOutsideLock&& to_move) noexcept = default;
    BufferLifetimeOrdinalCleanupOutsideLock& operator=(
        BufferLifetimeOrdinalCleanupOutsideLock&& to_move) noexcept = default;

    BufferLifetimeOrdinalCleanupOutsideLock(
        const BufferLifetimeOrdinalCleanupOutsideLock& to_copy) noexcept = delete;
    BufferLifetimeOrdinalCleanupOutsideLock& operator=(
        const BufferLifetimeOrdinalCleanupOutsideLock& to_copy) noexcept = delete;
  };
  // On return, if the return value has_value, the buffer_lifetime_ordinal has already been deleted
  // from member fields of CodecImpl, but any CodecPacket*(s) are still allocated, and the caller
  // should delete the BufferLifetimeOrdinalCleanupOutsideLock outside lock_. We also want to ensure
  // that packets don't get deleted until after CoreCodecCloseBufferLifetimeOrdinal.
  [[nodiscard]] std::optional<BufferLifetimeOrdinalCleanupOutsideLock>
  MaybeDeleteBufferLifetimeOrdinal(CodecPort port, uint64_t buffer_lifetime_ordinal)
      __TA_REQUIRES(lock_);

  CodecMetrics* codec_metrics_ = nullptr;
  std::optional<media_metrics::StreamProcessorEvents2MigratedMetricDimensionImplementation>
      codec_metrics_implementation_dimension_;

  // The CodecAdapter is owned by the CodecImpl, and is listed near the top of
  // the local variables in CodecImpl so that it gets deleted near the end of
  // ~CodecImpl's implicit deletions (just in case, as of this writing).
  //
  // The CodecAdapter must not make any CodecAdapterEvents calls into CodecImpl
  // while there's no active stream, and there will be no active stream by the
  // time ~CodecImpl starts.  The CodecAdapter must be ok with being destructed
  // any time there's no active stream.
  //
  // TODO(dustingreen): Maybe it would be more convenient for the CodecAdapter
  // if CodecImpl made a Shutdown() call on it after stopping the last stream
  // and before destruction - but let's see how this goes without the Shutdown()
  // call for now.
  std::unique_ptr<CodecAdapter> codec_adapter_;
  // This matches codec_adapter_->IsSupportsDynamicBuffers(), but it's nice to
  // avoid calling IsSupportsDynamicBuffers under lock_, just in case.
  bool is_supports_dynamic_buffers_ = false;
  bool is_supports_mid_stream_output_constraints_change_ = true;

  const StreamProcessorParams params_;

  // Regardless of which type of codec was created, these track the input
  // FormatDetails.
  //
  // We keep a copy of the format details used to create the codec, and on a
  // per-stream basis those details are used as the default details, but can be
  // overridden with QueueInputFormatDetails().  A new stream will default back
  // to the FormatDetails used to create the codec unless that stream uses
  // QueueInputFormatDetails().  The QueueInputFormatDetails() is not persistent
  // across streams.
  //
  // The oob_bytes field can be null if the codec type or specific format
  // does not require oob_bytes.
  //
  // This points directly to a field of decoder_params_ (or encoder_params_),
  // which out-last all usages of this pointer.
  const fuchsia::media::FormatDetails* initial_input_format_details_ = nullptr;

  // Held here temporarily until DeviceFidl is ready to handle errors so we can
  // bind.
  fidl::ClientEnd<fuchsia_sysmem2::Allocator> tmp_sysmem_;

  // Held here temporarily until DeviceFidl is ready to handle errors so we can
  // bind.
  fidl::InterfaceRequest<fuchsia::media::StreamProcessor> tmp_interface_request_;

  // This binding doesn't channel-own this CodecImpl.  The DeviceFidl owns all
  // the CodecImpl(s).  The DeviceFidl will SetErrorHandler() such that its
  // ownership drops if the channel fails.  The CodecImpl takes care of cleaning
  // itself up before calling the DeviceFidl's error handler, so that CodecImpl
  // is ready for destruction by the time DeviceFidl's error handler is called.
  fidl::Binding<fuchsia::media::StreamProcessor, CodecImpl*> binding_;

  // This is the zx::channel we get indirectly from binding_.Unbind() (we only
  // need the zx::channel part).  We delay closing the Codec zx::channel until
  // after removing the concurrency tally in ~CodecAdmission, so that a Codec
  // client can try again immediately on noticing channel closure without
  // potentially bouncing off still-existing old CodecAdmission.
  zx::channel codec_to_close_;
  bool was_bind_async_called_ = false;
  // This being true means BindAsync() reached the point where we can and must
  // fail via UnbindLocked() instead of just running the owner's error handler
  // directly.
  bool was_logically_bound_ = false;
  std::unique_ptr<codec_impl::Dispatcher> stream_control_dispatcher_;
  ClosureQueue stream_control_queue_;
  std::queue<fit::closure> stream_control_for_output_queue_;
  std::atomic<bool> stream_control_for_output_queue_pending_;
  fit::closure owner_error_handler_;
  // All stores are under lock_ and seq_cst. Loads under lock_ are relaxed. Loads outside lock_ are
  // seq_cst.
  std::atomic<bool> was_unbind_started_ = false;
  bool is_stream_control_done_ = false;
  bool was_unbind_completed_ = false;
  std::atomic<bool> is_client_error_handler_called_ = false;
  std::condition_variable wake_stream_control_condition_;
  std::condition_variable stream_control_done_condition_;
  std::vector<zx::eventpair> lifetime_tracking_;

  //
  // Codec protocol aspects.
  //

  // Some of the FIDL messages get handled or partly handled on the
  // StreamControl thread.
  void AddInputBuffer_StreamControl(CodecBuffer::Info buffer_info, CodecVmoRange vmo_range);
  void SetInputBufferPartialSettings_StreamControl(
      fuchsia::media::StreamBufferPartialSettings input_partial_settings);
  void FlushEndOfStreamAndCloseStream_StreamControl(uint64_t stream_lifetime_ordinal);
  void CloseCurrentStream_StreamControl(uint64_t stream_lifetime_ordinal,
                                        bool release_input_buffers, bool release_output_buffers);
  void Sync_StreamControl(ThreadSafeDeleter<SyncCallback> callback);
  void QueueInputFormatDetails_StreamControl(uint64_t stream_lifetime_ordinal,
                                             fuchsia::media::FormatDetails format_details);
  void QueueInputPacket_StreamControl(fuchsia::media::Packet packet);
  void QueueInputEndOfStream_StreamControl(uint64_t stream_lifetime_ordinal);
  // This method returns false if input buffers aren't configured enough so far,
  // or if sysmem-based buffers can't be confirmed to be allocated.  On
  // returning false, IsStoppingLocked() will already be true.
  [[nodiscard]] bool CheckWaitEnsureInputConfigured(ScopedLock& lock);

  [[nodiscard]] bool IsStreamActiveLocked();

  void SetInputBufferSettingsCommon(
      ScopedLock& lock, fuchsia::media::StreamBufferPartialSettings* input_partial_settings);

  void SetOutputBufferSettingsCommon(
      ScopedLock& lock, fuchsia::media::StreamBufferPartialSettings* output_partial_settings);

  void SetBufferSettingsCommon(ScopedLock& lock, CodecPort port,
                               fuchsia::media::StreamBufferPartialSettings* partial_settings,
                               const fuchsia::media::StreamBufferConstraints& constraints);

  // is_client_gone true unconditionally cleans up all packets, even if the client has sent
  // EnableOldOutputBuffers
  void EnsureBuffersNotConfigured(ScopedLock& lock, CodecPort port, bool is_client_gone);

  // This is just validating that the _partial_ settings set by the client are
  // valid with respect to the constraints indicated to the client, without any
  // involvement of sysmem yet (but soon), so there's not a ton to validate
  // here.
  [[nodiscard]] bool ValidatePartialBufferSettingsVsConstraintsLocked(
      CodecPort port, const fuchsia::media::StreamBufferPartialSettings& partial_settings,
      const fuchsia::media::StreamBufferConstraints& constraints) __TA_REQUIRES(lock_);

  void AddOutputBufferInternal(CodecBuffer::Info buffer_info, CodecVmoRange vmo_range);

  // Returns true if the port is done configuring (last buffer was added).
  // Returns false if the port is not done configuring or if Fail() was called;
  // currently the caller doesn't need to tell the difference between these two
  // very different cases.
  [[nodiscard]] bool AddNonDynamicBufferCommon(CodecBuffer::Info buffer_info,
                                               CodecVmoRange vmo_range);

  // Return value of false means FailLocked() has already been called.
  [[nodiscard]] bool CheckPlausibleBufferLifetimeOrdinalLocked(CodecPort port,
                                                               uint64_t buffer_lifetime_ordinal)
      __TA_REQUIRES(lock_);

  // Return value of false means FailLocked() has already been called.
  [[nodiscard]] bool CheckStreamLifetimeOrdinalLocked(uint64_t stream_lifetime_ordinal)
      __TA_REQUIRES(lock_);

  // Return value of false means FailLocked() has already been called.
  [[nodiscard]] bool StartNewStream(ScopedLock& lock, uint64_t stream_lifetime_ordinal,
                                    bool is_for_packet) __TA_REQUIRES(lock_);
  void EnsureStreamClosed(ScopedLock& lock) __TA_REQUIRES(lock_);
  void EnsureCoreCodecStreamStopped(ScopedLock& lock);
  void EnsureCodecStreamClosedLockedInternal() __TA_REQUIRES(lock_);

  // Run all items in the sysmem_completion_queue_.  The item itself is run
  // outside the lock.  Returns true if any completions ran.
  [[nodiscard]] bool RunAnySysmemCompletions(ScopedLock& lock) __TA_REQUIRES(lock_);

  // Only sysmem completions get posted this way.  These essentially cut in line
  // before most of the body of all QueueInput...StreamControl methods when
  // those are blocked waiting for sysmem completion.
  void PostSysmemCompletion(fit::closure to_run);
  // Returns false if IsStoppingLocked() is already true - just to save the
  // caller the hassle of checking itself.
  [[nodiscard]] bool WaitEnsureSysmemReadyOnInput(ScopedLock& lock) __TA_REQUIRES(lock_);
  void RunAnySysmemCompletionsOrWait(ScopedLock& lock) __TA_REQUIRES(lock_);

  [[nodiscard]] std::optional<zx::vmo> TryGetMatchExistingVmo(CodecPort port,
                                                              uint64_t buffer_lifetime_ordinal)
      __TA_REQUIRES(lock_);

  void ParticipateInBufferAllocationInternal(
      CodecPort port, uint64_t buffer_constraints_version_ordinal,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
      std::optional<uint64_t> maybe_buffer_lifetime_ordinal, bool allow_single_buffer);

  // Only called when CodecBuffer.parent_vmo_ sees ZX_VMO_ZERO_CHILDREN.
  void DeleteBuffer(CodecBuffer* buffer);

  // The returned constraints will have match_existing_vmo filled out for direct use with
  // BufferCollection.SetConstraints. For GetVmoInfo, the caller can split out match_existing_vmo
  // from the rest of the constraints for separate constraints_to_check vs. vmo_settings_to_check
  // (helps disambiguate GetVmoInfo results to know whether to blame the client in an error path).
  [[nodiscard]] std::optional<fuchsia_sysmem2::BufferCollectionConstraints>
  GetBufferConstraintsForDynamic(ScopedLock& lock, CodecPort port,
                                 uint64_t buffer_constraints_version_ordinal,
                                 bool allow_single_buffer,
                                 uint64_t* out_codec_adapter_constraints_version)
      __TA_REQUIRES(lock_);

  void OnGetVmoInfoCompletion(CodecPort port, std::shared_ptr<AddingBuffer> adding_buffer,
                              uint64_t buffer_constraints_version_ordinal,
                              uint64_t buffer_lifetime_ordinal, uint32_t buffer_index,
                              bool is_match_existing,
                              fidl::Result<fuchsia_sysmem2::Allocator::GetVmoInfo> result);

  void DoCodecBufferDelete(CodecBuffer* buffer);

  // Whether client opted in to OnStreamFailed (else channel closes).
  bool is_on_stream_failed_enabled_ __TA_GUARDED(lock_) = false;
  // Whether client opted in to old output buffers (else old output buffers can still be emitted by
  // the CodecAdapter but will not be emitted to the client). When this is false and the bitstream
  // format permits outputting old buffers, the StreamProcessor will not be entirely compliant with
  // the bitstream spec, but streams that will notice this are rare).
  //
  // Clients testing bitstream spec conformance will want to send EnableOldOutputBuffers to set this
  // to true.
  bool is_enable_old_output_buffers_ __TA_GUARDED(lock_) = false;
  // Whether client opted in to the same output buffer being concurrently
  // referenced by more than one output packet concurrently in flight to/with
  // the client (else not emitted, breaking bitstream compliance for such
  // streams which are rare).
  bool is_enable_same_output_buffer_concurrently_in_flight_ __TA_GUARDED(lock_) = false;

  // This is the stream_lifetime_ordinal of the current stream as viewed from
  // StreamControl ordering domain.  This is the stream lifetime ordinal that
  // gets removed from the head of the Stream queue when StreamControl is done
  // with the stream.
  uint64_t stream_lifetime_ordinal_ = 0;
  // This is the stream_lifetime_ordinal of the most recent stream as viewed
  // from the Output ordering domain (FIDL thread).  This is the stream lifetime
  // ordinal that we add to the tail of the Stream queue.
  uint64_t future_stream_lifetime_ordinal_ = 0;

  // The Output ordering domain (FIDL thread) adds items to the tail of this
  // queue, and the StreamControl ordering domain removes items from the head of
  // this queue.  This queue is how the StreamControl ordering domain knows
  // whether a stream is discarded or not.  If a stream isn't discarded then the
  // StreamControl domain can keep waiting for the client to process
  // OnOutputConstraints() for that stream.  If the stream has been discarded,
  // then StreamControl ordering domain cannot expect the client to ever process
  // OnOutputConstraints() for the stream, and the StreamControl ordering domain
  // can instead move on to the next stream.
  //
  // In addition, this can allow the StreamControl ordering domain to skip past
  // stream-specific items for a stream that's already known to be discarded by
  // the client.
  std::list<std::unique_ptr<Stream>> stream_queue_ __TA_GUARDED(lock_);
  // When no current stream, this is nullptr.  When there is a current stream,
  // this points to that stream, owned by stream_queue_.
  Stream* stream_ = nullptr;

  std::unique_ptr<const fuchsia::media::StreamBufferConstraints> input_constraints_;

  // This represents the most recent settings received from the client and
  // accepted.
  //
  // The settings can be received via SetInputBufferPartialSettings() or
  // SetOutputBufferPartialSettings(). In this case the settings are retained
  // as-received from the client. We discover some of the settings via sysmem
  // and store those in port_settings_.
  //
  // In the case of ParticipateInBufferAllocation and AddBuffer (if supported),
  // this is created by the first AddBuffer with a new (or first)
  // buffer_lifetime_ordinal, and there is no client-sent
  // StreamBufferPartialSettings.
  std::unique_ptr<PortSettings> port_settings_[kPortCount];

  // The most recent fully-configured input or output buffers had this
  // buffer_constraints_version_ordinal.  Even when !port_settings_[port], this
  // is used to detect whether the client has yet caught up to the
  // last_required_buffer_constraints_version_ordinal_[port].
  uint64_t last_provided_buffer_constraints_version_ordinal_[kPortCount] = {};

  // For CodecImpl, the initial StreamOutputConstraints can be the first sent
  // message. If sent that early, the StreamOutputConstraints is likely to
  // change again before any output data is emitted, but it _may not_.
  std::unique_ptr<const fuchsia::media::StreamOutputConstraints> output_constraints_;

  // The core codec indicated that it didn't like an output config that had this
  // buffer_constraints_version_ordinal set.  Normally this would lead to
  // mid-stream output format change, but in case the client starts a new stream
  // before that can happen, we go ahead and force the client to provide a newer
  // config with newer buffer_constraints_version_ordinal before we do format
  // detection for the new stream, just in case the core codec would be annoyed
  // if we ignored it's previous indication.  There's no reason to require every
  // core codec to consider how an incomplete mid-stream format change of an old
  // stream interacts with a new stream, so essentially force the mid-stream
  // format change to complete before start of the new stream (as far as the
  // core codec can tell).  The core codec still has to tolerate stopping the
  // old stream before mid-stream format change is complete, so it's possible
  // we'll eventually decide all core codecs need to just consider an incomplete
  // mid-stream format change to be cancelled by stopping the old stream, in
  // which case we could remove this member var.
  uint64_t codec_adapter_meh_output_buffer_constraints_version_ordinal_ = 0;

  // The server's buffer_lifetime_ordinal, per port.  In contrast to
  // port_settings_[port].buffer_lifetime_ordinal, this value is allowed to be
  // even when the previous odd buffer_lifetime_ordinal is over, due to buffer
  // de-allocation.
  uint64_t buffer_lifetime_ordinal_[kPortCount] = {};

  // This is the latest client-specified buffer_lifetime_ordinal from
  // SetOutputBufferPartialSettings, SetInputBufferPartialSettings, or
  // AddBuffer. This is used for protocol enforcement.
  uint64_t protocol_buffer_lifetime_ordinal_[kPortCount] = {};

  // Allocating these values and sending these values are tracked separately,
  // so that we can more tightly enforce the protocol.  If a client tries to
  // act on a newer ordinal before the server has actually sent it, the server
  // will notice that invalid client behavior and close the channel (instead
  // of just tracking a single number, which would potentially let the client
  // drive the server into the weeds).
  //
  // The next value we'll use for output buffer_constraints_version_ordinal and
  // output format_details_version_ordinal.
  uint64_t next_output_buffer_constraints_version_ordinal_ = 1;
  // For format-only changes that don't require buffer re-allocation, we can
  // just increment the format details ordinal.
  uint64_t next_output_format_details_version_ordinal_ = 1;

  // Separately from ordinal allocation, we track the most recent ordinal that
  // we've actually sent to the client, to allow tighter protocol enforcement in
  // case of a hostile client.
  uint64_t sent_buffer_constraints_version_ordinal_[kPortCount] = {};
  uint64_t sent_format_details_version_ordinal_[kPortCount] = {};

  // When !is_supports_dynamic_buffers_, this stays 0.
  //
  // When is_supports_dynamic_buffers_, this tracks the latest
  // constraints_version from the CodecAdapter that's been noticed, to avoid
  // triggering a mid-stream constraints change for the same constraints_version
  // again.
  uint64_t last_noticed_codec_adapter_output_constraints_version_ = 0;
  // When !is_supports_dynamic_buffers_, this stays 0.
  //
  // The last_sent_codec_adapter_output_constraints_version_ corresponds to
  // sent_buffer_constraints_version_ordinal_. In other words, when sending a
  // new buffer_constraints_version_ordinal and updating
  // sent_buffer_constraints_version_ordinal_, we also note here which
  // constraints_version from the CodecAdapter corresponds. This allows us to
  // check whether a client is behaving badly in GetVmoInfo response handling,
  // vs. the client just needing to catch up.
  uint64_t last_sent_codec_adapter_output_constraints_version_ = 0;

  // The server has sent this version ordinal with
  // buffer_constraints_action_required true.  The server can safely ignore any
  // output configuration that's stale vs. this, because the client will soon
  // catch up to at least this version.  This includes a value for input also,
  // for consistency, but this is mainly for output.
  uint64_t last_required_buffer_constraints_version_ordinal_[kPortCount] = {};

  // This is only relevant when using dynamic buffers.
  //
  // We want ParticipateInBufferAllocation and AddBuffer calls re. the same
  // port, buffer_constraints_version_ordinal, buffer_lifetime_ordinal to use
  // the same BufferCollectionConstraints, even if the CodecAdapter has moved on
  // to newer constraints.
  struct SnappedBufferConstraintsVersionOrdinal {
   public:
    SnappedBufferConstraintsVersionOrdinal(uint64_t buffer_constraints_version_ordinal,
                                           uint64_t constraints_version,
                                           fuchsia_sysmem2::BufferCollectionConstraints constraints)
        : buffer_constraints_version_ordinal_(buffer_constraints_version_ordinal),
          constraints_version_(constraints_version),
          constraints_(std::move(constraints)) {}

    [[nodiscard]] uint64_t buffer_constraints_version_ordinal() const {
      return buffer_constraints_version_ordinal_;
    }
    [[nodiscard]] uint64_t constraints_version() const { return constraints_version_; }
    [[nodiscard]] const fuchsia_sysmem2::BufferCollectionConstraints& constraints() const {
      return constraints_;
    }

    SnappedBufferConstraintsVersionOrdinal(const SnappedBufferConstraintsVersionOrdinal& to_copy) =
        delete;
    SnappedBufferConstraintsVersionOrdinal& operator=(
        const SnappedBufferConstraintsVersionOrdinal& to_copy) = delete;
    SnappedBufferConstraintsVersionOrdinal(SnappedBufferConstraintsVersionOrdinal&& to_move) =
        default;
    SnappedBufferConstraintsVersionOrdinal& operator=(
        SnappedBufferConstraintsVersionOrdinal&& to_move) = default;

   private:
    uint64_t buffer_constraints_version_ordinal_ = 0;
    // The CodecAdapter's constraints_version.
    uint64_t constraints_version_ = 0;
    fuchsia_sysmem2::BufferCollectionConstraints constraints_;
  };
  std::optional<const SnappedBufferConstraintsVersionOrdinal>
      snapped_buffer_constraints_version_ordinal_[kPortCount];

  // This is a queue of lambdas that are to be run on the StreamControl domain
  // before any further QueueInput... processing on StreamControl.  Even before
  // the sysmem completion is on this queue, QueueInput...StreamControl() will
  // be blocked waiting for sysmem completion to be done, and helping run any
  // items that show up on this queue.
  //
  // This line-cutting queue avoids forcing a round-trip to ensure the client
  // isn't sending any input until after the codec knows about the allocated
  // buffers.  This also avoids un-binding the client's channel while we wait
  // for sysmem allocation to be complete - this is worth avoiding because if we
  // unbind then we also don't find out about PEER_CLOSED which would be at
  // least somewhat problematic if the client didn't also cause sysmem
  // allocation to fail.
  //
  // We use wake_stream_control_condition_ to wake any
  // QueueInput...StreamControl waiter that's blocked and helping run items on
  // this queue, since we of course also have to give up on the wait if we're
  // shutting down, which is an aspect in common with other StreamControl waits
  // so it's convenient to share the condition var.
  std::queue<fit::closure> sysmem_completion_queue_;

  // Avoid re-posting to StreamControl to run sysmem_completion_queue_ items if
  // there's already a posted runner lambda that'll notice a newly-added item.
  bool is_sysmem_runner_pending_ = false;

  // When true and output buffers hold images, this forces a single image size
  // for a given buffer, which in turn forces buffer reallocation if the output
  // image size needs to change. Clients should only cause this to be true if
  // they really need to, because this is wastful in terms of extra buffer
  // reallocations when a stream switches dimensions, and also when dimensions
  // switch by using a new stream for the new dimensions. In both ways of
  // switching dimensions, it's typical for the dimensions to keep switching up
  // and down repeatedly, so many buffer reallocations can be avoided by leaving
  // this false (when feasible for the client).
  bool is_force_output_buffers_fixed_image_size_ = false;
  // Initially true; becomes false when we receive the first message that has
  // any field mentioning a buffer_lifetime_ordinal (even an optional field).
  // when false, the EnableForceOutputBuffersFixedImageSize message is not
  // permitted.
  //
  // This field is only ever written or read on the fidl thread, so it doesn't
  // need to be guarded by lock_.
  bool is_force_output_buffers_fixed_image_size_message_permitted_ = true;

  //
  // Adapter-related
  //

  // This is called on Output ordering domain (FIDL thread) any time a message
  // is received which would be able to start a new stream.
  //
  // More complete protocol validation happens on StreamControl ordering domain.
  // The validation here is just to validate to degree needed to not break our
  // stream_queue_ and future_stream_lifetime_ordinal_.
  //
  // Returns true if it worked.  Returns false if FailLocked() has already been
  // called, in which case the caller probably wants to just return.
  [[nodiscard]] bool EnsureFutureStreamSeenLocked(uint64_t stream_lifetime_ordinal)
      __TA_REQUIRES(lock_);

  // This is called on Output ordering domain (FIDL thread) any time a message
  // is received which would close a stream.
  //
  // More complete protocol validation happens on StreamControl ordering domain.
  // The validation here is just to validate to degree needed to not break our
  // stream_queue_ and future_stream_lifetime_ordinal_.
  //
  // Returns true if it worked.  Returns false if FailLocked() has already been
  // called, in which case the caller probably wants to just return.
  [[nodiscard]] bool EnsureFutureStreamCloseSeenLocked(uint64_t stream_lifetime_ordinal)
      __TA_REQUIRES(lock_);

  // This is called on Output ordering domain (FIDL thread) any time a flush is
  // seen.
  //
  // More complete protocol validation happens on StreamControl ordering domain.
  // The validation here is just to validate to degree needed to not break our
  // stream_queue_ and future_stream_lifetime_ordinal_.
  //
  // Returns true if it worked.  Returns false if FailLocked() has already been
  // called, in which case the caller probably wants to just return.
  [[nodiscard]] bool EnsureFutureStreamFlushSeenLocked(uint64_t stream_lifetime_ordinal)
      __TA_REQUIRES(lock_);

  void StartIgnoringClientOldOutputConfig(ScopedLock& lock);

  void TryFillDynamicStreamBufferConstraintsFields(
      CodecPort port, fuchsia::media::StreamBufferConstraints& buffer_constraints)
      __TA_EXCLUDES(lock_);

  void GenerateAndSendNewOutputConstraints(ScopedLock& lock,
                                           std::shared_ptr<PausedOutput> paused_output)
      __TA_REQUIRES(lock_);

  void onCoreCodecMidStreamOutputConstraintsChangeInternal(
      std::optional<uint64_t> constraints_version);
  void EnsureMidStreamOutputConstraintsChange(std::optional<uint64_t> constraints_version,
                                              std::optional<uint64_t> buffer_lifetime_ordinal);

  void ProcessStreamControlForOutputQueue() __TA_EXCLUDES(lock_);
  void MidStreamOutputConstraintsChange(uint64_t stream_lifetime_ordinal);

  [[nodiscard]] bool FixupBufferCollectionConstraintsLocked(
      CodecPort port, fuchsia_sysmem2::BufferCollectionConstraints* buffer_collection_constraints)
      __TA_REQUIRES(lock_);

  void OnBufferCollectionInfo(CodecPort port, uint64_t buffer_lifetime_ordinal, zx_status_t status,
                              fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info);

  // When this method is called we know we're already on the correct thread per
  // the port.
  void OnBufferCollectionInfoInternal(CodecPort port, uint64_t buffer_lifetime_ordinal,
                                      zx_status_t allocate_status,
                                      fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info);

  // Returns a pointer to active_buffers_[port][buffer_lifetime_ordinal_[port]], or nullopt if that
  // doesn't exist yet.
  //
  // This isn't all the active buffers, just all the input or output buffers with
  // buffer_lifetime_ordinal equal to buffer_lifetime_ordinal_[port]. For all the active buffers,
  // see active_buffers_.
  [[nodiscard]] BuffersByIndex* all_buffers(CodecPort port) __TA_REQUIRES(lock_);

  // Returns a reference to active_packets_[port][buffer_lifetime_ordinal_[port]]. The caller must
  // ensure the entry already exists, else this call will ZX_DEBUG_ASSERT().
  //
  // This isn't all the active packets, just all the input or output packets with
  // buffer_lifetime_ordinal equal to buffer_lifetime_ordinal_[port]. For all the active packets,
  // see active_packets_.
  [[nodiscard]] PacketsByIndex& all_packets(CodecPort port);

  // This is the count of buffers under the latest (odd) buffer_lifetime_ordinal that have been
  // added by the client but not yet completely removed. The max is
  // codec_adapter_.GetDynamicBuffersMax(port) (cached in dynamic_buffers_max_[port]).
  [[nodiscard]] uint64_t current_buffer_count(CodecPort port) __TA_REQUIRES(lock_);

  // Returns true if there are zero buffers associated with the port regardless
  // of buffer_lifetime_ordinal.
  [[nodiscard]] bool IsZeroBuffers(CodecPort port) __TA_REQUIRES(lock_) {
    return active_buffers_[port].empty() && adding_buffers_[port].empty();
  }

  // This is set if IsCoreCodecHwBased(), so CodecBuffer::Pin() can get the physical address info,
  // so DMA can be done directly from/to BufferCollection buffers.  We cache this just so we're not
  // constantly calling CoreCodecBti().
  zx::unowned_bti core_codec_bti_;

  // The uint64_t is buffer_lifetime_ordinal. An entry in this map is cleaned up in DeleteBuffer of
  // the last buffer of the buffer_lifetime_ordinal. Sharing these across different
  // buffer_lifetime_ordinal(s) wouldn't really save much.
  //
  // We do it this way to make CodecAdapter implementation for secure buffers as similar as possible
  // to the impl for non-secure buffers. This way, if the CPU attempts to read or write via the fake
  // mapping, we'll get a fault and stack crawl and process termination, which is what we want,
  // since code must not actually access a secure buffer with the CPU. If we instead mapped to the
  // real physical address, the FW and HW would still stop the actual access, but with less-clean
  // error that's potentially async and potentially more difficult to diagnose. This way we get a
  // stack crawl with the instruction pointer at the instruction that attempted the CPU access of a
  // secure buffer.
  //
  // At no point does zircon need to actually populate any covered leaf of the page table backing
  // this virtual address range. This should be limited to VMAR tracking in zircon, not any actual
  // page mappings.
  //
  // We only need one of these per buffer_lifetime_ordinal, since all the buffers of the same
  // buffer_lifetime_ordinal are the same size.
  FakeMapRangesByOrdinal fake_map_range_[kPortCount];

  // Entries here are all the active logical CodecBuffer(s). If a CodecBuffer
  // isn't here, the CodecAdapter doesn't know about the buffer. For example
  // buffers in adding_buffers_ are not known to the CodecAdapter yet and aren't
  // in active_buffers_.
  //
  // The uint64_t is the buffer_lifetime_ordinal of the buffer. The uint32_t is
  // the packet_index of the buffer.
  //
  // By nesting two unordered_map(s), we can efficiently access all the buffers
  // of the current buffer_lifetime_ordinal_[port] without using an ordered map.
  //
  // If the CodecAdapter does not support dynamic buffers, the key values all
  // have buffer_lifetime_ordinal == buffer_lifetime_ordinal_[port], and
  // buffer_index from 0..buffer_count-1.
  //
  // If the CodecAdapter does support dynamic buffers, the key values correspond
  // to any still-active buffers, which can include buffers with older
  // buffer_lifetime_ordinal. The buffer_index values can be any uint32_t value,
  // depending on the values the client set via StreamProcessor.AddBuffer. For
  // the current buffer_lifetime_ordinal, a buffer_index value can only be
  // reused after the previous buffer with the same buffer_index has been fully
  // removed, which includes removing from this map. For older
  // buffer_lifetime_ordinal values, any AddBuffer with an old
  // buffer_lifetime_ordinal is ignored, and further buffer_index value reuse
  // is therefore impossible.
  //
  // Any buffers with buffer_lifetime_ordinal < buffer_lifetime_ordinal_[port]
  // are pending removal. Buffers with buffer_lifetime_ordinal ==
  // buffer_lifetime_ordinal_[port] can be pending removal if
  // is_dynamic_buffers_[port] is set to true, and the client has sent
  // StreamProcessor.RemoveBuffer.
  //
  // The "pending removal" state is only possible when using dynamic buffers, as
  // non-dynamic buffers mode immediately removes buffers, with the
  // corresponding downside of potentially encountering a VP9 stream that uses
  // show_existing_frame on an old-dimensions frame and dropping that output
  // frame. Such a stream is permitted by the VP9 spec, but non-dynamic buffer
  // mode doesn't support outputting any old-dimensions frames of such a stream.
  // It does however support correctly decoding all the frames, if the
  // CodecAdapter correctly retains old-dimensions reference frames beyond the
  // start of a new buffer_lifetime_ordinal at StreamProcessor layer. For such a
  // stream, assuming correct CodecAdapter behavior wrt non-dynamic buffer mode,
  // all frames that are emitted are correctly decoded, but old-dimensions
  // frames are not emitted, despite the bitstream saying they should be. This
  // would tend to look like a stutter/jank fairly near a dimension switch,
  // though the distance from a prior dimension switch isn't technically
  // constrained by the VP9 bitstream spec. In practice such streams seem to be
  // essentially non-existent outside of test streams. For pre-encoded non-RTC
  // streams, a first new-dimensions frame is typically treated as an
  // intra-coded keyframe with no subsequent references to frames prior to the
  // first new-dimensions frame (for better or worse). At least partly, this may
  // be to avoid requiring the various-dimension pre-encoded streams to be
  // jointly encoded, as that would be substantially more complicated. For RTC
  // streams, typically there's no frame reordering in the first place, so no
  // use of show_existing_frame.
  //
  // All that said, we do want to be able to correctly decode test streams that
  // use show_existing_frames in atypical but still bitstream-spec-valid ways,
  // so that's why we keep some old buffer_lifetime_orinal values in this map
  // until the old frames have been fully dropped by the CodecAdapter. Until
  // then, the CodecAdapter can still refer to the old buffers in an output
  // packet, and we'll still emit that output packet via
  // StreamProcessor.OnOutputPacket.
  //
  // Buffers in this map remain alive until either CodecImpl is deleted, or
  // ZX_VMO_ZERO_CHILDREN is seen on CodecBuffer.parent_vmo_. There is no time
  // bound on how long an entry here can remain pending removal/deletion,
  // because some video bitstream formats don't limit how long a reference frame
  // can remain in the set of active reference frames (for example VP9). In
  // addition the codec could be paused in the middle of the stream, blocked on
  // availability of an output buffer or availability of more input data.
  //
  // When StreamProcessor.RemoveBuffer is used, the CodecBuffer here will have a
  // RemoveBuffer completion attached, which will run when removal is done. If
  // is_dynamic_buffers_[port] is set to true, the RemoveBuffer also causes the
  // buffer to become pending removal, even if the buffer has
  // buffer_lifetime_ordinal == buffer_lifetime_ordinal_[port]. If
  // is_dynamic_buffers_[port] is set to false, the RemoveBuffer just attaches
  // the completion so the client is notified later when the buffer completes
  // removal after its buffer_lifetime_ordinal < buffer_lifetime_ordinal_[port].
  //
  // See also all_buffers(port, create_if_absent), which returns a reference to
  // active_buffers_[port][buffer_lifetime_ordinal_[port]].
  BuffersByOrdinal active_buffers_[kPortCount] __TA_GUARDED(lock_);

  // Items correspond to in-progress AddBuffer(s). Entries are removed when the
  // AddBuffer completes its work, or when an attached RemoveBuffer is completed
  // instead. While there's an entry in active_buffers_ or adding_buffers_,
  // another AddBuffer with same buffer_lifetime_ordinal and buffer_index is
  // disallowed. Redundant RemoveBuffer(s) are also disallowed, but not all are
  // detected.
  //
  // We don't need to keep old-ordinal AddingBuffer instances because AddBuffer
  // to an old buffer_lifetime_ordinal is disallowed anyway, so there will only
  // ever be up to one GetVmoInfo in flight per old buffer_lifetime_ordinal.
  //
  // The count of active + adding buffers is capped to
  // DetailedCodecDescription.dynamic_buffers_input_max /
  // dynamic_buffers_output_max.
  AddingBuffersByOrdinal adding_buffers_[kPortCount] __TA_GUARDED(lock_);

  // Used during async shutdown sequence to re-kick the shutdown sequence when
  // we get down to zero buffers in any buffer_lifetime_ordinal, per port.
  //
  // Un-set means we haven't ever set a closure yet (and may never, if there
  // were already zero buffers when async shutdown steps checked). Set with
  // valid closure means we haven't hit zero yet. Set with invalid closure means
  // we hit zero and we've at least started toward calling the closure outside
  // lock_. We do it this way so we don't end up setting another redundant
  // closure sometimes since that would be less consistent run to run.
  std::optional<fit::closure> on_zero_buffers_[kPortCount] __TA_GUARDED(lock_);

  // When not using dynamic buffers, for this bool to be true, there must be
  // enough buffers in all_buffers_ and the CodecAdapter must also be fully
  // configured with regard to those buffers.
  //
  // When using dynamic buffers, this becomes true as soon as the first buffer
  // starts being added, which is also when the new buffer_lifetime_ordinal is
  // started.
  bool is_port_configured_[kPortCount] = {};

  // This owns all the currently-allocated CodecPacket(s) for the
  // StreamProcessor instance. The "active" part of the name just means the
  // CodecPacket(s) are allocated and potentially usable / reusable, analogous
  // to the "active" part of the name of active_buffers_.
  //
  // The uint64_t is the buffer_lifetime_ordinal.
  //
  // The index in the vector here is not the protocol packet_index. The index in
  // the vector is packed from 0 to the high water mark of buffer count under
  // the buffer_lifetime_ordinal, while the protocol packet_index can be
  // arbitrary uint32_t values when *is_dynamic_buffers_.
  //
  // Old buffer_lifetime_ordinal(s) are removed (along with all their
  // CodecPacket instances) only when all the CodecAdapter's handles to all
  // buffers under that buffer_lifetime_ordinal have been closed. The
  // CodecAdapter guarantees it won't subsequently reference any CodecPacket
  // under the buffer_lifetime_ordinal.
  //
  // See also all_packets(port) which returns
  // active_packets_[port][buffer_lifetime_ordinal_[port]].
  PacketsByOrdinal active_packets_[kPortCount];

  // Per-port, per-buffer_lifetime_ordinal, this maps from protocol packet_index
  // to CodecPacket*. These packets have is_free() false.
  //
  // For input this is used to quickly verify that the client isn't putting the
  // same protocol packet_index in flight concurrently.
  //
  // For output this is used to quickly look up the CodecPacket when handling
  // RecycleOutputPacket.
  ProtocolPacketsByOrdinal protocol_packets_by_protocol_packet_index_[kPortCount];

  // When *is_dynamic_buffers_[kInputPort], this is the set of CodecPacket(s)
  // with is_free() true under the current buffer_lifetime_ordinal_[kInputPort].
  //
  // This lets us quickly associate a client's incoming packet_index value with
  // a free input CodecPacket, shortly before marking the CodecPacket not free.
  std::vector<CodecPacket*> free_input_packets_;

  // We start this at 0xFFFFFFF0 to get plenty of coverage of rollover, and to
  // avoid the first protocol packet index being 0, while getting plenty of
  // coverage of 0. This is only used when *is_dynamic_buffers_[kOutputPort].
  uint32_t output_protocol_packet_index_counter_ = 0;

  // un-set means no attempt to add buffers to this port has occurred yet
  //
  // false means non-dynamic buffers, so can't add any dynamic buffers
  //
  // true means dynamic buffers, so can't add non-dynamic buffers
  //
  // Once set to true or false, this std::optional remains set and immutable for
  // the lifetime of the CodecImpl. There is no current requirement to be able
  // to switch a port of a CodecImpl instance between dynamic and non-dynamic
  // buffers. This rule continues to apply for any subsequent
  // buffer_lifetime_ordinal(s) on the port.
  std::optional<bool> is_dynamic_buffers_[kPortCount];

  // Start at 0, and stay 0 if the CodecAdapter doesn't support dynamic buffers.
  uint32_t dynamic_buffers_max_[kPortCount];

  //
  // Util aspects.
  //

  // Send OnFreeInputPacket() using shared_fidl_thread().  This can be called
  // on any thread other than shared_fidl_thread().
  void SendFreeInputPacketLocked(fuchsia::media::PacketHeader header) __TA_REQUIRES(lock_);

  [[nodiscard]] bool IsInputConfiguredLocked() __TA_REQUIRES(lock_);
  [[nodiscard]] bool IsOutputConfiguredLocked() __TA_REQUIRES(lock_);
  [[nodiscard]] bool IsPortConfiguredCommonLocked(CodecPort port) __TA_REQUIRES(lock_);

  // Either completely configured one way or another, or at least partially
  // configured using sysmem-style port settings.  Else the client isn't
  // behaving properly.
  [[nodiscard]] bool IsPortAtLeastPartiallyConfiguredLocked(CodecPort port) __TA_REQUIRES(lock_);

  void vFail(bool is_fatal, const char* format, va_list args) __TA_EXCLUDES(lock_);
  void vFailLocked(bool is_fatal, const char* format, va_list args) __TA_REQUIRES(lock_);

  void PostSerial(async_dispatcher_t* async, fit::closure to_run);
  // If |promise_not_on_previously_posted_fidl_thread_lambda| is true, the
  // caller is promising that it's not running in a lambda that was posted to
  // the fidl thread (running in a FIDL dispatch is fine).
  void PostToSharedFidl(fit::closure to_run);
  // This runs to_run_locked on shared fidl thread, or deletes to_run_locked if CodecImpl is going
  // away. The to_run_locked should run quickly. A return value of true means to_run completed. A
  // return value of false means to_run doesn't run and gets deleted async. The only way for false
  // to be returned is if IsStopping() / IsStoppingLocked() are returning true.
  [[nodiscard]] bool RunSyncOnSharedFidlForStream(ScopedLock& lock, fit::closure to_run)
      __TA_REQUIRES(lock_);
  void PostToStreamControl(fit::closure to_run);
  // This is only used to post MidStreamOutputConstraintsChange. The name is intentionally a warning
  // against using this for anything else. In particular, there can only ever be a max of one
  // not-stale MidStreamOutputConstraintsChange in progress at a time, and it also only happens
  // while there will be no more codec output until resolved. The suitability of this mechanism for
  // posting anything else would need to be carefully evaluated. This uses a queue which will get
  // processed by StreamControl even if StreamControl is currently waiting on output EOS of the
  // current stream.
  void PostToStreamControlForOutput(fit::closure to_run);
  // This is similar to PostToSharedFidl, except output can be paused/buffered until a mid-stream
  // output config change is done (under some conditions). We also queue output RemoveBuffer
  // completion this way. This queueing can be thought of as an extension of the FIDL channel's
  // queue from server to client. Warning: Take care to only queue output format changes, output
  // packets, output EOS, RemoveBuffer completions this way. Queueing other outbound messages can
  // create deadlock. For those, use PostToSharedFidl instead.
  void PostStreamOutputLocked(fit::closure to_run) __TA_REQUIRES(lock_);

  [[nodiscard]] bool IsStoppingLocked();
  [[nodiscard]] bool IsStopping();

  [[nodiscard]] bool IsDecoder() const;
  [[nodiscard]] bool IsEncoder() const;
  [[nodiscard]] bool IsDecryptor() const;

  [[nodiscard]] const fuchsia::mediacodec::CreateDecoder_Params& decoder_params() const;
  [[nodiscard]] const fuchsia::mediacodec::CreateEncoder_Params& encoder_params() const;
  [[nodiscard]] const fuchsia::media::drm::DecryptorParams& decryptor_params() const;

  void LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent event_code) const;

  //
  // Core codec interfacing.
  //

  void HandlePendingInputFormatDetails();

  [[nodiscard]] std::string GetBufferName(CodecPort port);

  // Only tell the core codec to ensure any current stream is stopped if
  // CoreCodecInit() was ever called.
  bool is_core_codec_init_called_ = false;

  bool is_capture_core_codec_ordering_domain_called_ __TA_GUARDED(checker_core_codec_lock_) = false;

  std::atomic<bool> is_core_codec_stream_started_ = false;

  // This generator is "very fast".
  std::ranlux24_base prng_{std::random_device{}()};
  std::uniform_int_distribution<uint32_t> uniform_uint32_;

  //
  // For use by core codec:
  //

  // If the core codec needs to fail the whole CodecImpl, such as when/if new
  // FormatDetails are different than the initial FormatDetails and
  // the core codec doesn't support switching from the old to the new input
  // format details (for example due to needing different input buffer config).
  void onCoreCodecFailCodec(const char* format, ...) override;

  // The core codec should only call this method at times when there is a
  // current stream, not between streams.
  void onCoreCodecFailStream(fuchsia::media::StreamError error) override;

  void onCoreCodecResetStreamAfterCurrentFrame() override;

  // "Mid-stream" can mean at the start of a stream also - it's just required
  // that a stream be active currently.  The core codec must ensure that this
  // call is properly ordered with respect to onCoreCodecOutputPacket() and
  // onCoreCodecOutputEndOfStream() calls.
  //
  // A call to onCoreCodecMidStreamOutputConstraintsChange2 must not be followed
  // by any more output (including EndOfStream) until the associated output
  // re-config is completed by a call to
  // CoreCodecMidStreamOutputBufferReConfigFinish().
  void onCoreCodecMidStreamOutputConstraintsChange2(uint64_t constraints_version) override;
  void onCoreCodecMidStreamOutputConstraintsChange(bool output_re_config_required) override;

  void onCoreCodecOutputFormatChange() override;

  void onCoreCodecInputPacketDone(CodecPacket* packet) override;

  void onCoreCodecOutputPacket(CodecPacket* packet, bool error_detected_before,
                               bool error_detected_during) override;

  void onCoreCodecOutputTimestampHasNoOutput(uint64_t timestamp_ish) override;

  void onCoreCodecOutputEndOfStream(bool error_detected_before) override;

  void onCoreCodecLogEvent(
      media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent event_code) override;

  //
  // Core codec.
  //
  // These are here for TA annotations and to cleanly do a few asserts as we call out to the
  // codec_adapter_, and to make call sites look a bit nicer.
  //

  [[nodiscard]]
  std::optional<media_metrics::StreamProcessorEvents2MigratedMetricDimensionImplementation>
  CoreCodecMetricsImplementation() __TA_EXCLUDES(lock_) override;

  // We allow lock_ to be held during this call, since this just returns a constant value.
  [[nodiscard]] bool IsCoreCodecRequiringOutputConfigForFormatDetection() override;

  // We allow lock_ to be held during this call, since this just returns a constant value.
  [[nodiscard]] bool IsCoreCodecMappedBufferUseful(CodecPort port) override;

  // We allow lock_ to be held during this call, since this just returns a constant value.
  [[nodiscard]] bool IsCoreCodecHwBased(CodecPort port) override;

  [[nodiscard]] zx::unowned_bti CoreCodecBti() __TA_EXCLUDES(lock_) override;

  void CoreCodecInit(const fuchsia::media::FormatDetails& initial_input_format_details)
      __TA_EXCLUDES(lock_) override;

  void CoreCodecSetSecureMemoryMode(CodecPort port,
                                    fuchsia::mediacodec::SecureMemoryMode secure_memory_mode)
      __TA_EXCLUDES(lock_) override;

  void CoreCodecSetForceNewBuffersOnNewDimensions(bool force) override;

  std::optional<CoreCodecGetBufferCollectionConstraints3Result>
  CoreCodecGetBufferCollectionConstraints3(CodecPort port) __TA_EXCLUDES(lock_) override;
  fuchsia_sysmem2::BufferCollectionConstraints CoreCodecGetBufferCollectionConstraints2(
      CodecPort port, const fuchsia::media::StreamBufferConstraints& stream_buffer_constraints,
      const fuchsia::media::StreamBufferPartialSettings& partial_settings)
      __TA_EXCLUDES(lock_) override;

  [[nodiscard]] uint64_t CoreCodecGetConstraintsVersion(CodecPort port)
      __TA_EXCLUDES(lock_) override;

  void CoreCodecSetBufferCollectionInfo(
      CodecPort port, const fuchsia_sysmem2::BufferCollectionInfo& buffer_collection_info)
      __TA_EXCLUDES(lock_) override;

  [[nodiscard]] fuchsia::media::StreamOutputFormat CoreCodecGetOutputFormat(
      uint64_t stream_lifetime_ordinal, uint64_t new_output_format_details_version_ordinal)
      __TA_EXCLUDES(lock_) override;

  void CoreCodecStartStream() __TA_EXCLUDES(lock_) override;

  void CoreCodecQueueInputFormatDetails(
      const fuchsia::media::FormatDetails& per_stream_override_format_details)
      __TA_EXCLUDES(lock_) override;

  void CoreCodecQueueInputPacket(CodecPacket* packet) __TA_EXCLUDES(lock_) override;

  void CoreCodecQueueInputEndOfStream() __TA_EXCLUDES(lock_) override;

  void CoreCodecStopStream() __TA_EXCLUDES(lock_) override;

  void CoreCodecResetStreamAfterCurrentFrame() __TA_EXCLUDES(lock_) override;

  void CoreCodecAddBuffer(CodecPort port, const CodecBuffer* buffer) __TA_EXCLUDES(lock_) override;

  void CoreCodecConfigureBuffers(CodecPort port,
                                 const std::vector<std::unique_ptr<CodecPacket>>& packets)
      __TA_EXCLUDES(lock_) override;

  void CoreCodecRecycleOutputPacket(CodecPacket* packet) __TA_EXCLUDES(lock_) override;

  void CoreCodecEnsureBuffersNotConfigured(CodecPort port) __TA_EXCLUDES(lock_) override;

  void CoreCodecSetStreamControlProfile(zx::unowned_thread stream_control_thread)
      __TA_EXCLUDES(lock_) override;

  void CoreCodecMidStreamOutputBufferReConfigPrepare() __TA_EXCLUDES(lock_) override;

  void CoreCodecMidStreamOutputBufferReConfigFinish() __TA_EXCLUDES(lock_) override;

  void CoreCodecRemoveBuffer(CodecPort port, const CodecBuffer* buffer)
      __TA_EXCLUDES(lock_) override;

  void CoreCodecCloseBufferLifetimeOrdinal(CodecPort port, uint64_t buffer_lifetime_ordinal)
      __TA_EXCLUDES(lock_) override;

  [[nodiscard]] std::string CoreCodecGetSchedulerProfileName(OrderingDomain ordering_domain)
      __TA_EXCLUDES(lock_) override;

  CodecImpl() = delete;
  CodecImpl(CodecImpl& to_copy) = delete;
  CodecImpl(CodecImpl&& to_move) = delete;
  CodecImpl& operator=(CodecImpl& to_assign) = delete;

 protected:
  // Returns true iff the caller is running on the fidl sequence / ordering domain / thread.
  [[nodiscard]] bool IsFidl();
  // Returns true iff the caller is running on the stream control sequence / ordering domain /
  // thread.
  [[nodiscard]] bool IsStreamControl();
  // When is_sharing_fidl_domain_for_core_codec_ or CaptureCoreCodecOrderingDomain has been called:
  //   * Returns true iff the caller is running on the core codec sequence / ordering domain /
  //     thread.
  //
  // When !is_sharing_fidl_domain_for_core_codec_ and CaptureCoreCodecOrderingDomain hasn't been
  // called so far:
  //   * Returns true iff the caller is not running on any other known sequences / ordering domains
  //     / threads.
  [[nodiscard]] bool IsCoreCodec();
};

#endif  // SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_IMPL_H_
