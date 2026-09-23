// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_ADAPTER_H_
#define SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_ADAPTER_H_

#include <fidl/fuchsia.media/cpp/fidl.h>
#include <fidl/fuchsia.media/cpp/hlcpp_conversion.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fuchsia/media/cpp/fidl.h>
#include <fuchsia/mediacodec/cpp/fidl.h>
#include <lib/fidl/cpp/hlcpp_conversion.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zx/thread.h>

#include <list>
#include <random>

#include <fbl/macros.h>

#include "codec_adapter_events.h"
#include "codec_diagnostics.h"
#include "codec_input_item.h"
#include "codec_port.h"

class CodecBuffer;

enum class OrderingDomain : uint32_t {
  StreamControl = 0,
};

// The CodecAdapter abstract base class is used by CodecImpl to interface with a
// particular SW or HW codec.  At the layer of this interface, there's only ever
// up to one active stream to worry about, and Codec FIDL protocol enforcement
// has already been handled above.
//
// For HW-based codecs that need to share the HW, a CodecAdapter represents up
// to one active stream, and does not directly participate in sharing the HW;
// that's further down.
//
// The intent of this interface is to be as narrow an in-process codec interface
// as feasible between FIDL protocol aspects above, and codec-specific details
// below.
//
// For DFv2 drivers, avoid assuming that the thrd_t will be the same for
// separate calls in the same ordering domain, as DFv2 is allowed to switch
// threads for the same ordering domain as long as it ensures ordering of calls.
// This is why we call them "ordering domain(s)" not "threads".
//
// This class is sub-classed to implement a specific codec. The sub-class can be
// parameterized as needed to deal with different formats/modes/etc. When a core
// codec SW lib or FW/HW has a similar interface for decode and encode, it can
// sometimes be reasonable to have a sub-class that's not specific to decode vs
// encode, then further decoder vs encoder sub-classes under that. For HW with
// very different/separate interfaces for decode vs. encode it's more typical to
// sub-class CodecAdapter separately for decode vs. encode. For drivers, some
// aspects common to decode and encode can be in the
// not-StreamProcessor-server-specific portion of the driver, so only
// StreamProcessor-instance-specific stuff would go in the
// common-to-decode-and-encode sub-class of CodecAdapter.
class CodecAdapter {
 public:
  // At least for now, the CodecImpl and CodecAdapter share their main lock.
  //
  // The CodecImpl won't call CodecAdapter methods with the lock_ held, mainly
  // to avoid building up dependencies on the lock sharing, and also to avoid
  // situations where the core codec code would just have to release the lock_
  // in order to acquire video_decoder_lock_ (which is "before" lock_, due to
  // calls from interrupt handlers that already have video_decoder_lock_ held).
  //
  // The CodecAdapter should never call CodecAdapterEvents methods with lock_
  // held.
  CodecAdapter(std::mutex& lock, CodecAdapterEvents* codec_adapter_events);
  virtual ~CodecAdapter();

  // This is called if CodecDiagnostics is available, and not called if not. Codec should not retain
  // ownership of the CodecDiagnostics object and instead call CreateCodec() and then retain
  // ownership of that object.
  virtual void SetCodecDiagnostics(CodecDiagnostics* codec_diagnostics);

  // This will return std::nullopt by default.  A sub-class must implement this method if the
  // sub-class ever calls CodecAdapterEvents.onCoreCodecLogEvent (else metrics won't actually get
  // updated).
  virtual std::optional<media_metrics::StreamProcessorEvents2MigratedMetricDimensionImplementation>
  CoreCodecMetricsImplementation();

  // Core codec.

  // During format detection, a codec may be ok with null output config (false),
  // or may require an output config (true).
  virtual bool IsCoreCodecRequiringOutputConfigForFormatDetection() = 0;

  // If true, the codec can make use of VMOs that are mapped for direct access
  // by the CPU.
  //
  // If true, the CodecImpl will map the buffer VMOs unless buffers are secure
  // memory, and CodecBuffer::base() is usable for direct CPU access.
  //
  // If a codec doesn't support secure memory operation, then buffers won't be
  // secure memory and will be mapped if IsCoreCodecMappedBufferUseful().
  //
  // If buffers are secure, then CodecImpl won't actually map the buffer VMOs,
  // and CodecBuffer::base() isn't usable for direct CPU access.  Instead
  // CodecBuffer::base() will be a vaddr that will fault if accessed as if it
  // were buffer data, and preserves the low-order vmo_usable_start %
  // PAGE_SIZE bits.
  virtual bool IsCoreCodecMappedBufferUseful(CodecPort port) = 0;

  // If true, the codec is HW-based, in the sense that at least some of the
  // processing is performed by specialized processing HW running separately
  // from any CPU execution context.
  virtual bool IsCoreCodecHwBased(CodecPort port) = 0;

  // The return value from this method must match
  // DetailedCodecDescription.supports_dynamic_buffers. This setting must be
  // constant per codec as seen by CodecFactory, per boot.
  virtual bool IsSupportsDynamicBuffers() { return false; }
  // If IsSupportsDynamicBuffers returns true, this must return > 0 for both
  // input and output.
  virtual uint32_t GetDynamicBuffersMax(CodecPort port) { return 0; }

  // Whether the core codec supports mid-stream output constraints changes.
  // CodecAdapter defaults to true. Subclasses such as DecryptorAdapter that do not
  // implement self-pausing and buffer reconfiguration during mid-stream format changes
  // can override this to return false.
  virtual bool IsSupportsMidStreamOutputConstraintsChange() { return true; }

