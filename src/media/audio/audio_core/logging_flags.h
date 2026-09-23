// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_LOGGING_FLAGS_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_LOGGING_FLAGS_H_

#include <lib/zx/time.h>

#include <cstdint>

namespace media::audio {

//
// Render-related logging
//
// Timing and lifetime for AudioRenderers (including timestamps).
inline constexpr bool kLogUltrasoundRendererCtorDtor = false;
inline constexpr bool kLogAudioRendererCtorDtor = false;
inline constexpr bool kLogAudioRendererSetUsage = false;
inline constexpr bool kLogRendererClockConstruction = false;
inline constexpr bool kLogRendererPlayCalls = false;
inline constexpr bool kLogRendererPauseCalls = false;

// Logging related to render discontinuities
//
// In packet queue underflows, we discard data because its start timestamp has already passed.
// The client is not submitting packets fast enough to meet the pipeline demand, so these are
// understood to be UNDERflows (not enough data).
// For each packet queue, we log the first underflow, and subsequent instances depending on
// AudioCore's logging level. If set to INFO, we log less often (at log_level 1: INFO), throttling
// by kPacketQueueUnderflowInfoInterval. If WARNING or higher, we log even less, per
// kPacketQueueUnderflowWarningInterval. By default, NDEBUG logs at WARNING, and DEBUG at INFO.
inline constexpr bool kLogPacketQueueUnderflow = true;
inline constexpr uint16_t kPacketQueueUnderflowWarningInterval = 100;
inline constexpr uint16_t kPacketQueueUnderflowInfoInterval = 10;
// If AudioCore's log level is TRACE or DEBUG, we log all packet queue underflows.
//
// We also log an underflow if its duration exceeds the previously-reported one by a set threshold.
// This intends to more consistently log a long underflow's _first_ packet.
inline constexpr zx::duration kPacketQueueUnderflowDurationIncreaseWarningThreshold = zx::msec(500);
inline constexpr zx::duration kPacketQueueUnderflowDurationIncreaseInfoThreshold = zx::msec(50);
//
// To disable timestamp checks of client-submitted packets, set kLogRendererUnderflow to false.
inline constexpr bool kLogRendererUnderflow = true;
//
// In renderer continuity underflows, we discard data because a NO_TIMESTAMP packet (which is
// understood to be played continuously with the previously-submitted packet) is received too late
// for AudioCore to reliably play every frame in the packet. The client is not submitting data fast
// enough (leading to renderer data loss), so these are considered UNDERflows.
// As with other underflows/overflows, we always log the first one; subsequent instances are
// logged depending on the throttling constants below, based on AudioCore's logging level.
inline constexpr uint16_t kRendererContinuityUnderflowWarningInterval = 100;
inline constexpr uint16_t kRendererContinuityUnderflowInfoInterval = 10;
// If AudioCore's log level is TRACE or DEBUG, we log all continuity underflows.
//
// In renderer timestamp underflows, we discard data because a client-timestamped packet
// intentionally starts before the end of the previous packet. Specifically, the overlapping
// section of the new packet will be dropped. The client is not advancing timestamps fast enough
// (leading to renderer data loss), so these are considered UNDERflows (despite their being
// understood as packet OVERlaps, which can be confusing).
// As with other underflows/overflows, we always log the first one; subsequent instances are
// logged depending on the throttling constants below, based on AudioCore's logging level.
inline constexpr uint16_t kRendererTimestampUnderflowWarningInterval = 100;
inline constexpr uint16_t kRendererTimestampUnderflowInfoInterval = 10;
// If AudioCore's log level is TRACE or DEBUG, we log all timestamp underflows.

//
// Capture-related logging
//
// Lifetime and state for Capturers.
inline constexpr bool kLogUltrasoundCapturerCtorDtor = false;
inline constexpr bool kLogAudioCapturerCtorDtor = false;
inline constexpr bool kLogAudioCapturerSetUsage = false;

// Logging related to capture discontinuities
//
// In a capture overflow, we discard data when no buffer space is available. The client did not
// free empty space fast enough for incoming data: these are OVERflows (too much data).
// As with other underflows/overflows, we always log the first one; subsequent instances are
// logged depending on the throttling constants below, based on AudioCore's logging level.
inline constexpr bool kLogCaptureOverflow = true;
inline constexpr uint64_t kCaptureOverflowWarningInterval = 100'000;  // Log 1/100'000 instances.
inline constexpr uint64_t kCaptureOverflowInfoInterval = 10'000;      // Log 1/10'000 instances.
// If AudioCore's log level is TRACE or DEBUG, we log all capture overflows.

//
// Relevant to both renderers and capturers
//
inline constexpr bool kLogPresentationDelay = false;

//
// Loudness-related logging
//
inline constexpr bool kLogMuteCalls = false;
inline constexpr bool kLogMuteChanges = true;
inline constexpr bool kLogVolumeCalls = true;
inline constexpr bool kLogVolumeChanges = true;
inline constexpr bool kLogCaptureUsageVolumeGainActions = true;
inline constexpr bool kLogRenderUsageVolumeGainActions = true;
inline constexpr bool kLogRendererSetGainMuteRampCalls = false;
inline constexpr bool kLogRendererSetGainMuteRampActions = false;
inline constexpr bool kLogSetDeviceGainMuteActions = true;

//
// Policy-related logging
//
inline constexpr bool kLogPolicyLoader = true;
// Routing-related logging
inline constexpr bool kLogRoutingChanges = false;
// Logging related to idle power-conservation policy/mechanism.
inline constexpr bool kLogIdlePolicyChannelFrequencies = false;
inline constexpr bool kLogIdlePolicyStaticConfigValues = false;
inline constexpr bool kLogIdlePolicyCounts = false;
inline constexpr bool kLogIdleTimers = false;
inline constexpr bool kLogSetActiveChannelsSupport = false;
inline constexpr bool kLogSetActiveChannelsCalls = false;
inline constexpr bool kLogSetActiveChannelsActions = true;
// Logging related to thermal management
inline constexpr bool kLogThermalStateChanges = true;

//
// Mix-related logging
//
inline constexpr bool kLogReconciledTimelineFunctions = false;  // very verbose for ongoing streams
inline constexpr bool kLogInitialPositionSync = false;
inline constexpr bool kLogDestDiscontinuities = true;
inline constexpr int kLogDestDiscontinuitiesStride = 997;  // Prime, to avoid misleading cadences.

// Jam-synchronizations can occur up to 100/sec. We log each MixStage's first occurrence, plus
// subsequent instances depending on our logging level. To disable jam-sync logging for a certain
// log level, set the interval to 0. To disable all jam-sync logging, set kLogJamSyncs to false.
inline constexpr bool kLogJamSyncs = true;
inline constexpr uint16_t kJamSyncWarningInterval = 200;  // Log 1 of every 200 jam-syncs at WARNING
inline constexpr uint16_t kJamSyncInfoInterval = 20;      // Log 1 of every 20 jam-syncs at INFO
// If AudioCore's log level is TRACE or DEBUG, we log all jam-syncs.

// Timing and position advance, in pipeline stages.
#ifdef NDEBUG
// These should be false in production builds.
inline constexpr bool kLogReadLocks = false;
inline constexpr bool kLogTrims = false;
#else
// Keep to true in debug builds so we have verbose logs on FX_CHECK failures in tests.
inline constexpr bool kLogReadLocks = true;
inline constexpr bool kLogTrims = true;
#endif

//
// Effects-related logging
//
inline constexpr bool kLogEffectsV1CtorValues = false;
inline constexpr bool kLogEffectsV2CtorValues = false;
inline constexpr bool kLogEffectsUpdates = false;
inline constexpr bool kLogThermalEffectEnumeration = false;

//
// Device- and driver-related logging
//
inline constexpr bool kLogAudioDevice = false;
inline constexpr bool kLogDevicePlugUnplug = true;
inline constexpr bool kLogAddRemoveDevice = true;

// Values retrieved from the audio driver related to delay, and associated calculations.
inline constexpr bool kLogDriverDelayProperties = false;

// Formats supported by the driver, and the format chosen when creating a RingBuffer.
inline constexpr bool kLogAudioDriverFormats = false;

// Log driver callbacks received (except position notifications: handled separately below).
inline constexpr bool kLogAudioDriverCallbacks = false;
// For non-zero value N, log every Nth position notification. If 0, don't log any.
inline constexpr uint16_t kDriverPositionNotificationDisplayInterval = 0;

//
// Memory-related logging.
//
// Logging related to our periodic memory pinning (to avoid page faults on deadline threads).
// This occurs no sooner than every `kTimeBetweenPins`, as defined in `PinExecutableMemory`.
inline constexpr bool kLogMemoryPins = true;
inline constexpr bool kLogMemoryPinsIfNoChange = false;

}  // namespace media::audio

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_LOGGING_FLAGS_H_
