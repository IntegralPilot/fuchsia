// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::events::router::EventConsumer;
use crate::events::types::{Event, EventPayload, LogSinkRequestedPayload};
use crate::identity::ComponentIdentity;
use crate::logs::container::LogsArtifactsContainer;
use crate::logs::debuglog::{DebugLog, DebugLogBridge, KERNEL_IDENTITY};
use crate::logs::shared_buffer::{FilterCursor, FilterCursorStream, SharedBuffer};
use crate::logs::stats::{GlobalAnalytics, LogStreamStats};
use anyhow::format_err;
use diagnostics_data::{LogsData, Severity};
use diagnostics_log_encoding::ARCHIVIST_URL;
use fidl_fuchsia_diagnostics::{
    ComponentSelector, LogInterestSelector, StreamMode, StringSelector,
};
use fidl_fuchsia_diagnostics_types::Severity as FidlSeverity;
use flyweights::FlyStr;
use fuchsia_async as fasync;
use fuchsia_inspect as inspect;
use fuchsia_sync::Mutex;
use futures::prelude::*;
use log::{LevelFilter, debug, error};
use moniker::{ExtendedMoniker, Moniker};
use selectors::SelectorExt;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};

// LINT.IfChange
#[derive(Ord, PartialOrd, Eq, PartialEq)]
pub struct ComponentInitialInterest {
    /// The URL or moniker for the component which should receive the initial interest.
    component: UrlOrMoniker,
    /// The log severity the initial interest should specify.
    log_severity: Severity,
}
// LINT.ThenChange(/src/lib/assembly/config_schema/src/platform_config/diagnostics_config.rs)

impl FromStr for ComponentInitialInterest {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut split = s.rsplitn(2, ":");
        match (split.next(), split.next()) {
            (Some(severity), Some(url_or_moniker)) => {
                let Ok(url_or_moniker) = UrlOrMoniker::from_str(url_or_moniker) else {
                    return Err(format_err!("invalid url or moniker"));
                };
                let Ok(severity) = Severity::from_str(severity) else {
                    return Err(format_err!("invalid severity"));
                };
                Ok(ComponentInitialInterest { log_severity: severity, component: url_or_moniker })
            }
            _ => Err(format_err!("invalid interest")),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Ord, PartialOrd)]
pub enum UrlOrMoniker {
    /// An absolute fuchsia url to a component.
    Url(FlyStr),
    /// The absolute moniker for a component.
    Moniker(ExtendedMoniker),
    /// A partial string to match against url or moniker.
    Partial(FlyStr),
}

impl FromStr for UrlOrMoniker {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl::from_str(s).is_ok()
            || fuchsia_url::boot::AbsoluteComponentUrl::parse(s).is_ok()
        {
            Ok(UrlOrMoniker::Url(s.into()))
        } else if s.starts_with("/") {
            if let Ok(moniker) = Moniker::from_str(s) {
                Ok(UrlOrMoniker::Moniker(ExtendedMoniker::ComponentInstance(moniker)))
            } else {
                Err(())
            }
        } else {
            Ok(UrlOrMoniker::Partial(s.into()))
        }
    }
}

/// Static ID, used for persistent changes to interest settings.
pub const STATIC_CONNECTION_ID: usize = 0;
static INTEREST_CONNECTION_ID: AtomicUsize = AtomicUsize::new(STATIC_CONNECTION_ID + 1);

/// This is set when Archivist first starts. This might not be set for unit tests.
pub static ARCHIVIST_MONIKER: OnceLock<Moniker> = OnceLock::new();

/// The IOBuffer writer tag used for Archivist logs. We rely on the first container we create having
/// this tag, so the Archivist container must be the first container created after creating the
/// shared buffer. This tag is used before we create the container because we want to set up logging
/// at the earliest possible moment after a binary starts.
pub const ARCHIVIST_TAG: u64 = 0;