  // Any core codec that performs DMA that will potentially continue beyond the
  // lifetime of the process that holds open the VMO handles being DMA(ed)
  // should override this method to provide CodecImpl with the driver's BTI so
  // VMOs can be properly pinned for DMA.  If a core codec returns true from
  // IsCoreCodecHwBased(), the core codec should also override this method.
  //
  // TODO(https://fxbug.dev/42114424): At least the VP9 decoder isn't overriding this method yet.
  // Also we should enforce that this method be overridden when
  // IsCoreCodecHwBased() true.
  virtual zx::unowned_bti CoreCodecBti() { return zx::unowned_bti(); }

  // The initial input format details and later input format details will
  // _often_ remain the same overall format, and only differ in ways that are
  // reasonable on a format-specific basis.  However, not always.  A core codec
  // should check that any new input format details is still fully compatible
  // with the core codec's initialized configuration (as set up during
  // CoreCodecInit()), and if not, fail the CodecImpl using
  // onCoreCodecFailCodec().  Core codecs may re-configure themselves based
  // on new input FormatDetails to the degree that's reasonable for the
  // input format and the core codec's capabilities, but there's no particular
  // degree to which this is required (for now at least).  Core codecs are
  // discouraged from attempting to reconfigure themselves to process completely
  // different input formats that are better to think of as a completely
  // different Codec.
  //
  // A client that's using different FormatDetails than the initial
  // FormatDetails (to any degree) should try one more time with a fresh
  // Codec before giving up (giving up immediately only if the format details at
  // time of failure match the initial format details specified during Codec
  // creation).
  //
  // The core codec can copy the initial_input_format_details during this call
  // (using fidl::Clone() or similar), but as is the custom with references,
  // should not stash the passed-in reference.
  //
  // TODO(dustingreen): Re-visit the lifetime rule and required copy here, once
  // more is nailed down re. exactly how the core codec relates to CodecImpl.
  virtual void CoreCodecInit(const fuchsia::media::FormatDetails& initial_input_format_details) = 0;

  // The default implementation silently accepts OFF.  On any other value, the
  // default implementation fails the codec.
  //
  // CodecAdapter sub-classes can accept ON if they support ON.  Same for
  // DYNAMIC if/when we add that.  The CodecFactory implementation should
  // already have information on which codecs support ON (or DYNAMIC) for output
  // or input, and CodecImpl will enforce consistency of BufferCollection
  // constraints and BufferCollectionInfo_2 with the SecureMemoryMode specified
  // during codec creation.
  virtual void CoreCodecSetSecureMemoryMode(
      CodecPort port, fuchsia::mediacodec::SecureMemoryMode secure_memory_mode);

  virtual void CoreCodecSetForceNewBuffersOnNewDimensions(bool force) {
    ZX_PANIC("must be overriden when IsSupportsDynamicBuffers() true");
  }

