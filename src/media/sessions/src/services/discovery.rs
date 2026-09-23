// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod filter;
mod watcher;

use self::filter::*;
use self::watcher::*;
use crate::interrupter::*;
use crate::proxies::observer::*;
use crate::proxies::player::*;
use crate::{CHANNEL_BUFFER_SIZE, Result, SessionId};
use async_utils::stream::StreamMap;
use fidl::endpoints::{ClientEnd, ServerEnd};
use fidl_fuchsia_media::UsageReporterProxy;
use fidl_fuchsia_media_sessions2::*;
use futures::StreamExt;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::prelude::*;
use futures::stream::Once;
use log::{info, warn};
use std::collections::{HashMap, HashSet};
use std::ops::RangeFrom;

const LOG_TAG: &str = "discovery";

type Task = Once<BoxFuture<'static, Result<()>>>;

/// Implements `fuchsia.media.session2.Discovery`.
pub struct Discovery {
    player_stream: mpsc::Receiver<Player>,
    catch_up_events: HashMap<SessionId, FilterApplicant<(SessionId, PlayerProxyEvent)>>,
    watcher_ids: RangeFrom<usize>,
    /// Clients watching the stream of player events, through a collection view or
    /// through a `SessionControl` channel.
    watchers: StreamMap<usize, Task>,
    /// Connections to player serving sessions.
    players: StreamMap<SessionId, Player>,
    /// A set of ids of players paused by an interruption on their audio usage.
    interrupt_paused_players: HashSet<SessionId>,
    /// A channel over which out of band interrupted removals can be sent. For
    /// example, if an audio interruption occurs and we pause a player, we add it
    /// to the interrupted set.
    ///
    /// If a user then sends a pause command during the interruption, we must
    /// remove the player from the interrupted set, otherwise we would wake up the
    /// player after the interruption against the user's intent.
    interrupted_removal_sink: mpsc::Sender<SessionId>,
    /// A stream of interrupted set removal commands.
    interrupted_removal_stream: mpsc::Receiver<SessionId>,
    /// A stream of interruptions of audio usages, observed from the audio service.
    interrupter: Interrupter,
    /// The sender through which we distribute player events to all watchers.
    player_update_sender: mpmc::Sender<FilterApplicant<(SessionId, PlayerProxyEvent)>>,
}

impl Discovery {
    pub fn new(
        player_stream: mpsc::Receiver<Player>,
        usage_reporter_proxy: UsageReporterProxy,
    ) -> Self {
        let (interrupted_removal_sink, interrupted_removal_stream) =
            mpsc::channel(CHANNEL_BUFFER_SIZE);
        Self {
            player_stream,
            catch_up_events: HashMap::new(),
            watcher_ids: 0..,
            watchers: StreamMap::empty(),
            players: StreamMap::empty(),
            interrupt_paused_players: HashSet::new(),
            interrupter: Interrupter::new(usage_reporter_proxy),
            interrupted_removal_sink,
            interrupted_removal_stream,
            player_update_sender: mpmc::Sender::default(),
        }
    }

