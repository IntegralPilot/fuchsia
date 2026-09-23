// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use flex_fuchsia_tracing_controller as trace;
use futures::FutureExt;
use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TriggerAction {
    Terminate,
}

impl From<trace::Action> for TriggerAction {
    fn from(t: trace::Action) -> Self {
        match t {
            trace::Action::Terminate => Self::Terminate,
            _ => panic!("Unknown trigger action: {t:?}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Trigger {
    /// See fuchsia.tracing.controller.Controller.WatchAlert for more info.
    pub alert: Option<String>,
    pub action: Option<TriggerAction>,
}

impl From<trace::Trigger> for Trigger {
    fn from(t: trace::Trigger) -> Self {
        Self { alert: t.alert, action: t.action.map(Into::into) }
    }
}

impl From<&trace::Trigger> for Trigger {
    fn from(t: &trace::Trigger) -> Self {
        Self { alert: t.alert.clone(), action: t.action.map(Into::into) }
    }
}
// A wrapper type for ffx::Trigger that does some unwrapping on allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TriggerSetItem {
    alert: String,
    action: TriggerAction,
}

impl TriggerSetItem {
    fn new(t: Trigger) -> Option<Self> {
        let alert = t.alert?;
        let action = t.action?;
        Some(Self { alert, action })
    }

    /// Convenience constructor for doing a lookup.
    fn lookup(alert: String) -> Self {
        Self { alert: alert, action: TriggerAction::Terminate }
    }
}

impl std::cmp::PartialOrd for TriggerSetItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for TriggerSetItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.alert.cmp(&other.alert)
    }
}

type TriggersFut<'a> = Pin<Box<dyn Future<Output = Option<TriggerAction>> + Send + 'a>>;

pub struct TriggersWatcher<'a> {
    inner: TriggersFut<'a>,
}

impl<'a> TriggersWatcher<'a> {
    pub fn new(
        controller: trace::SessionProxy,
        triggers: Vec<Trigger>,
        shutdown: async_channel::Receiver<()>,
    ) -> Self {
        Self {
            inner: Box::pin(async move {
                let items: Vec<_> =
                    triggers.into_iter().filter_map(|i| TriggerSetItem::new(i)).collect();
                let set: BTreeSet<TriggerSetItem> = items.iter().map(|t| t).cloned().collect();
                let mut shutdown_fut = std::pin::pin!(shutdown.recv().fuse());
                loop {
                    let mut watch_alert = controller.watch_alert().fuse();
                    futures::select! {
                        _ = shutdown_fut => {
                            log::info!("received shutdown alert");
                            break;
                        }
                        alert = watch_alert => {
                            let Ok(alert) = alert else { break };
                            log::info!("alert received: {}", alert);
                            let lookup_item = TriggerSetItem::lookup(alert);
                            if set.contains(&lookup_item) {
                                return set.get(&lookup_item).map(|s| s.action.clone());
                            }
                        }
                    }
                }
                None
            }),
        }
    }
}

impl Future for TriggersWatcher<'_> {
    type Output = Option<TriggerAction>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.inner).poll(cx)
    }
}