/// LogsRepository holds all diagnostics data and is a singleton wrapped by multiple
/// [`pipeline::Pipeline`]s in a given Archivist instance.
pub struct LogsRepository {
    mutable_state: Mutex<LogsRepositoryState>,
    shared_buffer: Arc<SharedBuffer>,
    scope_handle: fasync::ScopeHandle,
}

impl LogsRepository {
    pub fn new(
        ring_buffer: ring_buffer::Reader,
        initial_interests: impl Iterator<Item = ComponentInitialInterest>,
        parent: &fuchsia_inspect::Node,
        scope: fasync::Scope,
    ) -> Arc<Self> {
        let scope_handle = scope.to_handle();
        Arc::new_cyclic(|me: &Weak<LogsRepository>| {
            let mut mutable_state = LogsRepositoryState::new(parent, initial_interests, scope);
            let me_clone = Weak::clone(me);
            let shared_buffer = SharedBuffer::new(
                ring_buffer,
                Box::new(move |identity| {
                    if let Some(this) = me_clone.upgrade() {
                        this.on_container_inactive(&identity);
                    }
                }),
                Default::default(),
                mutable_state.global_analytics.logs_node(),
            );
            if let Some(m) = ARCHIVIST_MONIKER.get() {
                let archivist_container = mutable_state.create_log_container(
                    Arc::new(ComponentIdentity::new(
                        ExtendedMoniker::ComponentInstance(m.clone()),
                        ARCHIVIST_URL,
                    )),
                    &shared_buffer,
                    Weak::clone(me),
                );
                // We rely on the first container we create ending up with the correct tag.
                assert_eq!(archivist_container.buffer().iob_tag(), ARCHIVIST_TAG);
            }
            LogsRepository { scope_handle, mutable_state: Mutex::new(mutable_state), shared_buffer }
        })
    }

    pub async fn flush(&self) {
        self.shared_buffer.flush().await;
    }

    /// Drain the kernel's debug log. The returned future completes once
    /// existing messages have been ingested.
    pub fn drain_debuglog<K>(self: &Arc<Self>, klog_reader: K)
    where
        K: DebugLog + Send + Sync + 'static,
    {
        let mut mutable_state = self.mutable_state.lock();

        // We can only have one klog reader, if this is already set, it means we are already
        // draining klog.
        if mutable_state.draining_klog {
            return;
        }
        mutable_state.draining_klog = true;

        let container =
            mutable_state.get_log_container(KERNEL_IDENTITY.clone(), &self.shared_buffer, self);
        let Some(ref scope) = mutable_state.scope else {
            return;
        };
        scope.spawn(async move {
            debug!("Draining debuglog.");
            let mut kernel_logger = DebugLogBridge::create(klog_reader);
            let mut messages = match kernel_logger.existing_logs() {
                Ok(messages) => messages,
                Err(e) => {
                    error!(e:%; "failed to read from kernel log, important logs may be missing");
                    return;
                }
            };
            messages.sort_by_key(|m| m.timestamp());
            for message in messages {
                container.ingest_message(message);
            }

            let res = kernel_logger
                .listen()
                .try_for_each(|message| async {
                    container.ingest_message(message);
                    Ok(())
                })
                .await;
            if let Err(e) = res {
                error!(e:%; "failed to drain kernel log, important logs may be missing");
            }
        });
    }

    pub fn logs_cursor_raw(
        &self,
        mode: StreamMode,
        selectors: Vec<ComponentSelector>,
    ) -> FilterCursor {
        self.shared_buffer.cursor(mode, selectors)
    }

    /// Returns a log stream filtered to the specified selectors. If `selectors` is empty, all logs
    /// are returned.
    pub fn logs_cursor(
        &self,
        mode: StreamMode,
        selectors: Vec<ComponentSelector>,
    ) -> FilterCursorStream<LogsData> {
        self.shared_buffer.cursor(mode, selectors).into()
    }

