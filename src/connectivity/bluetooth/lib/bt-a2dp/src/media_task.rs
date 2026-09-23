// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_avdtp::{EndpointType, MediaStream};
use dyn_clone::DynClone;
use fidl_fuchsia_bluetooth_bredr::AudioOffloadExtProxy;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_inspect::Node;
use fuchsia_inspect_derive::AttachError;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

use crate::codec::MediaCodecConfig;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MediaTaskStatus {
    AudioDisabled,
    Stopped,
}

#[derive(Debug, Error, Clone)]
#[non_exhaustive]
pub enum MediaTaskError {
    #[error("Operation or configuration not supported")]
    NotSupported,
    #[error("Peer closed the media stream")]
    PeerClosed,
    #[error("Resources needed are already being used")]
    ResourcesInUse,
    #[error("Media stream error: {0}")]
    MediaStream(Arc<std::io::Error>),
    #[error("Codec configuration error: {0}")]
    CodecConfig(Arc<bt_avdtp::Error>),
    #[error("FIDL error: {0}")]
    Fidl(#[from] fidl::Error),
    #[error("Other Media Task Error: {0}")]
    Other(String),
}

impl From<std::io::Error> for MediaTaskError {
    fn from(error: std::io::Error) -> Self {
        Self::MediaStream(Arc::new(error))
    }
}

impl From<bt_avdtp::Error> for MediaTaskError {
    fn from(error: bt_avdtp::Error) -> Self {
        Self::CodecConfig(Arc::new(error))
    }
}

impl From<anyhow::Error> for MediaTaskError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error.to_string())
    }
}

/// MediaTaskRunners are configured with information about the media codec when either peer in a
/// conversation configures a stream endpoint.  When successfully configured, they can start
/// MediaTasks by accepting a MediaStream, which will provide or consume media on that stream until
/// dropped or stopped.
///
/// A builder that will make media task runners from requested configurations.
pub trait MediaTaskBuilder: Send + Sync + DynClone {
    /// Configure a new stream based on the given `codec_config` parameters.
    /// Returns a MediaTaskRunner if the configuration is supported, an
    /// MediaTaskError::NotSupported otherwise.
    fn configure(
        &self,
        peer_id: &PeerId,
        codec_config: &MediaCodecConfig,
    ) -> Result<Box<dyn MediaTaskRunner>, MediaTaskError>;

    /// Return the direction of tasks created by this builder.
    /// Source tasks provide local encoded audio to a peer.
    /// Sink tasks consume encoded audio from a peer.
    fn direction(&self) -> EndpointType;

    /// Provide a set of encoded media configurations that this task can support.
    /// This can vary based on current system capabilities, and should be checked before
    /// communicating capabilities to each peer.
    /// `offload` is a proxy to the offload capabilities of the controller for this peer.
    /// Returns a future that resolves to the set of MediaCodecConfigs that this builder supports,
    /// typically one config per MediaCodecType, or an error if building the configs failed.
    fn supported_configs(
        &self,
        peer_id: &PeerId,
        offload: Option<AudioOffloadExtProxy>,
    ) -> BoxFuture<'static, Result<Vec<MediaCodecConfig>, MediaTaskError>>;
}

dyn_clone::clone_trait_object!(MediaTaskBuilder);

/// MediaTaskRunners represent an ability of the media system to start streaming media.
/// They are configured for a specific codec by `MediaTaskBuilder::configure`
/// Typically a MediaTaskRunner can start multiple streams without needing to be reconfigured,
/// although possibly not simultaneously.
pub trait MediaTaskRunner: Send {
    /// Start a MediaTask using the MediaStream given.
    /// If the task started, returns a MediaTask which will finish if the stream ends or an
    /// error occurs, and can be stopped using `MediaTask::stop` or by dropping the MediaTask.
    /// This can fail with MediaTaskError::ResourcesInUse if a MediaTask cannot be started because
    /// one is already running.
    fn start(
        &mut self,
        stream: MediaStream,
        offload: Option<AudioOffloadExtProxy>,
    ) -> Result<Box<dyn MediaTask>, MediaTaskError>;

    /// Try to reconfigure the MediaTask to accept a new configuration.  This differs from
    /// `MediaTaskBuilder::configure` as it attempts to preserve the same configured session.
    /// The runner remains configured with the initial configuration on an error.
    fn reconfigure(&mut self, _config: &MediaCodecConfig) -> Result<(), MediaTaskError> {
        Err(MediaTaskError::NotSupported)
    }

