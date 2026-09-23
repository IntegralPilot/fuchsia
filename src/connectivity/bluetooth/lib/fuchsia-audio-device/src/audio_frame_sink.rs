// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_inspect as inspect;
use fuchsia_inspect_derive::{AttachError, Inspect};
use fuchsia_sync::Mutex;
use futures::task::{Context, Poll};
use futures::{FutureExt, io};
use log::warn;
use std::pin::Pin;
use std::sync::Arc;

use crate::frame_vmo;
use crate::stream_config::{SoftStreamConfig, StreamConfigOrTask};
use crate::types::{Error, Result};

/// A sink that accepts audio frames to send as input to Fuchsia audio
/// Usually acquired via SoftStreamConfig::create_input()
pub struct AudioFrameSink {
    /// Handle to the VMO that is receiving the frames.
    frame_vmo: Arc<Mutex<frame_vmo::FrameVmo>>,
    /// The index of the next frame we are writing.
    next_frame_index: usize,
    /// StreamConfig this is attached to, or the SoftStreamConfig::process_requests task
    stream_config: StreamConfigOrTask,
    /// Inspect node
    inspect: inspect::Node,
}

impl AudioFrameSink {
    pub fn new(stream_config: SoftStreamConfig) -> AudioFrameSink {
        AudioFrameSink {
            frame_vmo: stream_config.frame_vmo(),
            next_frame_index: 0,
            stream_config: StreamConfigOrTask::StreamConfig(Box::new(stream_config)),
            inspect: Default::default(),
        }
    }

    /// Start the requests task if not started, and poll the task.
    fn poll_task(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        if let StreamConfigOrTask::Complete = &self.stream_config {
            return Poll::Ready(Err(Error::InvalidState));
        }
        if let StreamConfigOrTask::Task(task) = &mut self.stream_config {
            return task.poll_unpin(cx);
        }
        self.stream_config.start();
        self.poll_task(cx)
    }
}

impl Inspect for &mut AudioFrameSink {
    fn iattach(
        self,
        parent: &fuchsia_inspect::Node,
        name: impl AsRef<str>,
    ) -> core::result::Result<(), AttachError> {
        self.inspect = parent.create_child(name.as_ref());
        if let StreamConfigOrTask::StreamConfig(o) = &mut self.stream_config {
            return o.iattach(&self.inspect, "soft_stream_config");
        }
        Ok(())
    }
}