  // All codecs must implement this (or deprecated
  // CoreCodecGetBufferCollectionConstraints2) for both ports. The returned
  // structure will be sent to sysmem in a SetConstraints() call. This method
  // can be called on the FIDL thread or the StreamControl domain (thread for
  // now).
  //
  // For codecs with DetailedCodecDescription.supports_dynamic_buffers true,
  // this call is also used for ParticipateInBufferAllocation. There is
  // intentionally no mechanism for the CodecAdapter sub-class to detect whether
  // this call is driven by non-dynamic buffer allocation or dynamic buffer
  // allocation. The CodecAdapter should always set buffer count fields assuming
  // non-dynamic buffer allocation. For example, min_buffer_count_for_camping
  // should be set assuming non-dynamic buffer allocation. The caller
  // (CodecImpl) will fix up the buffer count fields to ensure that dynamic
  // buffer allocation isn't forced to allocate more buffers than the
  // StreamProcessor client is indicating.
  //
  // Input:
  //
  // For now, a core codec has no way to trigger being asked for new input
  // constraints, so the input constraints (for now) need to be generally
  // applicable to any potential setting/property of the input.
  //
  // A decoder should permit a fairly wide range of buffer space, without
  // worrying whether the min is enough to efficiently handle a high bitrate.
  // The CodecImpl will own bumping up the min based on approximate bitrate
  // provided in the initial decoder creation parameters and/or per-stream input
  // format details (this logic is shared because it can reasonably be shared).
  // A core codec that has special requirements for extra input buffer space
  // given a particular bitrate can take it upon itself to set the input min
  // buffer space, but the idea is that typically it won't be necessary for a
  // decoder to increase the min based on input bitrate beyond what CodecImpl
  // does, so needing to do this should be fairly rare.
  //
  // A video encoder which needs to vary it's input BufferCollectionConstraints
  // based on encoder settings can do so, using the data provided to
  // CoreCodecInit(). If later use of QueueInputFormatDetails() (per-stream)
  // results in an input packet that conforms to the old
  // BufferCollectionConstraints but does not conform to the effective new
  // BufferCollectionConstraints, the core codec can use
  // CodecAdapterEvents::onCoreCodecFailCodec() (the core codec should fail the
  // codec instance in this case rather than attempt to handle input data that
  // is outside the bounds that would have been indicated by the core codec had
  // the current input format details been used as the initial format details).
  // Clients that change the input format details on the fly should be willing
  // to re-request a new codec instance at least once starting with the new
  // input format details via the CodecFactory. This is true for additional
  // reasons beyond this paragraph involving the possibility of accelerated but
  // partial codec implementations. If a client needs to change input format
  // details but doesn't want to concern itself with tracking whether the
  // current codec was created with the current input format details, a client
  // can instead choose to always create a new codec via CodecFactory on any
  // change to the input format details.
  //
  // Output:
  //
  // A CodecAdapter can trigger this method to get called again by indicating an
  // output format detection/change with action_required true via
  // CoreCodecEvents::onCoreCodecMidStreamOutputConstraintsChange().
  //
  // Filling out the usage bits is optional. If the usage bits are not filled
  // out (all still 0), the caller will fill them out based on
  // IsCoreCodecMappedBufferUseful() and IsCoreCodecHwBased(). The core codec
  // must either leave usage set to all 0, or completely fill them out.
  //
  // The CodecAdapter must not set must_match_vmo (at least for now). The intent
  // of this rule is to avoid the CodecAdapter unnecessarily constraining new
  // buffers of a new buffer_lifetime_ordinal to have the same
  // SingleBufferSettings as buffers of a previous buffer_lifetime_ordinal.
  //
  // All buffers of a given buffer_lifetime_ordinal are are guaranteed to have
  // the same sysmem SingleBufferSettings. This is enforced by CodecImpl.
  //
  // For decoder output, if a stream is active, the CodecAdapter needs to make
  // sure that the constraints are set such that the stream can continue
  // decoding correctly given buffers that conform to the returned constraints.
  // This may imply tighter constraints than would be returned if a stream
  // weren't active. There is no requirement that a video decoder be able to
  // handle switching PixelFormatAndModifier mid-stream, but if a CodecAdapter
  // doesn't prevent that using the constraints here, that can happen, and then
  // the CodecAdapter is expected to correctly handle it without dropping any
  // output frames specified by the bitstream.
  //
  // If an uncompressed video port is listing more than one
  // PixelFormatAndModifier, and has at least one linear pixel_format_modifier,
  // the most widely compatible pixel_format and pixel_format_modifier should be
  // at index 0 under image_format_constraints. The index 0 PixelFormat will be
  // sent to the StreamProcessor client in StreamBufferConstraints.pixel_format.
  // See doc comments on that field for more.
  //
  // CodecImpl will try this method before falling back to
  // CoreCodecGetBufferCollectionConstraints2 if !result.has_value().
  //
  // CodecAdapter(s) with IsSupportsDynamicBuffers() true must override this
  // method and CoreCodecGetConstraintsVersion, and must not return
  // std::nullopt.
  struct CoreCodecGetBufferCollectionConstraints3Result {
    fuchsia_sysmem2::BufferCollectionConstraints constraints;
    // Not the same thing as buffer_constraints_version_ordinal in CodecImpl. For output, must
    // change at least once per call to onCoreCodecMidStreamOutputConstraintsChange (and may change
    // more than once, for example if internal constraints change but the existing buffers are still
    // usable despite the internal constraints changing), and must be snapped under the same lock
    // hold interval as the data used to build `constraints`. This is currently mainly for debugging
    // purposes (super cheap to fill out).
    //
    // For input this should always be 0 (at least so far).
    uint64_t constraints_version = 0;
  };
  virtual std::optional<CoreCodecGetBufferCollectionConstraints3Result>
  CoreCodecGetBufferCollectionConstraints3(CodecPort port) {
    return std::nullopt;
  }
  // Deprecated; prefer to override CoreCodecGetBufferCollectionConstraints3
  // instead. Don't override both.
  //
  // The returned constraints should not depend on the stream_buffer_constraints
  // or partial_settings passed into this call. Those parameters should be
  // completely ignored. See CoreCodecGetBufferCollectionConstraints3 for the
  // new revision of this method that removes these params.
  //
  // See comments for CoreCodecGetBufferCollectionConstraints3 for more.
  virtual fuchsia_sysmem2::BufferCollectionConstraints CoreCodecGetBufferCollectionConstraints2(
      CodecPort port, const fuchsia::media::StreamBufferConstraints& stream_buffer_constraints,
      const fuchsia::media::StreamBufferPartialSettings& partial_settings) {
    return {};
  }

  // The return value is in the same sequence as the CoreCodecGetBufferCollectionConstraints3Result
  // constraints_version field.
  //
  // This method must be overridden if CoreCodecGetBufferCollectionConstraints3 is overridden.
  //
  // This method must be overridden if IsSupportsDynamicBuffers() is true.
  //
  // This can get called on any thread, but not on the same stack as an outbound call to CodecImpl.
  virtual uint64_t CoreCodecGetConstraintsVersion(CodecPort port) {
    ZX_PANIC("must be overridden when IsSupportsDynamicBuffers");
  }