    /// Returns a log container.
    ///
    /// NOTE: This function does nothing to stop the container from being removed, so this is
    /// currently only suitable for test code.
    #[cfg(test)]
    pub fn get_log_container(
        self: &Arc<Self>,
        identity: Arc<ComponentIdentity>,
    ) -> Arc<LogsArtifactsContainer> {
        self.mutable_state.lock().get_log_container(identity, &self.shared_buffer, self)
    }

    /// Waits until `stop_accepting_new_log_sinks` is called and all log sink tasks have completed.
    /// After that, any pending Cursors will return Poll::Ready(None).
    pub async fn wait_for_termination(&self) {
        let Some(scope) = self.mutable_state.lock().scope.take() else {
            error!("Attempted to terminate twice");
            return;
        };
        scope.join().await;
        // Process messages from log sink.
        debug!("Log ingestion stopped.");
        // Terminate the shared buffer first so that pending messages are processed before we
        // terminate all the containers.
        self.shared_buffer.terminate().await;
        for container in self.mutable_state.lock().logs_data_store.values() {
            container.terminate();
        }
    }

    /// Closes the connection in which new logger draining tasks are sent. No more logger tasks
    /// will be accepted when this is called and we'll proceed to terminate logs.
    pub fn stop_accepting_new_log_sinks(&self) {
        self.scope_handle.close();
    }

    /// Returns an id to use for a new interest connection. Used by both LogSettings and Log, to
    /// ensure shared uniqueness of their connections.
    pub fn new_interest_connection(&self) -> usize {
        INTEREST_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// Updates log selectors associated with an interest connection.
    pub fn update_logs_interest(&self, connection_id: usize, selectors: Vec<LogInterestSelector>) {
        self.mutable_state.lock().update_logs_interest(connection_id, selectors);
    }

    /// Indicates that the connection associated with the given ID is now done.
    pub fn finish_interest_connection(&self, connection_id: usize) {
        self.mutable_state.lock().finish_interest_connection(connection_id);
    }

    fn on_container_inactive(&self, identity: &ComponentIdentity) {
        let mut repo = self.mutable_state.lock();
        if !repo.is_live(identity) {
            repo.remove(identity);
        }
    }
}

#[cfg(test)]
impl LogsRepository {
    pub fn for_test(scope: fasync::Scope) -> Arc<Self> {
        use crate::logs::shared_buffer::create_ring_buffer;

        LogsRepository::new(
            create_ring_buffer(crate::constants::LEGACY_DEFAULT_MAXIMUM_CACHED_LOGS_BYTES as usize),
            std::iter::empty(),
            &Default::default(),
            scope,
        )
    }
}

impl EventConsumer for LogsRepository {
    fn handle(self: Arc<Self>, event: Event) {
        match event.payload {
            EventPayload::LogSinkRequested(LogSinkRequestedPayload {
                component,
                request_stream,
            }) => {
                debug!(identity:% = component; "LogSink requested.");
                // NOTE: It is important that we hold the lock whilst we call
                // `Container::handle_log_sink` because otherwise the container could be removed by
                // `on_container_inactive`.  After calling `handle_log_sink`, the container cannot
                // be removed until after the `LogSink` channel is closed.
                let mut mutable_state = self.mutable_state.lock();
                let container =
                    mutable_state.get_log_container(component, &self.shared_buffer, &self);
                container.handle_log_sink(request_stream, self.scope_handle.clone());
            }
            _ => unreachable!("Archivist state just subscribes to log sink requested"),
        }
    }
}

pub struct LogsRepositoryState {
    logs_data_store: HashMap<Arc<ComponentIdentity>, Arc<LogsArtifactsContainer>>,
    inspect_node: inspect::Node,
    global_analytics: GlobalAnalytics,

    /// Interest registrations that we have received through fuchsia.logger.Log/ListWithSelectors
    /// or through fuchsia.logger.LogSettings/SetInterest.
    interest_registrations: BTreeMap<usize, Vec<LogInterestSelector>>,

    /// Whether or not we are draining the kernel log.
    draining_klog: bool,

    /// Scope where log ingestion tasks are running.
    scope: Option<fasync::Scope>,