impl io::AsyncWrite for AudioFrameSink {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::result::Result<usize, io::Error>> {
        if let Poll::Ready(r) = self.poll_task(cx) {
            self.stream_config = StreamConfigOrTask::Complete;
            if let Some(error) = r.err() {
                return Poll::Ready(Err(io::Error::other(error)));
            } else {
                return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
            }
        }
        let result = {
            let mut lock = self.frame_vmo.lock();
            futures::ready!(lock.poll_write(self.next_frame_index, buf, cx))
        };
        match result {
            Ok((latest, missed)) => {
                if missed > 0 {
                    warn!("Couldn't write {missed} frames due to slow writing");
                }
                self.next_frame_index = latest;
                // We always write the whole buffer if it's written
                Poll::Ready(Ok(buf.len()))
            }
            Err(e) => Poll::Ready(Err(io::Error::other(e))),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), io::Error>> {
        // No buffering is done
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), io::Error>> {
        self.stream_config = StreamConfigOrTask::Complete;
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_utils::PollExt;
    use fidl_fuchsia_hardware_audio::*;
    use fidl_fuchsia_media::{AudioChannelId, AudioPcmMode, PcmFormat};
    use fixture::fixture;
    use fuchsia_async as fasync;
    use futures::AsyncWriteExt;

    const TEST_UNIQUE_ID: &[u8; 16] = &[5; 16];
    const TEST_CLOCK_DOMAIN: u32 = 0x00010203;

    pub(crate) fn with_audio_frame_sink<F>(_name: &str, test: F)
    where
        F: FnOnce(fasync::TestExecutor, StreamConfigProxy, AudioFrameSink) -> (),
    {
        let exec = fasync::TestExecutor::new_with_fake_time();
        let format = PcmFormat {
            pcm_mode: AudioPcmMode::Linear,
            bits_per_sample: 16,
            frames_per_second: 44100,
            channel_map: vec![AudioChannelId::Lf, AudioChannelId::Rf],
        };
        let (client, frame_sink) = SoftStreamConfig::create_input(
            TEST_UNIQUE_ID,
            "Google",
            "UnitTest",
            TEST_CLOCK_DOMAIN,
            format,
            zx::MonotonicDuration::from_millis(100),
        )
        .expect("should always build");
        test(exec, client.into_proxy(), frame_sink)
    }

    #[fixture(with_audio_frame_sink)]
    #[fuchsia::test]
    #[rustfmt::skip]
    fn audio_in(mut exec: fasync::TestExecutor, stream_config: StreamConfigProxy, mut frame_sink: AudioFrameSink) {

        // Some test "audio" data.  Silence in signed 16-bit, for 10ms
        let mut send_audio = Vec::new();
        let mut x: u8 = 0x01;
        const BYTES_PER_SECOND: usize = 44100 * 2 * 2;  // 44100 frames, 2 bytes per frame, 2
                                                        // channels per frame.
        send_audio.resize_with(BYTES_PER_SECOND, || {
            x = x.wrapping_add(2);
            x
        });

        let mut next_byte = 0;
        // Sending 10ms packets of the buffer (882 bytes, 441 frames * 2 bytes per frame * 2
        // channels per frame)
        const TEN_MS_BYTES: usize = 441 * 2 * 2;
        let next_buf = &send_audio[next_byte..next_byte + TEN_MS_BYTES];
        let mut write_fut = frame_sink.write(next_buf);
        // Poll the frame stream, which should start the processing of proxy requests.
        exec.run_until_stalled(&mut write_fut).expect_pending("not started yet");

        let result = exec.run_until_stalled(&mut stream_config.get_properties());
        let props1 = match result {
            Poll::Ready(Ok(v)) => v,
            x => panic!("Expected result to be ready ok, got {x:?}"),
        };

        assert_eq!(props1.unique_id.unwrap(),                *TEST_UNIQUE_ID);
        assert_eq!(props1.is_input.unwrap(),                 true);
        assert_eq!(props1.can_mute.unwrap(),                 false);
        assert_eq!(props1.can_agc.unwrap(),                  false);
        assert_eq!(props1.min_gain_db.unwrap(),              0f32);
        assert_eq!(props1.max_gain_db.unwrap(),              0f32);
        assert_eq!(props1.gain_step_db.unwrap(),             0f32);
        assert_eq!(props1.plug_detect_capabilities.unwrap(), PlugDetectCapabilities::Hardwired);
        assert_eq!(props1.manufacturer.unwrap(),             "Google");
        assert_eq!(props1.product.unwrap(),                  "UnitTest");
        assert_eq!(props1.clock_domain.unwrap(),             TEST_CLOCK_DOMAIN);

        let result = exec.run_until_stalled(&mut stream_config.get_supported_formats());
        let formats = match result {
            Poll::Ready(Ok(v)) => v,
            x => panic!("Get supported formats not ready ok: {x:?}"),
        };

        let first = formats.first().to_owned().expect("supported formats to be present");
        let pcm = first.pcm_supported_formats.to_owned().expect("pcm format to be present");
        assert_eq!(pcm.channel_sets.unwrap()[0].attributes.as_ref().unwrap().len(), 2usize);
        assert_eq!(pcm.sample_formats.unwrap()[0],        SampleFormat::PcmSigned);
        assert_eq!(pcm.bytes_per_sample.unwrap()[0],      2u8);
        assert_eq!(pcm.valid_bits_per_sample.unwrap()[0], 16u8);
        assert_eq!(pcm.frame_rates.unwrap()[0],           44100);

        let (ring_buffer, server) = fidl::endpoints::create_proxy::<RingBufferMarker>();

        let format = Format {
            pcm_format: Some(fidl_fuchsia_hardware_audio::PcmFormat {
                number_of_channels:      2u8,
                sample_format:           SampleFormat::PcmSigned,
                bytes_per_sample:        2u8,
                valid_bits_per_sample:   16u8,
                frame_rate:              44100,
            }),
            ..Default::default()
        };

        stream_config.create_ring_buffer(&format, server).expect("ring buffer error");
        let props2 = match exec.run_until_stalled(&mut ring_buffer.get_properties()) {
            Poll::Ready(Ok(v)) => v,
            x => panic!("expected get_properties to be ready ok: {x:?}"),
        };
        assert_eq!(props2.needs_cache_flush_or_invalidate, Some(false));
        assert!(props2.driver_transfer_bytes.unwrap() > 0);

        const TWO_SEC_FRAMES: u32 = 44100 * 2;

        let result = exec.run_until_stalled(&mut ring_buffer.get_vmo(TWO_SEC_FRAMES, 0)); // 2 seconds.
        assert!(result.is_ready());
        let reply = match result {
            Poll::Ready(Ok(Ok(v))) => v,
            _ => panic!("ring buffer get vmo error"),
        };
        let audio_vmo = reply.1;

        // Frames * bytes per sample * channels per sample.
        let bytes_per_two_seconds: usize = BYTES_PER_SECOND * 2;
        assert!(
            bytes_per_two_seconds <= audio_vmo.get_size().expect("should always exist after getbuffer") as usize
        );

        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(42));
        let _ = exec.wake_expired_timers();
        let start_time = exec.run_until_stalled(&mut ring_buffer.start());
        if let Poll::Ready(s) = start_time {
            assert_eq!(s.expect("start time error"), 42);
        } else {
            panic!("start error");
        }

        // Writing audio should succeed now. Fill up the buffer.
        let frame_result = exec.run_until_stalled(&mut write_fut).expect("should be ready");
        // Should have written 882 bytes (441 frames * 2 bytes per frame)
        assert_eq!(frame_result.unwrap(), TEN_MS_BYTES);
        let mut write_fut = loop {
            next_byte = (next_byte + TEN_MS_BYTES) % send_audio.len();
            let next_buf = &send_audio[next_byte..next_byte + TEN_MS_BYTES];
            let mut write_fut = frame_sink.write(next_buf);
            match exec.run_until_stalled(&mut write_fut) {
                Poll::Pending => break write_fut,
                Poll::Ready(Ok(len)) => assert_eq!(TEN_MS_BYTES, len),
                x => panic!("Expected writes to succeed until pending, got {x:?} {next_byte}"),
            };
        };

        // Shouldn't be able to write any more until time goes forward.
        exec.run_until_stalled(&mut write_fut).expect_pending("buffer is full");

        // Run the ring buffer for a bit over half a second.
        exec.set_fake_time(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_millis(500)));
        let _ = exec.wake_expired_timers();

        // Should be able to write again now.
        let result = exec.run_until_stalled(&mut write_fut).expect("buf isn't full");
        match result {
            Ok(len) => assert_eq!(TEN_MS_BYTES, len),
            Err(x) => panic!("Ok from frame write, got {x:?}"),
        };

        let result = exec.run_until_stalled(&mut ring_buffer.stop());
        assert!(result.is_ready());

        // Watch gain only replies once.
        let result = exec.run_until_stalled(&mut stream_config.watch_gain_state());
        assert!(result.is_ready());
        let result = exec.run_until_stalled(&mut stream_config.watch_gain_state());
        assert!(!result.is_ready());

        // Watch plug state only replies once.
        let result = exec.run_until_stalled(&mut stream_config.watch_plug_state());
        assert!(result.is_ready());
        let result = exec.run_until_stalled(&mut stream_config.watch_plug_state());
        assert!(!result.is_ready());
    }
}