  // There are no VMO handles in the buffer_collection_info. Those are instead
  // provided via calls to CoreCodecAddBuffer(), as CodecImpl handles allocation
  // of CodecBuffer instances (each of which has a VMO).
  //
  // The buffer_collection_id is redacted, because when using dynamic buffers,
  // the overall set of buffers for a given port can be from separate buffer
  // collections, so in that case there's not a single buffer_collection_id.
  // Despite this, the BufferCollectionInfo is guaranteed by the caller to be
  // identical for all the current buffers of a given port.
  //
  // This method allows a core codec to know things like buffer_count (when not
  // using dynamic buffers; see below), whether sysmem selected CPU domain or
  // RAM domain for sharing of buffers, whether protected buffers were
  // allocated, etc.
  //
  // When using dynamic buffers, the "buffers" field is un-set. In this case the
  // core codec must rely on CoreCodecAddBuffer calls to know how many current
  // buffers there are for a given port (until
  // CoreCodecEnsureBuffersNotConfigured which resets the count of current
  // buffers to 0 for a given port). In this context, "current buffers" does not
  // count any old buffers still held by the DPB or in an output queue.
  //
  // This call occurs shortly before the first CoreCodecAddBuffer when starting
  // from 0 current buffers. Additional buffers added prior to
  //
  // If the codec supports dynamic buffers, the buffer_collection_info passed to
  // this call will have buffers.size() == 0, and the codec should treat that as
  // normal. Other fields are valid. The codec needs to configure a buffer
  // dynamically when CoreCodecAddBuffer is called, including after processing
  // has begun.
  //
  // A CodecAdapter that supports dynamic buffers and whose output constraints
  // ever change (inter-stream or intra-stream) must tolerate a call to
  // CoreCodecSetBufferCollectionInfo (re. output) with SingleBufferSettings
  // that are not suitable to continue processing the current position of the
  // current stream (whether the stream is actively processing or not). The
  // expected way for the CodecAdapter to handle this is to track its own output
  // constraints version uint64_t which changes atomically along with any
  // updates to the data from which sysmem constraints for output are derived
  // (such as when a decoding video stream changes dimensions). The CodecAdapter
  // should also track whether onCoreCodecMidStreamOutputConstraintsChange has
  // been called yet for the current output constraints version. When the output
  // constraints version changes, this called-yet bool gets reset to false. The
  // CodecAdapter should evaluate whether the current SingleBufferSettings are
  // suitable for continued processing (a) just after the output constraints
  // version has changed and (b) just after the SingleBufferSettings are
  // set/changed by a call to CoreCodecSetBufferCollectionInfo. If the
  // SingleBufferSettings are not suitable and the called-yet bool is false, the
  // CodecAdapter must call onCoreCodecMidStreamOutputConstraintsChange and set
  // the called-yet bool to true. Using this scheme, the CodecAdapter helps
  // achieve the following:
  //   * Processing doesn't pause if the current buffers are fine.
  //   * Unnecessary buffer reallocations are avoided.
  //   * Reallocation of buffers is reliably triggered when new buffers are
  //     needed.
  //   * The client can start the process of replacing buffers at any time as
  //     needed for client-specific reasons.
  //
  // The buffer_collection_info and its SingleBufferSettings are guaranteed to
  // conform to at least one BufferCollectionConstraints previously returned
  // from a previous call to CoreCodecGetBufferCollectionConstraints3/2. In
  // other words, the CodecAdapter only needs to check aspects of the settings
  // here which the CodecAdapter didn't fully constrain already, and doesn't
  // need to worry about ever seeing a completely-arbitrary BufferCollectionInfo
  // here.
  virtual void CoreCodecSetBufferCollectionInfo(
      CodecPort port, const fuchsia_sysmem2::BufferCollectionInfo& buffer_collection_info) = 0;

  // Stream lifetime:
  //
  // The CoreCodecStartStream() and CoreCodecStopStream() calls bracket the
  // lifetime of the current stream.  The CoreCodecQueue.* calls are
  // stream-specific and apply to the current stream.  There is only up to one
  // current stream, and CoreCodecQueue.* calls will only occur when there is a
  // current stream.
  //
  // At least for now, we don't use a separate object instance for the current
  // stream, for the following reasons:
  //   * This interface is the validated and de-async-ed version of the Codec
  //     FIDL interface and the Codec FIDL interface doesn't have a separate
  //     Stream object/channel, so not having a separate stream object here
  //     makes the correspondence closer.
  //   * While the stream is fairly separate, there are also aspects of stream
  //     behavior such as mid-stream output format change which can cause a
  //     stream to essentially re-configure codec-wide output buffers, so the
  //     separate-ness of a stream from the codec isn't complete (regardless of
  //     separate stream object or not).
  //
  // All that said, it certainly can be useful to think of the stream as a
  // logical lifetime of a thing, despite it not being a separate object (at
  // least for now). Some implementations of CodecAdapter may find it
  // convenient to create their own up-to-one-at-a-time-per-CodecAdapter stream
  // object to model the current stream, and that's totally fine.

  // The "Queue" methods will only be called in between CoreCodecStartStream()
  // and CoreCodecStopStream().
  virtual void CoreCodecStartStream() = 0;

  // The parameter includes the oob_bytes. The core codec is free to call
  // onCoreCodecFailCodec() (immediately on this stack or async) if the
  // override input format details can't be accommodated (even in situations
  // where the override input format details would be ok as initial input format
  // details, such as when new input buffer config is needed).
  //
  // That said, the core codec should try to accommodate the change, especially
  // if the client has configured adequate input buffers, and the basic type of
  // the input data hasn't changed.
  //
  // TODO(dustingreen): Nail down the above sorta-vaguely-described rules
  // better.
  //
  // Only permitted between CoreCodecStartStream() and CoreCodecStopStream().
  virtual void CoreCodecQueueInputFormatDetails(
      const fuchsia::media::FormatDetails& per_stream_override_format_details) = 0;

  // Only permitted between CoreCodecStartStream() and CoreCodecStopStream().
  virtual void CoreCodecQueueInputPacket(CodecPacket* packet) = 0;