    /// Set the delay reported from the peer for this media task.
    /// This should configure the media source or sink to attempt to compensate.
    /// Typically this is zero for Sink tasks, but Source tasks can receive this info from the peer.
    /// May only be supported before start.
    /// If an Error is returned, the delay has not been set.
    fn set_delay(&mut self, _delay: Duration) -> Result<(), MediaTaskError> {
        Err(MediaTaskError::NotSupported)
    }

    /// Watch for active channel state changes on the media source.
    /// Resolves to true when active, false when inactive.
    /// Only transitions are reported: the returned future does not resolve if the state has not
    /// changed since it was last reported, and may never resolve.  The end of the media task is
    /// not reported here, `MediaTask::finished` reports that instead.
    /// Default implementation is Ready(true) for tasks that are always active.
    fn watch_active(&mut self) -> BoxFuture<'static, bool> {
        futures::future::ready(true).boxed()
    }

    /// Add information from the running media task to the inspect tree
    /// (i.e. data transferred, jitter, etc)
    fn iattach(&mut self, _parent: &Node, _name: &str) -> Result<(), AttachError> {
        Err("attach not implemented".into())
    }
}

/// MediaTasks represent a media stream being actively processed (sent or received from a peer).
/// They are are created by `MediaTaskRunner::start`.
/// Typically a MediaTask will run a background task that is active until dropped or
/// `MediaTask::stop` is called.
pub trait MediaTask: Send {
    /// Returns a Future that finishes when the running media task finishes for any reason.
    /// Should return a future that immediately resolves if this task is finished.
    fn finished(&mut self) -> BoxFuture<'static, Result<MediaTaskStatus, MediaTaskError>>;

    /// Returns the result if this task has finished, and None otherwise
    fn result(&mut self) -> Option<Result<MediaTaskStatus, MediaTaskError>> {
        self.finished().now_or_never()
    }

    /// Stops the task normally, signalling to all waiters Ok(MediaTaskStatus::Stopped).
    /// Returns the result sent to MediaTask::finished futures, which may be different from Ok(MediaTaskStatus::Stopped).
    /// When this function returns, it is good practice to ensure the MediaStream that started
    /// this task is also dropped.
    fn stop(&mut self) -> Result<MediaTaskStatus, MediaTaskError>;
}

pub mod tests {
    use super::*;

    use fuchsia_sync::Mutex;
    use futures::channel::{mpsc, oneshot};
    use futures::stream::StreamExt;
    use futures::{Future, TryFutureExt};
    use std::fmt;
    use std::sync::Arc;
    use std::task::Poll;

    #[derive(Clone)]
    pub struct TestMediaTask {
        /// The PeerId that was used to make this Task
        pub peer_id: PeerId,
        /// The configuration used to make this task
        pub codec_config: MediaCodecConfig,
        /// If still started, this holds the MediaStream.
        pub stream: Arc<Mutex<Option<MediaStream>>>,
        /// Sender for the shared result future. None if already sent.
        sender: Arc<Mutex<Option<oneshot::Sender<Result<MediaTaskStatus, MediaTaskError>>>>>,
        /// Shared result future.
        result: Shared<BoxFuture<'static, Result<MediaTaskStatus, MediaTaskError>>>,
        /// Delay the task was started with.
        pub delay: Duration,
    }