    /// Returns a stream of all `PlayerProxyEvent`s for all media sessions.
    pub fn sessions_info_stream(
        &self,
        filter: Filter,
    ) -> impl Stream<Item = (SessionId, PlayerProxyEvent)> + Unpin + Send + 'static {
        let queue: Vec<FilterApplicant<(SessionId, PlayerProxyEvent)>> =
            self.catch_up_events.values().cloned().collect();
        stream::iter(queue)
            .chain(self.player_update_sender.new_receiver())
            .filter_map(watcher_filter(filter))
    }

    fn single_session_info_stream(
        &self,
        session_id: SessionId,
    ) -> impl Stream<Item = SessionInfoDelta> + Unpin + Send + 'static {
        self.sessions_info_stream(Filter::new(WatchOptions {
            allowed_sessions: Some(vec![session_id]),
            ..Default::default()
        }))
        .filter_map(move |(id, event)| {
            if id != session_id {
                warn!(tag = LOG_TAG; "Watcher did not filter sessions by id");
                future::ready(None)
            } else {
                match event {
                    PlayerProxyEvent::Updated(update) => future::ready(Some(update)),
                    _ => future::ready(None),
                }
            }
        })
        .map(|delta| delta())
    }

    /// Connects a client to a single session if that session exists.
    async fn connect_to_session(
        &mut self,
        session_id: SessionId,
        session_control_request: ServerEnd<SessionControlMarker>,
    ) {
        let mut requests = session_control_request.into_stream();

        let session_info_stream = self.single_session_info_stream(session_id);

        let (mut control_request_sink, control_request_stream) = mpsc::channel(CHANNEL_BUFFER_SIZE);
        let (mut status_request_sink, status_request_stream) = mpsc::channel(CHANNEL_BUFFER_SIZE);

        let mut interrupted_removal_sink = self.interrupted_removal_sink.clone();
        let partitioner = move || async move {
            while let Some(request) = requests.try_next().await? {
                match ForwardControlRequest::try_from(request) {
                    Ok(control_request) => {
                        if matches!(control_request, ForwardControlRequest::Pause) {
                            interrupted_removal_sink.send(session_id).await?;
                        }
                        control_request_sink.send(control_request).await?
                    }
                    Err(status_request) => status_request_sink.send(status_request).await?,
                }
            }

            Result::Ok(())
        };
        let partitioner = partitioner();
        let observer = Observer::new(session_info_stream, status_request_stream);
        let task = future::join(partitioner, observer).map(drop).boxed();

        if let Some(player) = self.players.inner_mut().get_mut(&session_id) {
            player.serve_controls(control_request_stream);
            player.add_proxy_task(task);
        }
    }

    /// Connects a sink to the stream of `PlayerProxyEvents` for the set of all sessions.
    async fn connect_session_watcher(
        &mut self,
        watch_options: WatchOptions,
        sink: impl Sink<(SessionId, PlayerProxyEvent), Error = anyhow::Error> + Send + 'static,
    ) {
        let session_info_stream = self.sessions_info_stream(Filter::new(watch_options));
        let event_forward = session_info_stream.map(Ok).forward(sink).boxed();

        let id = self.watcher_ids.next().expect("Taking next element from infinite sequence");
        self.watchers.insert(id, stream::once(event_forward));
    }

    /// Connects a client to the set of all sessions.
    async fn handle_watch_sessions(
        &mut self,
        watch_options: WatchOptions,
        session_watcher: ClientEnd<SessionsWatcherMarker>,
    ) {
        let proxy = session_watcher.into_proxy();

        let sink = FlowControlledProxySink::from(proxy);
        self.connect_session_watcher(watch_options, sink).await;
    }

    async fn handle_player_update(
        &mut self,
        player_update: FilterApplicant<(SessionId, PlayerProxyEvent)>,
    ) {
        let (id, event) = &player_update.applicant;
        if let PlayerProxyEvent::Removed = event {
            self.catch_up_events.remove(id);
            self.interrupt_paused_players.remove(id);
        } else {
            let mut usage = None;
            if let Some(player) = self.players.inner_mut().get_mut(id) {
                usage = player.usage_to_pause_on_interruption();
            }

            if let Some(usage) = usage {
                if let Err(e) = self.interrupter.watch_usage(usage).await {
                    warn!(
                        tag = LOG_TAG;
                        concat!(
                            "Audio policy service UsageReporter is unavailable; ",
                            "interruptions will not work. Error: {:?}"
                        ),
                        e
                    )
                }
            }

            self.catch_up_events.insert(*id, player_update.clone());
        }
        self.player_update_sender.send_or_disconnect(player_update).await;
    }

    async fn handle_interruption(&mut self, Interruption { usage, stage }: Interruption) {
        let mut interrupted = HashSet::new();
        std::mem::swap(&mut interrupted, &mut self.interrupt_paused_players);

        if let InterruptionStage::Begin = stage {
            info!(
                tag = LOG_TAG;
                concat!(
                    "Usage {:?} was interrupted; will pause the players ",
                    "that requested to be paused on interruption.",
                ),
                usage
            );
        }

        // We can ignore errors sending our commands to the player, because they will
        // drop from the StreamMap when disconnected on their own.
        match stage {
            InterruptionStage::Begin => {
                for (id, player) in self.players.inner_mut().iter() {
                    if player.usage_to_pause_on_interruption() == Some(usage) && player.is_active()
                    {
                        let pause_error = player.pause();
                        if pause_error.is_err() {
                            info!(tag = LOG_TAG; "Received pause error {:?}", pause_error.err());
                        }
                        interrupted.insert(*id);
                    }
                }
            }
            InterruptionStage::End => {
                for (id, player) in self.players.inner_mut().iter() {
                    if interrupted.remove(&id)
                        && player.usage_to_pause_on_interruption() == Some(usage)
                    {
                        let play_error = player.play();
                        if play_error.is_err() {
                            info!(tag = LOG_TAG; "Received play error {:?}", play_error.err());
                        }
                    }
                }
            }
        }

        std::mem::swap(&mut interrupted, &mut self.interrupt_paused_players);
    }

    async fn observe_session(
        &mut self,
        session_id: SessionId,
        session_request: ServerEnd<SessionObserverMarker>,
    ) {
        let status_request_stream = session_request.into_stream();

        let status_request_stream = status_request_stream
            .map(std::result::Result::ok)
            .take_while(|r| future::ready(r.is_some()))
            .filter_map(|r| future::ready(r.and_then(SessionObserverRequest::into_watch_status)));
        let session_info_stream = self.single_session_info_stream(session_id);

        let observer = Observer::new(session_info_stream, status_request_stream);

        if let Some(player) = self.players.inner_mut().get_mut(&session_id) {
            player.add_proxy_task(observer.map(drop).boxed());
        }
    }

    pub async fn serve(
        mut self,
        mut discovery_request_stream: mpsc::Receiver<DiscoveryRequest>,
        mut observer_request_stream: mpsc::Receiver<ObserverDiscoveryRequest>,
    ) -> Result<()> {
        // Loop forever. All input channels live the life of the service, so we will always have a
        // stream to poll.
        loop {
            futures::select! {
                // A request has come in from any of the potentially many clients connected to the
                // discovery service.
                request = discovery_request_stream.select_next_some() => {
                    match request {
                        DiscoveryRequest::ConnectToSession {
                            session_id, session_control_request, ..
                        } => {
                            self.connect_to_session(session_id, session_control_request).await;
                        }
                        DiscoveryRequest::WatchSessions { watch_options, session_watcher, ..} => {
                            self.handle_watch_sessions(watch_options, session_watcher).await;
                        }
                    }
                }
                // A request from any of the potentially many clients connected
                // to `ObserverDiscovery`.
                request = observer_request_stream.select_next_some() => {
                    match request {
                        ObserverDiscoveryRequest::ConnectToSession {
                            session_id, session_request, ..
                        } => {
                            self.observe_session(session_id, session_request).await;
                        },
                        ObserverDiscoveryRequest::WatchSessions { watch_options, sessions_watcher, .. } => {
                            self.handle_watch_sessions(watch_options, sessions_watcher).await;
                        }
                    }
                }
                // Watch the audio service for interruption notifications.
                interruption = self.interrupter.select_next_some() => {
                    self.handle_interruption(interruption).await;
                }
                removal = self.interrupted_removal_stream.select_next_some() => {
                    self.interrupt_paused_players.remove(&removal);
                }
                // Drive dispatch of events to watcher clients.
                _ = self.watchers.next() => {
                },
                // A new player has been published to `fuchsia.media.sessions2.Publisher`.
                mut new_player = self.player_stream.select_next_some() => {
                    if let Err(e) = new_player.notify_published() {
                        warn!(tag = LOG_TAG; "Notify failed {:?}", e);
                    }
                    self.players.insert(new_player.id(), new_player);
                }
                // A player answered a hanging get for its status.
                player_update = self.players.next() => {
                    if let Some(player_update) = player_update {
                        self.handle_player_update(player_update).await;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::id::Id;
    use crate::proxies::player::ValidPlayerInfoDelta;
    use crate::spawn_log_error;
    use assert_matches::assert_matches;
    use fidl::endpoints::{create_endpoints, create_proxy};
    use fidl_fuchsia_media::UsageReporterMarker;
    use fidl_fuchsia_media_sessions2::InterruptionBehavior;
    use fuchsia_inspect::Inspector;
    use futures::channel::oneshot;

    #[fuchsia::test]
    async fn watchers_caught_up_to_existing_players() -> Result<()> {
        let (mut player_sink, player_stream) = mpsc::channel(100);
        let (mut discovery_request_sink, discovery_request_stream) = mpsc::channel(100);
        let (_observer_request_sink, observer_request_stream) = mpsc::channel(100);
        let (player_published_sink, _player_published_receiver) = oneshot::channel();
        let dummy_control_handle =
            create_endpoints::<DiscoveryMarker>().1.into_stream_and_control_handle().1;

        let (usage_reporter_proxy, _server_end) = create_proxy::<UsageReporterMarker>();
        let under_test = Discovery::new(player_stream, usage_reporter_proxy);
        spawn_log_error(under_test.serve(discovery_request_stream, observer_request_stream));

        // Create one watcher ahead of any players, for synchronization.
        let (watcher1_client, watcher1_server) = create_endpoints::<SessionsWatcherMarker>();
        let mut watcher1 = watcher1_server.into_stream();
        discovery_request_sink
            .send(DiscoveryRequest::WatchSessions {
                watch_options: Default::default(),
                session_watcher: watcher1_client,
                control_handle: dummy_control_handle.clone(),
            })
            .await?;

        // Add a player to the set, and vend an update from it.
        let inspector = Inspector::default();
        let (player_client, player_server) = create_endpoints::<PlayerMarker>();
        let player = Player::new(
            Id::new()?,
            player_client,
            PlayerRegistration {
                domain: Some(String::from("test_domain://")),
                ..Default::default()
            },
            inspector.root().create_child("test_player"),
            player_published_sink,
        )?;
        player_sink.send(player).await?;
        let mut player_requests = player_server.into_stream();
        let info_change_responder = player_requests
            .try_next()
            .await?
            .expect("Receiving a request")
            .into_watch_info_change()
            .expect("Receiving info change responder");
        info_change_responder.send(&Default::default())?;

        // Synchronize with the first watcher. After receiving this, we know that the service knows
        // about the player.
        assert_matches!(watcher1.try_next().await?, Some(_));

        // A new watcher connecting after the registration of the player should be caught up
        // with the existence of the player.
        let (watcher2_client, watcher2_server) = create_endpoints::<SessionsWatcherMarker>();
        discovery_request_sink
            .send(DiscoveryRequest::WatchSessions {
                watch_options: Default::default(),
                session_watcher: watcher2_client,
                control_handle: dummy_control_handle.clone(),
            })
            .await?;
        let mut watcher2 = watcher2_server.into_stream();
        assert_matches!(watcher2.try_next().await?.and_then(|r| r.into_session_updated()), Some(_));

        Ok(())
    }

    #[fuchsia::test]
    async fn interruption_is_gracefully_disabled_when_usage_reporter_is_unavailable() -> Result<()>
    {
        let (mut player_sink, player_stream) = mpsc::channel(100);
        let (_discovery_request_sink, discovery_request_stream) = mpsc::channel(100);
        let (_observer_request_sink, observer_request_stream) = mpsc::channel(100);
        let (player_published_sink, _player_published_receiver) = oneshot::channel();

        let (usage_reporter_proxy, server_end) = create_proxy::<UsageReporterMarker>();
        drop(server_end);
        let under_test = Discovery::new(player_stream, usage_reporter_proxy);
        spawn_log_error(under_test.serve(discovery_request_stream, observer_request_stream));

        let (player_client, _player_server) = create_endpoints::<PlayerMarker>();
        let inspector = Inspector::default();
        let mut player = Player::new(
            Id::new()?,
            player_client,
            PlayerRegistration {
                domain: Some(String::from("test_domain://")),
                ..Default::default()
            },
            inspector.root().create_child("test_player"),
            player_published_sink,
        )?;
        player.update(ValidPlayerInfoDelta {
            interruption_behavior: Some(InterruptionBehavior::Pause),
            ..Default::default()
        });
        player_sink.send(player).await?;

        // The test passes by not panicking.
        Ok(())
    }

    // Ensure that slow or unresponsive session watchers do not block the Discovery service.
    //
    // If a FIDL client connects via `WatchSessions` then stops consuming events, its internal
    // broadcast channel buffer fills up. If the publisher awaited space in this lagging client's
    // queue, all other clients would be blocked.
    //
    // Instead `Discovery::handle_player_update` broadcasts events via `send_or_disconnect`. When a
    // client watcher's buffer is full, `send_or_disconnect` drops and disconnects that receiver
    // rather waiting for it to have available space.
    //
    // This test connects a slow watcher (whose channel is never read) and a responsive fast
    // watcher. It broadcasts 150 player updates (exceeding the channel buffer capacity of 100)
    // and verifies that the slow watcher is dropped while the fast watcher continues receiving
    // all events without any stalling or hanging in the Discovery service.
    #[fuchsia::test]
    async fn slow_watcher_does_not_block_discovery() -> Result<()> {
        let (mut player_sink, player_stream) = mpsc::channel(100);
        let (mut discovery_request_sink, discovery_request_stream) = mpsc::channel(100);
        let (_observer_request_sink, observer_request_stream) = mpsc::channel(100);
        let (player_published_sink, _player_published_receiver) = oneshot::channel();
        let dummy_control_handle =
            create_endpoints::<DiscoveryMarker>().1.into_stream_and_control_handle().1;

        let (usage_reporter_proxy, _server_end) = create_proxy::<UsageReporterMarker>();
        let under_test = Discovery::new(player_stream, usage_reporter_proxy);
        spawn_log_error(under_test.serve(discovery_request_stream, observer_request_stream));

        // Connect slow watcher (we never read from its server endpoint).
        let (slow_watcher_client, _slow_watcher_server) =
            create_endpoints::<SessionsWatcherMarker>();
        discovery_request_sink
            .send(DiscoveryRequest::WatchSessions {
                watch_options: Default::default(),
                session_watcher: slow_watcher_client,
                control_handle: dummy_control_handle.clone(),
            })
            .await?;

        // Connect fast watcher.
        let (fast_watcher_client, fast_watcher_server) =
            create_endpoints::<SessionsWatcherMarker>();
        let mut fast_watcher = fast_watcher_server.into_stream();
        discovery_request_sink
            .send(DiscoveryRequest::WatchSessions {
                watch_options: Default::default(),
                session_watcher: fast_watcher_client,
                control_handle: dummy_control_handle.clone(),
            })
            .await?;

        // Add a player.
        let inspector = Inspector::default();
        let (player_client, player_server) = create_endpoints::<PlayerMarker>();
        let player = Player::new(
            Id::new()?,
            player_client,
            PlayerRegistration {
                domain: Some(String::from("test_domain://")),
                ..Default::default()
            },
            inspector.root().create_child("test_player"),
            player_published_sink,
        )?;
        player_sink.send(player).await?;

        let mut player_requests = player_server.into_stream();

        // Send enough updates to exceed CHANNEL_BUFFER_SIZE (100).
        for _ in 0..150 {
            let info_change_responder = player_requests
                .try_next()
                .await?
                .expect("Receiving a request")
                .into_watch_info_change()
                .expect("Receiving info change responder");
            info_change_responder.send(&Default::default())?;

            // Fast watcher successfully receives every update and responds without hanging.
            let req = fast_watcher.try_next().await?.expect("Receiving watcher request");
            if let SessionsWatcherRequest::SessionUpdated { responder, .. } = req {
                responder.send()?;
            } else {
                panic!("Unexpected request");
            }
        }

        Ok(())
    }
}