  // Only permitted between CoreCodecStartStream() and CoreCodecStopStream().
  virtual void CoreCodecQueueInputEndOfStream() = 0;

  // Stop the core codec from processing any more data for the stream that was
  // active and is now stopping.
  virtual void CoreCodecStopStream() = 0;

  // Reset the stream.  Used in processing a watchdog.  If an adapter never generates a watchdog, it
  // doesn't need to override this method.
  //
  // Do not discard any input data or output data beyond what was being worked on at the time the
  // watchdog fired.
  virtual void CoreCodecResetStreamAfterCurrentFrame();

  // Add an input or output buffer.
  //
  // The added buffer is consistent with the most recent prior call to
  // CoreCodecSetBufferCollectionInfo re. the same port (includes SingleBufferSettings). While the
  // VMOs are intentionally not included in that call, child VMOs of those VMOS are indicated via
  // this call. This lets the CodecImpl own allocation of CodecBuffer instances, and lets CodecImpl
  // detect when a CodecAdapter is completely done removing a buffer. For this reason, the
  // CodecAdapter won't be able to use fuchsia.sysmem2/Allocator.GetVmoInfo, as the VMO is not
  // directly supplied by sysmem (child VMOs don't count). The CodecAdapter should have the sysmem
  // info it needs via CodecImpl.
  //
  // A CodecAdapter that doesn't support dynamic buffers may be able to fully configure a buffer
  // during this call and later ignore CoreCodecConfigureBuffers(), or may use
  // CoreCodecConfigureBuffers() to finish configuring buffers.
  //
  // A CodecAdapter that supports dynamic buffers must fully configure a buffer during this call
  // (unless it knows this is an output buffer that it'll never use), and CoreCodecConfigureBuffers
  // won't get called (whether dynamic buffers are being used by the StreamProcessor client or not).
  //
  // A CodecAdapter that doesn't support dynamic buffers can rely on CodecImpl to call
  // CoreCodecEnsureBuffersNotConfigured before adding any buffers with a new BufferCollectionInfo.
  //
  // A CodecAdapter that supports dynamic buffers can rely on CodecImpl to call
  // CoreCodecRemoveBuffer for all not-already-started-removing buffers of the same port before
  // calling CoreCodecSetBufferCollectionInfo and adding a buffer with new SingleBufferSettings via
  // CoreCodecAddBuffer.
  virtual void CoreCodecAddBuffer(CodecPort port, const CodecBuffer* buffer) = 0;

  // When dynamic buffers are supported, this must remove and close handles to an input or output
  // buffer, when safe to do so.
  //
  // The buffer should be removed asap, but without resorting to copying of payload data to other
  // buffers, and without breaking the codec state for the currently active stream (if any).
  // Unnecessary delay removing and closing the buffer may stall the overall pipeline. The removal
  // can proceed async after this call returns.
  //
  // If there is no current stream, or if a stream hasn't seen any input data yet, the buffer should
  // be removed and closed very quickly (async is still allowed).
  //
  // For input, this call will never happen between CoreCodecQueueInputPacket and
  // onCoreCodecInputPacketDone, for any buffer referenced by any such input packet.
  //
  // For output packets, until the CodecAdapter has closed all handles (based on
  // CodecBuffer.GetChildVmo), it remains valid to call onCoreCodecOutputPacket with this
  // CodecBuffer.
  //
  // If there's a current stream that has seen input, and the buffer is still needed to retain
  // stream processing context, or to maintain validity of an item in an output queue, the
  // CodecAdapter _must_ retain at least one handle to the buffer (based on CodecBuffer.GetChildVmo)
  // until the buffer is no longer needed for correct processing of the stream, at which point the
  // retained handle to the buffer (and all mappings, and all pins) should be closed/removed
  // quickly.
  //
  // See also CoreCodecCloseBufferLifetimeOrdinal for a convenient way to manage tracking/cleanup of
  // old packets.
  virtual void CoreCodecRemoveBuffer(CodecPort port, const CodecBuffer* buffer) {
    // This panic means there's a bug. See IsSupportsDynamicBuffers. If that
    // is overridden to return true, this method must also be overridden.
    ZX_PANIC(
        "CoreCodecRemoveBuffer called on a CodecAdapter that doesn't (correctly) support dynamic buffers");
  }

  // Finish setting up input or output buffer(s).
  //
  // Consider doing as much as feasible in CoreCodecAddBuffer() instead, to be
  // _slightly_ nicer to shared_fidl_thread().
  //
  // When a CodecAdapter supports dynamic buffers, this call won't happen, and
  // CoreCodecAddBuffer can happen again later. In this case the CodecAdapter
  // learns about available output packets for the first time via the initial
  // CoreCodecRecycleOutputPacket. For input packets the CodecAdapter learns
  // about an input packet for the first time via CoreCodecQueueInputPacket.
  //
  // Even codecs which don't support dynamic buffers may prefer to ignore the
  // packets parameter and learn of the available packets via
  // CoreCodecRecycleOutputPacket.
  //
  // TODO(dustingreen): Assuming a well-behaved client but time-consuming call
  // to this method, potentially another Codec instance could be disrupted due
  // to sharing the shared_fidl_thread().  If we see an example of that
  // happening, we could switch to not sharing any FIDL threads across Codec
  // instances.
  virtual void CoreCodecConfigureBuffers(
      CodecPort port, const std::vector<std::unique_ptr<CodecPacket>>& packets) = 0;