    /// The initial log interests with which archivist was configured.
    initial_interests: BTreeMap<UrlOrMoniker, Severity>,
}

impl LogsRepositoryState {
    fn new(
        parent: &fuchsia_inspect::Node,
        initial_interests: impl Iterator<Item = ComponentInitialInterest>,
        scope: fasync::Scope,
    ) -> Self {
        Self {
            inspect_node: parent.create_child("log_sources"),
            logs_data_store: HashMap::new(),
            interest_registrations: BTreeMap::new(),
            draining_klog: false,
            initial_interests: initial_interests
                .map(|ComponentInitialInterest { component, log_severity }| {
                    (component, log_severity)
                })
                .collect(),
            scope: Some(scope),
            global_analytics: GlobalAnalytics::new(parent),
        }
    }

    /// Returns a container for logs artifacts, constructing one and adding it to the trie if
    /// necessary.
    pub fn get_log_container(
        &mut self,
        identity: Arc<ComponentIdentity>,
        shared_buffer: &Arc<SharedBuffer>,
        repo: &Arc<LogsRepository>,
    ) -> Arc<LogsArtifactsContainer> {
        match self.logs_data_store.get(&identity) {
            None => self.create_log_container(identity, shared_buffer, Arc::downgrade(repo)),
            Some(existing) => Arc::clone(existing),
        }
    }

    fn create_log_container(
        &mut self,
        identity: Arc<ComponentIdentity>,
        shared_buffer: &Arc<SharedBuffer>,
        repo: Weak<LogsRepository>,
    ) -> Arc<LogsArtifactsContainer> {
        let initial_interest = self.get_initial_interest(identity.as_ref());
        let stats = Arc::new(LogStreamStats::new(&self.inspect_node, &identity));
        let buffer = shared_buffer.new_container_buffer(Arc::clone(&identity), Arc::clone(&stats));
        let container = Arc::new(LogsArtifactsContainer::new(
            Arc::clone(&identity),
            self.interest_registrations.values().flat_map(|s| s.iter()),
            initial_interest,
            stats,
            buffer,
            Some(Box::new(move |c| {
                if let Some(repo) = repo.upgrade() {
                    repo.on_container_inactive(&c.identity)
                }
            })),
        ));
        self.logs_data_store.insert(Arc::clone(&identity), Arc::clone(&container));
        container
    }