    impl fmt::Debug for TestMediaTask {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("TestMediaTask")
                .field("peer_id", &self.peer_id)
                .field("codec_config", &self.codec_config)
                .field("result", &self.result.clone().now_or_never())
                .finish()
        }
    }

    impl TestMediaTask {
        pub fn new(
            peer_id: PeerId,
            codec_config: MediaCodecConfig,
            stream: MediaStream,
            delay: Duration,
        ) -> Self {
            let (sender, receiver) = oneshot::channel();
            let result = receiver
                .map_ok_or_else(|_err| Ok(MediaTaskStatus::Stopped), |result| result)
                .boxed()
                .shared();
            Self {
                peer_id,
                codec_config,
                stream: Arc::new(Mutex::new(Some(stream))),
                sender: Arc::new(Mutex::new(Some(sender))),
                result,
                delay,
            }
        }

        /// Return true if the background media task is running.
        pub fn is_started(&self) -> bool {
            // The stream being held represents the task running.
            self.stream.lock().is_some()
        }

        /// End the streaming task without an external stop().
        /// Sends an optional result from the task.
        pub fn end_prematurely(
            &self,
            task_result: Option<Result<MediaTaskStatus, MediaTaskError>>,
        ) {
            let _removed_stream = self.stream.lock().take();
            let mut lock = self.sender.lock();
            let sender = lock.take();
            if let (Some(result), Some(sender)) = (task_result, sender) {
                sender.send(result).expect("send ok");
            }
        }
    }

    impl MediaTask for TestMediaTask {
        fn finished(&mut self) -> BoxFuture<'static, Result<MediaTaskStatus, MediaTaskError>> {
            self.result.clone().boxed()
        }

        fn stop(&mut self) -> Result<MediaTaskStatus, MediaTaskError> {
            let _ = self.stream.lock().take();
            {
                let mut lock = self.sender.lock();
                if let Some(sender) = lock.take() {
                    let _ = sender.send(Ok(MediaTaskStatus::Stopped));
                    return Ok(MediaTaskStatus::Stopped);
                }
            }
            // Result should be available.
            self.finished().now_or_never().unwrap()
        }
    }

    pub struct TestMediaTaskRunner {
        /// The peer_id this was started with.
        pub peer_id: PeerId,
        /// The config that this runner will start tasks for
        pub codec_config: MediaCodecConfig,
        /// If this is reconfigurable
        pub reconfigurable: bool,
        /// If this supports delay reporting
        pub supports_set_delay: bool,
        /// What the delay is right now
        pub set_delay: Option<std::time::Duration>,
        /// The Sender that will send a clone of the started tasks to the builder.
        pub sender: mpsc::Sender<TestMediaTask>,
        /// Receiver for active state changes.
        pub active_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<bool>>>>,
    }

    impl MediaTaskRunner for TestMediaTaskRunner {
        fn start(
            &mut self,
            stream: MediaStream,
            _offload: Option<AudioOffloadExtProxy>,
        ) -> Result<Box<dyn MediaTask>, MediaTaskError> {
            let task = TestMediaTask::new(
                self.peer_id.clone(),
                self.codec_config.clone(),
                stream,
                self.set_delay.unwrap_or(Duration::ZERO),
            );
            // Don't particularly care if the receiver got dropped.
            let _ = self.sender.try_send(task.clone());
            Ok(Box::new(task))
        }

        fn set_delay(&mut self, delay: std::time::Duration) -> Result<(), MediaTaskError> {
            if self.supports_set_delay {
                self.set_delay = Some(delay);
                Ok(())
            } else {
                Err(MediaTaskError::NotSupported)
            }
        }

        fn reconfigure(&mut self, config: &MediaCodecConfig) -> Result<(), MediaTaskError> {
            if self.reconfigurable {
                self.codec_config = config.clone();
                Ok(())
            } else {
                Err(MediaTaskError::NotSupported)
            }
        }

        fn watch_active(&mut self) -> BoxFuture<'static, bool> {
            let Some(receiver) = self.active_receiver.clone() else {
                return futures::future::ready(true).boxed();
            };
            futures::future::poll_fn(move |cx| match receiver.lock().poll_next_unpin(cx) {
                Poll::Ready(Some(val)) => Poll::Ready(val),
                // No one can change the active state anymore, so it will never change again.
                // Real implementations never signal termination here, they just stop changing.
                Poll::Ready(None) => Poll::Pending,
                Poll::Pending => Poll::Pending,
            })
            .boxed()
        }
    }

    /// A TestMediaTask expects to be configured once, and then started and stopped as appropriate.
    /// It will Error if started again while started or stopped while stopped, or if it was
    /// configured multiple times.
    pub struct TestMediaTaskBuilder {
        sender: Mutex<mpsc::Sender<TestMediaTask>>,
        receiver: mpsc::Receiver<TestMediaTask>,
        active_sender: mpsc::UnboundedSender<bool>,
        active_receiver: Arc<Mutex<mpsc::UnboundedReceiver<bool>>>,
        reconfigurable: bool,
        supports_set_delay: bool,
        configs: Vec<MediaCodecConfig>,
        direction: EndpointType,
    }

    impl TestMediaTaskBuilder {
        pub fn new() -> Self {
            let (sender, receiver) = mpsc::channel(5);
            let (active_sender, active_receiver) = mpsc::unbounded();
            let _ = active_sender.unbounded_send(true);
            Self {
                sender: Mutex::new(sender),
                receiver,
                active_sender,
                active_receiver: Arc::new(Mutex::new(active_receiver)),
                reconfigurable: false,
                supports_set_delay: false,
                configs: vec![crate::codec::MediaCodecConfig::min_sbc()],
                direction: EndpointType::Sink,
            }
        }

        pub fn new_inactive() -> Self {
            let (sender, receiver) = mpsc::channel(5);
            let (active_sender, active_receiver) = mpsc::unbounded();
            Self {
                sender: Mutex::new(sender),
                receiver,
                active_sender,
                active_receiver: Arc::new(Mutex::new(active_receiver)),
                reconfigurable: false,
                supports_set_delay: false,
                configs: vec![crate::codec::MediaCodecConfig::min_sbc()],
                direction: EndpointType::Sink,
            }
        }

        pub fn set_active(&self, active: bool) {
            let _ = self.active_sender.unbounded_send(active);
        }

        pub fn with_configs(&mut self, configs: Vec<MediaCodecConfig>) -> &mut Self {
            self.configs = configs;
            self
        }

        pub fn with_direction(&mut self, direction: EndpointType) -> &mut Self {
            self.direction = direction;
            self
        }

        pub fn new_reconfigurable() -> Self {
            Self { reconfigurable: true, ..Self::new() }
        }

        pub fn new_delayable() -> Self {
            Self { supports_set_delay: true, ..Self::new() }
        }

        /// Returns a type that implements MediaTaskBuilder.  When a MediaTask is built using
        /// configure(), it will be available from `next_task`.
        pub fn builder(&self) -> Box<dyn MediaTaskBuilder> {
            Box::new(TestMediaTaskBuilderBuilder {
                sender: self.sender.lock().clone(),
                active_receiver: self.active_receiver.clone(),
                reconfigurable: self.reconfigurable,
                supports_set_delay: self.supports_set_delay,
                configs: self.configs.clone(),
                direction: self.direction,
            })
        }

        /// Gets a future that will return a handle to the next TestMediaTask that gets started
        /// from a Runner that was retrieved from this builder.
        /// The TestMediaTask, can tell you when it's started and give you a handle to the MediaStream.
        pub fn next_task(&mut self) -> impl Future<Output = Option<TestMediaTask>> + '_ {
            self.receiver.next()
        }

        /// Expects that a task had been built, and retrieves that task, or panics.
        #[track_caller]
        pub fn expect_task(&mut self) -> TestMediaTask {
            self.receiver
                .try_next()
                .expect("should have made a task")
                .expect("shouldn't have dropped all senders")
        }
    }

    #[derive(Clone)]
    struct TestMediaTaskBuilderBuilder {
        sender: mpsc::Sender<TestMediaTask>,
        active_receiver: Arc<Mutex<mpsc::UnboundedReceiver<bool>>>,
        reconfigurable: bool,
        supports_set_delay: bool,
        configs: Vec<MediaCodecConfig>,
        direction: EndpointType,
    }

    impl MediaTaskBuilder for TestMediaTaskBuilderBuilder {
        fn configure(
            &self,
            peer_id: &PeerId,
            codec_config: &MediaCodecConfig,
        ) -> Result<Box<dyn MediaTaskRunner>, MediaTaskError> {
            let runner = TestMediaTaskRunner {
                peer_id: peer_id.clone(),
                codec_config: codec_config.clone(),
                sender: self.sender.clone(),
                active_receiver: Some(self.active_receiver.clone()),
                reconfigurable: self.reconfigurable,
                supports_set_delay: self.supports_set_delay,
                set_delay: None,
            };
            Ok::<Box<dyn MediaTaskRunner>, _>(Box::new(runner))
        }

        fn direction(&self) -> EndpointType {
            self.direction
        }

        fn supported_configs(
            &self,
            _peer_id: &PeerId,
            _offload: Option<AudioOffloadExtProxy>,
        ) -> BoxFuture<'static, Result<Vec<MediaCodecConfig>, MediaTaskError>> {
            futures::future::ready(Ok(self.configs.clone())).boxed()
        }
    }
}