  // This method can be called at any time while output buffers are configured,
  // including while there's no active stream.
  //
  // This will also be called on each of the output packets shortly after the
  // output packet is created.
  //   * For CodecAdapter(s) that don't support dynamic buffers, this method is
  //     called on each new output packet shortly after
  //     CoreCodecConfigureBuffers() is called. Typically it's cleaner for the
  //     CodecAdapter to ignore the CoreCodecConfigureBuffers packets parameter
  //     and find out about new packets via this call only.
  //   * For CodecAdapter(s) that support dynamic buffers, packets are managed
  //     dynamically by CodecImpl to ensure that there will be at least as many
  //     packets as buffers under each buffer_lifetime_ordinal. New output
  //     packets get added via CoreCodecRecycleOutputPacket before a new output
  //     buffer is added, so a CodecAdapter without show_existing_frame won't
  //     need to pend on availability of an output packet (until
  //     CoreCodecRecycleOutputPacket). The number of output packets will always
  //     be at least as large as the number of buffers, but with
  //     show_existing_frame, the CodecAdapter may still need to wait on
  //     availability of an output packet due to a single buffer potentially
  //     being queued more than once as output using separate packets. So far,
  //     this only comes up for vp9 show_existing_frame.
  //
  // All CodecPacket instances created under a given buffer_lifetime_ordinal
  // remain allocated until all buffers of the buffer_lifetime_ordinal have been
  // dropped by the CodecAdapter. This allows a CodecAdapter to access a
  // CodecPacket instance as long as the CodecAdapter still has any buffers
  // under the same buffer_lifetime_ordinal, regardless of whether the packet is
  // currently free with the CodecAdapter or in-flight with the client. While
  // this can result in quite a few more currently-allocated packets than
  // buffers under an old buffer_lifetime_ordinal, the packets aren't large, and
  // the number of old still-active buffer_lifetime_ordinal values is capped.
  //
  // If DetailedCodecDescription.supports_dynamic_buffers is true, this call can
  // happen for packets of an old buffer_lifetime_ordinal. If false, this call
  // will only happen for packets of the current buffer_lifetime_ordinal.
  //
  // If DetailedCodecDescription.supports_dynamic_buffers is true, the
  // CodecAdapter must tolerate this call when the CodecAdapter has already
  // closed all buffers of the packet's buffer_lifetime_ordinal. This can happen
  // if CodecImpl hasn't yet processed ZX_VMO_ZERO_CHILDREN corresponding to the
  // last closed handle to the last buffer of the old buffer_lifetime_ordinal.
  // For CodecAdapter(s) that need to sometimes output a buffer with an old
  // buffer_lifetime_ordinal, one way to accomplish this is to keep the
  // CodecAdapter's context for the old buffer_lifetime_ordinal until
  // CoreCodecCloseBufferLifetimeOrdinal, even though all handles to the old
  // buffers have been closed. For CodecAdapter(s) that never output a buffer
  // with old buffer_lifetime_ordinal, the CodecAdapter can keep no context for
  // old buffer_lifetime_ordinal(s), and ignore this call if the packet isn't
  // for the current buffer_lifetime_ordinal.
  //
  // Despite this recycling mechanism, it's not permitted for a CodecAdapter to
  // change the contents of any buffer with an old buffer_lifetime_ordinal. Only
  // buffers of the current buffer_lifetime_ordinal can be written to. When a
  // StreamProcessor client has not sent StreamProcessor.EnableOldOutputBuffers,
  // CodecImpl will immediately recycle all the client-owned output packets with
  // old buffer_lifetime_ordinal as soon as the buffer_lifetime_ordinal is
  // out-of-date (shortly after CoreCodecEnsureBuffersNotConfigured or
  // CoreCodecRemoveBuffer has been called applying to all buffers of the
  // out-of-date buffer_lifetime_ordinal). However, the client will likely still
  // have some of the old buffers in its output queue or downstream being
  // rendered etc. This restriction is an additional reason that the
  // CodecAdapter must not select any buffer of an old buffer_lifetime_ordinal
  // as a free buffer to be filled with new output data, even if there are no
  // packets outstanding with that buffer from the CodecAdapter's point of view,
  // and the new output data would fit. The packets recycled here with old
  // buffer_lifetime_ordinal are only for the CodecAdapter to potentially use to
  // output another old output buffer in the output sequence that was filled
  // back when the buffer's buffer_lifetime_ordinal was current.
  virtual void CoreCodecRecycleOutputPacket(CodecPacket* packet) = 0;