    fn get_initial_interest(&self, identity: &ComponentIdentity) -> Option<FidlSeverity> {
        let exact_url_severity =
            self.initial_interests.get(&UrlOrMoniker::Url(identity.url.clone())).copied();
        let exact_moniker_severity =
            self.initial_interests.get(&UrlOrMoniker::Moniker(identity.moniker.clone())).copied();

        let partial_severity = self
            .initial_interests
            .iter()
            .filter_map(|(uom, severity)| match uom {
                UrlOrMoniker::Partial(p) => {
                    if identity.url.contains(p.as_str())
                        || identity.moniker.to_string().contains(p.as_str())
                    {
                        Some(*severity)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .min();

        [exact_url_severity, exact_moniker_severity, partial_severity]
            .into_iter()
            .flatten()
            .min()
            .map(FidlSeverity::from)
    }

    fn is_live(&self, identity: &ComponentIdentity) -> bool {
        match self.logs_data_store.get(identity) {
            Some(container) => container.is_active(),
            None => false,
        }
    }

    /// Updates our own log interest if we are the root Archivist and logging
    /// to klog.
    fn maybe_update_own_logs_interest(
        &mut self,
        selectors: &[LogInterestSelector],
        clear_interest: bool,
    ) {
        let Some(moniker) = ARCHIVIST_MONIKER.get() else { return };
        let lowest_selector = selectors
            .iter()
            .filter(|selector| {
                // If this is an embedded archivist, the wildcard selector "**" is used (in
                // tests at least) to change the interest level on all components. Since
                // archivist logs to itself, this creates a situation where archivist logs can
                // get included which is undesirable in the vast majority of cases. To address
                // this, we prevent the global wildcard pattern "**" and "*" from matching
                // archivist. This is clearly a bit of a hack, but it is balanced by it being
                // the behavior that the vast majority of users will want. It is still possible
                // to change the interest level for archivist by using an exact match. Note that
                // this will apply to both the embedded and system archivist to keep things
                // consistent.
                if selector.selector.moniker_segments.as_ref().is_some_and(|s| {
                    matches!(
                        &s[..],
                        [StringSelector::StringPattern(s)] if s == "**" || s == "*"
                    )
                }) {
                    return false;
                }

                moniker.matches_component_selector(&selector.selector).unwrap_or(false)
            })
            .min_by_key(|selector| selector.interest.min_severity.unwrap_or(FidlSeverity::Info));
        if let Some(selector) = lowest_selector {
            if clear_interest {
                log::set_max_level(LevelFilter::Info);
            } else {
                log::set_max_level(
                    match selector.interest.min_severity.unwrap_or(FidlSeverity::Info) {
                        FidlSeverity::Trace => LevelFilter::Trace,
                        FidlSeverity::Debug => LevelFilter::Debug,
                        FidlSeverity::Info => LevelFilter::Info,
                        FidlSeverity::Warn => LevelFilter::Warn,
                        FidlSeverity::Error => LevelFilter::Error,
                        // Log has no "Fatal" level, so set it to Error
                        // instead.
                        FidlSeverity::Fatal => LevelFilter::Error,
                        FidlSeverity::__SourceBreaking { .. } => return,
                    },
                );
            }
        }
    }

    fn update_logs_interest(&mut self, connection_id: usize, selectors: Vec<LogInterestSelector>) {
        self.maybe_update_own_logs_interest(&selectors, false);
        let previous_selectors =
            self.interest_registrations.insert(connection_id, selectors).unwrap_or_default();
        // unwrap safe, we just inserted.
        let new_selectors = self.interest_registrations.get(&connection_id).unwrap();
        for logs_data in self.logs_data_store.values() {
            logs_data.update_interest(new_selectors.iter(), &previous_selectors);
        }
    }

    pub fn finish_interest_connection(&mut self, connection_id: usize) {
        let selectors = self.interest_registrations.remove(&connection_id);
        if let Some(selectors) = selectors {
            self.maybe_update_own_logs_interest(&selectors, true);
            for logs_data in self.logs_data_store.values() {
                logs_data.reset_interest(&selectors);
            }
        }
    }

    pub fn remove(&mut self, identity: &ComponentIdentity) {
        self.logs_data_store.remove(identity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::shared_buffer::create_ring_buffer;
    use crate::logs::testing::make_message;
    use fidl_fuchsia_diagnostics::StreamMode;
    use fidl_fuchsia_logger::LogSinkMarker;
    use fuchsia_inspect::Inspector;
    use moniker::ExtendedMoniker;
    use ring_buffer::MAX_MESSAGE_SIZE;
    use selectors::{FastError, SelectorExt};
    use std::time::Duration;

    #[fuchsia::test]
    async fn data_repo_filters_logs_by_selectors() {
        let repo = LogsRepository::for_test(fasync::Scope::new());
        let foo_container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./foo").unwrap(),
            "fuchsia-pkg://foo",
        )));
        let bar_container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./bar").unwrap(),
            "fuchsia-pkg://bar",
        )));

        foo_container.ingest_message(make_message("a", None, zx::BootInstant::from_nanos(1)));
        bar_container.ingest_message(make_message("b", None, zx::BootInstant::from_nanos(2)));
        foo_container.ingest_message(make_message("c", None, zx::BootInstant::from_nanos(3)));

        let stream = repo.logs_cursor(StreamMode::Snapshot, Vec::new());

        let results =
            stream.map(|value| value.msg().unwrap().to_string()).collect::<Vec<_>>().await;
        assert_eq!(results, vec!["a".to_string(), "b".to_string(), "c".to_string()]);

        let filtered_stream = repo.logs_cursor(
            StreamMode::Snapshot,
            vec![selectors::parse_component_selector::<FastError>("foo").unwrap()],
        );

        let results =
            filtered_stream.map(|value| value.msg().unwrap().to_string()).collect::<Vec<_>>().await;
        assert_eq!(results, vec!["a".to_string(), "c".to_string()]);
    }