  // De-configure input or output buffers. This will never occur at a time when
  // the core codec is expected to be processing data. For input, this can only
  // be called while there's no active stream. For output, this can be called
  // while there's no active stream, or after a stream is started but before any
  // input data is queued, or during processing shortly after the core codec
  // calling onCoreCodecMidStreamOutputConstraintsChange(true), after
  // CoreCodecMidStreamOutputBufferReConfigPrepare() and before
  // CoreCodecMidStreamOutputBufferReConfigFinish().
  //
  // The "ensure" part of the name is because this needs to ensure that buffers
  // will be fully de-configured (and handles dropped) as soon as the buffer is
  // no longer needed in the DPB or in an output queue. When not between
  // CoreCodecMidStreamOutputBufferReConfigPrepare and
  // CoreCodecMidStreamOutputBufferReConfigFinish calls, this means handles to
  // all buffers of the specified port should be dropped quickly and before
  // returning from this call, since there is no active stream or the stream
  // hasn't seen any input data yet, so (using video decoders as an example)
  // there are no output frames in the DPB and no reason to retain any buffers
  // relevant to any output queue (any such output is already known to be safe
  // to drop).
  //
  // The handles to VMOs that the CodecAdapter has are handles to child VMOs,
  // and CodecImpl needs all handles to a child VMO to be closed before
  // StreamProcessor.RemoveBuffer can complete. Keeping a handle open longer
  // than necessary is visible to StreamProcessor clients above, and in some
  // cases could cause the whole pipeline to stall.
  //
  // Any memory mappings to buffers must also be removed, as these keep the
  // child VMO from being dropped at zircon layer.
  //
  // As always, the codec must ensure that zero DMA writes to a buffer will
  // occur during any interval when the buffer is not pinned.
  //
  // This call needs to work regardless of whether buffers are presently fully
  // de-configured already, or if CoreCodecAddBuffer() has been called 1-N times
  // but CoreCodecConfigureBuffers() hasn't been called yet (and won't be, if
  // this method is called instead), or if CoreCodecAddBuffer() has been called
  // N times and CoreCodecConfigureBuffers() has also been called.
  //
  // The "not configured" means pending removal or removed, as appropriate. An
  // output buffer that's still used by bitstream-spec-specified decoder state
  // can still be emitted as output, but as soon as the buffer is no longer used
  // by bitstream-spec-specified decoder state, any handles to the buffer held
  // by the CodecAdapter must be closed. Closing any relevant handles during
  // this call isn't strictly required, but they should be closed asap without
  // damaging any ongoing decoder state.
  //
  // For CodecAdapter(s) with IsSupportsDynamicBuffers() true, this won't get
  // called, and instead CodecImpl will call CoreCodecRemoveBuffer on specific
  // buffers as appropriate.
  //
  // If there's a current stream that has seen input, and a buffer is still
  // needed to retain stream processing context, or to maintain validity of an
  // item in an output queue, the CodecAdapter should retain at least one
  // handle to the buffer (based on CodecBuffer.GetChildVmo) until the buffer is
  // no longer needed, at which point the handle to the buffer should be closed
  // quickly (this is should, not must). If the CodecAdapter does the preceding,
  // the CodecAdapter, as appropriate, should also call onCoreCodecOutputPacket
  // with an old packet and old buffer if output of an old buffer is specified
  // by the bitstream (this is should, not must, and only if the CodecAdapter is
  // retaining a duplicated handle). See also
  // CoreCodecCloseBufferLifetimeOrdinal for a convenient way to manage
  // tracking/cleanup of old packets.
  virtual void CoreCodecEnsureBuffersNotConfigured(CodecPort port) = 0;

  // This call is deprecated, never called, and will be removed. Sub-classes
  // should not override this.
  virtual std::unique_ptr<const fuchsia::media::StreamBufferConstraints>
  CoreCodecBuildNewInputConstraints();

  // This call is deprecated, never called, and will be removed. Sub-classes
  // should not override this.
  virtual std::unique_ptr<const fuchsia::media::StreamOutputConstraints>
  CoreCodecBuildNewOutputConstraints(uint64_t stream_lifetime_ordinal,
                                     uint64_t new_output_buffer_constraints_version_ordinal,
                                     bool buffer_constraints_action_required = true);

  // This will be called on the InputData domain, during the core codec's call
  // to onCoreCodecOutputPacket(), so that the format will be delivered at most
  // once before any packet which needs a new format to be indicated.  The core
  // codec can trigger this to occur during the next onCoreCodecOutputPacket()
  // by calling onCoreCodecOutputFormatChange().  The tracking of pending
  // output format is per-stream, and all streams start with a pending output
  // format, so a core codec need not call onCoreCodecOutputFormatChange()
  // unless the format change is mid-stream (but calling before the first packet
  // is allowed and not harmful).
  virtual fuchsia::media::StreamOutputFormat CoreCodecGetOutputFormat(
      uint64_t stream_lifetime_ordinal, uint64_t new_output_format_details_version_ordinal) = 0;

  // CoreCodecMidStreamOutputBufferReConfigPrepare()
  //
  // For a mid-stream format change where output buffer re-configuration is
  // needed (as initiated async by the core codec calling
  // CodecAdapterEvents::onCoreCodecMidStreamOutputConstraintsChange(true)),
  // this method is called on the StreamControl thread before the client is
  // notified of the need for output buffer re-config (via OnOutputConstraints()
  // with buffer_constraints_action_required true).
  //
  // The CodecAdapter should do whatever is necessary to ensure that output
  // buffers are done de-configuring to the extent that it doesn't prevent
  // correct decoding of the stream, by the time this method returns. If a
  // CodecAdapter keeps old buffer handles/references around (based on
  // CodecBuffer.GetChildVmo) as needed by the bitstream spec, the core codec
  // should drop those handles/references as soon as they're no longer needed
  // for correct processing of the stream.
  //
  // As always, calls to CodecAdapterEvents must not be made while holding
  // lock_.
  virtual void CoreCodecMidStreamOutputBufferReConfigPrepare() = 0;

  // This method is only called if !IsSupportsDynamicBuffers(). If
  // IsSupportsDynamicBuffers(), the CodecAdapter should continue processing as
  // soon as it has at least one new buffer that's suitable.
  //
  // If !IsSupportsDynamicBuffers():
  //
  // This method is called when the mid-stream output buffer re-configuration
  // has completed.  This is called after all the calls to CoreCodecAddBuffer()
  // and the call to CoreCodecConfigureBuffers() are done.
  //
  // The core codec should do whatever is necessary to get back into normal
  // steady-state operation in this method.
  //
  // The core codec must not onCoreCodecOutputPacket() or
  // onCoreCodecOutputEndOfStream() until this method has been called, or until
  // CoreCodecStartStream() is called and some input is available, should the
  // current stream be stopped before completing mid-stream output buffer
  // re-config.  This works partly because the CodecImpl guarantees that if a
  // mid-stream re-config didn't finish, there will be a complete output
  // re-config before the CoreCodecStartStream() - in other words this re-config
  // is abandoned and a new one takes its place and is fully complete prior to
  // the new stream starting.
  //
  // When IsSupportsDynamicBuffers() true, output is buffered as necessary in
  // CodecImpl, so the CodecAdapter doesn't need to worry about emitting output
  // too early.
  virtual void CoreCodecMidStreamOutputBufferReConfigFinish() = 0;

  // Returns a name for the codec that's used for debugging.
  virtual std::string CoreCodecGetName() { return ""; }

  // If desired, the CodecAdapter can set a scheduler profile on the stream control thread.
  // CodecImpl will call this function after the creation of the StreamControl thread to give the
  // CodecAdapter an opportunity to set a scheduler profile on the thread. Ownership of the thread
  // remains with CodecImpl and the handle should be duplicated as necessary (such as for passing to
  // fuchsia.media.ProfileProvider). TODO(https://fxbug.dev/42149456): Generalize this mechanism for
  // all codec_impl threads
  //
  // DFv2 drivers should prefer to implement
  // CoreCodecGetSchedulerProfileName(OrderingDomain::StreamControl). If that returns a non-empty
  // string then this method won't get called. In DFv2 drivers, this won't get called.
  //
  // DFv1 drivers that need a scheduler profile will continue to implement this method.
  virtual void CoreCodecSetStreamControlProfile(zx::unowned_thread stream_control_thread) {}
  // For DFv2 drivers, this is how to set the scheduler role (not CoreCodecSetStreamControlProfile).
  // An empty string will skip attempting to set the scheduler role. CodecAdapter(s) should not
  // attempt to set the scheduler role by grabbing a handle to the current thread during a call on
  // StreamControl ordering domain, because DFv2 is technically allowed to switch threads as long as
  // it ensures sequential calls and handles the scheduler profile correctly.
  //
  // For DFv1 drivers, this won't work and should not return a non-empty string since it'll only
  // result in a warning to the log. See CoreCodecSetStreamControlProfile for the DFv1 way which
  // will only get called if this returns an empty string (such as the default impl).
  virtual std::string CoreCodecGetSchedulerProfileName(OrderingDomain ordering_domain) {
    return {};
  }

  // This will be called only after all handles to buffers of the old buffer_lifetime_ordinal have
  // been closed by the CodecAdapter and CodecImpl has noticed this via ZX_VMO_ZERO_CHILDREN. Until
  // the start of this call, CoreCodecRecycleOutputPacket can still be called with a packet of the
  // old buffer_lifetime_ordinal. After this call, the CodecAdapter should no longer be tracking
  // anything regarding the specified buffer_lifetime_ordinal (not even packets). The CodecAdapter
  // should tolerate this call occurring for a buffer_lifetime_ordinal for which there were never
  // any packets or buffers, or for which the CodecAdapter has already stopped tracking the
  // buffer_lifetime_ordinal unilaterally.
  //
  // CodecAdapter(s) which never output a buffer with old buffer_lifetime_ordinal can use the
  // default implementation (can ignore the call). In this case the CodecAdapter never keeps any
  // context, buffers, or packets for the old buffer_lifetime_ordinal in the first place.
  virtual void CoreCodecCloseBufferLifetimeOrdinal(CodecPort port,
                                                   uint64_t buffer_lifetime_ordinal) {}

 protected:
  // If SetCodecMetrics() was called, this will log an event.  If this method is ever called by a
  // subclass, then CoreCodecMetricsImplementation() must be implemented by the subclass.
  void LogEvent(media_metrics::StreamProcessorEvents2MigratedMetricDimensionEvent event_code);

  // See comment on the constructor re. sharing this lock with the caller of
  // CodecAdapter methods, at least for now.
  std::mutex& lock_;

  // This is how the sub-class informs CodecImpl of async events such as stream or codec failures
  // and output packets. The lock_ must not be held during any of these calls.
  CodecAdapterEvents* events_ = nullptr;

  // For now all the sub-classes queue input here, so may as well be in the base class for now. A
  // sub-class is not required to use this, nor CodecInputItem, but unless a sub-class has a reason
  // to queue differently, using these avoids redundant implementation.
  std::list<CodecInputItem> input_queue_;

  // A core codec will also want to track free output packets, but how best to
  // do that is sub-class-specific.

  // It's generally useful to have a source of random numbers that's compatible with std:: for
  // purposes such as scrambling the order of free packets. These are instance-specific only because
  // of thread-safety considerations, not because of generated sequence considerations.
  std::random_device random_device_;
  std::mt19937 not_for_security_prng_;

 private:
  CodecAdapter() = delete;
  DISALLOW_COPY_ASSIGN_AND_MOVE(CodecAdapter);
};

#endif  // SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_CODEC_ADAPTER_H_