    #[fuchsia::test]
    async fn data_repo_correctly_sets_initial_interests() {
        let repo = LogsRepository::new(
            create_ring_buffer(100000),
            [
                ComponentInitialInterest {
                    component: UrlOrMoniker::Url("fuchsia-pkg://bar".into()),
                    log_severity: Severity::Info,
                },
                ComponentInitialInterest {
                    component: UrlOrMoniker::Url("fuchsia-pkg://baz".into()),
                    log_severity: Severity::Warn,
                },
                ComponentInitialInterest {
                    component: UrlOrMoniker::Moniker("/core/bar".try_into().unwrap()),
                    log_severity: Severity::Error,
                },
                ComponentInitialInterest {
                    component: UrlOrMoniker::Moniker("/core/foo".try_into().unwrap()),
                    log_severity: Severity::Debug,
                },
            ]
            .into_iter(),
            &fuchsia_inspect::Node::default(),
            fasync::Scope::new(),
        );

        // We have the moniker configured, use the associated severity.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/foo").unwrap(),
            "fuchsia-pkg://foo",
        )));
        expect_initial_interest(Some(FidlSeverity::Debug), container, repo.scope_handle.clone())
            .await;

        // We have the URL configure, use the associated severity.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/baz").unwrap(),
            "fuchsia-pkg://baz",
        )));
        expect_initial_interest(Some(FidlSeverity::Warn), container, repo.scope_handle.clone())
            .await;

        // We have both a URL and a moniker in the config. Pick the minimium one, in this case Info
        // for the URL over Error for the moniker.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/bar").unwrap(),
            "fuchsia-pkg://bar",
        )));
        expect_initial_interest(Some(FidlSeverity::Info), container, repo.scope_handle.clone())
            .await;

        // Neither the moniker nor the URL have an associated severity, therefore, the minimum
        // severity isn't set.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/quux").unwrap(),
            "fuchsia-pkg://quux",
        )));
        expect_initial_interest(None, container, repo.scope_handle.clone()).await;
    }

    #[fuchsia::test]
    async fn data_repo_correctly_handles_partial_matching() {
        let repo = LogsRepository::new(
            create_ring_buffer(100000),
            [
                "fuchsia-pkg://fuchsia.com/bar#meta/bar.cm:INFO".parse(),
                "fuchsia-pkg://fuchsia.com/baz#meta/baz.cm:WARN".parse(),
                "/core/bust:DEBUG".parse(),
                "core/bar:ERROR".parse(),
                "foo:DEBUG".parse(),
                "both:TRACE".parse(),
            ]
            .into_iter()
            .map(Result::unwrap),
            &fuchsia_inspect::Node::default(),
            fasync::Scope::new(),
        );

        // We have a partial moniker configured, use the associated severity.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/foo").unwrap(),
            "fuchsia-pkg://fuchsia.com/not-foo#meta/not-foo.cm",
        )));
        expect_initial_interest(Some(FidlSeverity::Debug), container, repo.scope_handle.clone())
            .await;

        // We have a partial url configured, use the associated severity.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/not-foo").unwrap(),
            "fuchsia-pkg://fuchsia.com/foo#meta/foo.cm",
        )));
        expect_initial_interest(Some(FidlSeverity::Debug), container, repo.scope_handle.clone())
            .await;

        // We have the URL configure, use the associated severity.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/baz").unwrap(),
            "fuchsia-pkg://fuchsia.com/baz#meta/baz.cm",
        )));
        expect_initial_interest(Some(FidlSeverity::Warn), container, repo.scope_handle.clone())
            .await;

        // We have both a URL and a moniker in the config. Pick the minimum one, in this case Info
        // for the URL over Error for the moniker.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/bar").unwrap(),
            "fuchsia-pkg://fuchsia.com/bar#meta/bar.cm",
        )));
        expect_initial_interest(Some(FidlSeverity::Info), container, repo.scope_handle.clone())
            .await;

        // Neither the moniker nor the URL have an associated severity, therefore, the minimum
        // severity isn't set.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/quux").unwrap(),
            "fuchsia-pkg://fuchsia.com/quux#meta/quux.cm",
        )));
        expect_initial_interest(None, container, repo.scope_handle.clone()).await;

        // We have a partial match for both moniker and url, should still work.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/both").unwrap(),
            "fuchsia-pkg://fuchsia.com/both#meta/both.cm",
        )));
        expect_initial_interest(Some(FidlSeverity::Trace), container, repo.scope_handle.clone())
            .await;

        // Exact moniker match should not match sub-monikers.
        let container = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("core/bust/testing").unwrap(),
            "fuchsia-pkg://fuchsia.com/busted#meta/busted.cm",
        )));
        expect_initial_interest(None, container, repo.scope_handle.clone()).await;
    }

    async fn expect_initial_interest(
        expected_severity: Option<FidlSeverity>,
        container: Arc<LogsArtifactsContainer>,
        scope: fasync::ScopeHandle,
    ) {
        let (log_sink, stream) = fidl::endpoints::create_proxy_and_stream::<LogSinkMarker>();
        container.handle_log_sink(stream, scope);
        let initial_interest = log_sink.wait_for_interest_change().await.unwrap().unwrap();
        assert_eq!(initial_interest.min_severity, expected_severity);
    }

    #[fuchsia::test]
    async fn inspect_node_cleaned_up_on_roll_out() {
        let inspector = Inspector::default();
        let repo = LogsRepository::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            std::iter::empty(),
            inspector.root(),
            fasync::Scope::new(),
        );

        let identity_foo = Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./foo").unwrap(),
            "fuchsia-pkg://foo",
        ));

        // Create container A
        let container_foo = repo.get_log_container(Arc::clone(&identity_foo));
        container_foo.ingest_message(make_message("a", None, zx::BootInstant::from_nanos(1)));

        // Force SharedBuffer to scan messages and update msg_ids.end.
        // Without this, ContainerInfo::is_active() evaluates to false immediately after
        // mark_stopped(), causing repo.is_live() to return false before rollout happens.
        let _cursor = repo.logs_cursor(
            StreamMode::Subscribe,
            vec![identity_foo.moniker.clone().into_component_selector()],
        );

        container_foo.mark_stopped();
        drop(container_foo);

        // Verify stats exist
        let hierarchy = fuchsia_inspect::reader::read(&inspector).await.unwrap();
        assert!(
            hierarchy.get_child_by_path(&["log_sources", "foo"]).is_some(),
            "foo stats must exist initially"
        );

        // Ingest messages for another component until foo is rolled out
        let container_bar = repo.get_log_container(Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./bar").unwrap(),
            "fuchsia-pkg://bar",
        )));

        let large_str = "b".repeat(1000);
        for i in 2..1000 {
            container_bar.ingest_message(make_message(
                &large_str,
                None,
                zx::BootInstant::from_nanos(i),
            ));
            fasync::Timer::new(Duration::from_millis(10)).await;
            if !repo.mutable_state.lock().is_live(&identity_foo) {
                break;
            }
        }

        assert!(
            !repo.mutable_state.lock().is_live(&identity_foo),
            "foo container must be inactive after rollout"
        );

        // Verify stats are cleaned up
        let hierarchy = fuchsia_inspect::reader::read(&inspector).await.unwrap();
        assert!(
            hierarchy.get_child_by_path(&["log_sources", "foo"]).is_none(),
            "foo stats must be cleaned up after rollout"
        );
    }
}
