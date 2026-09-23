// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::EditTransactionError;
use crate::rule::Rule;
use flex_client::ProxyHasDomain;
use flex_fuchsia_pkg_rewrite as rewrite;
use flex_fuchsia_pkg_rewrite::{EditTransactionProxy, EngineProxy};
use std::future::Future;
use zx_status as zx;

const RETRY_ATTEMPTS: usize = 100;

/// A helper for managing the editing of rewrite rules.
pub struct EditTransaction {
    transaction: EditTransactionProxy,
}

impl EditTransaction {
    /// Removes all dynamically configured rewrite rules, leaving only any
    /// statically configured rules.
    pub fn reset_all(&self) -> Result<(), EditTransactionError> {
        self.transaction.reset_all().map_err(EditTransactionError::from)
    }

    /// Returns a vector of all dynamic (editable) rewrite rules. The
    /// vector will reflect any changes made to the rewrite rules so far in
    /// this transaction.
    pub async fn list_dynamic(&self) -> Result<Vec<Rule>, EditTransactionError> {
        let (iter, iter_server_end) =
            self.transaction.domain().create_proxy::<rewrite::RuleIteratorMarker>();
        self.transaction.list_dynamic(iter_server_end)?;

        let mut rules = Vec::new();
        loop {
            let chunk = iter.next().await?;
            if chunk.is_empty() {
                break;
            }

            for rule in chunk {
                rules.push(Rule::try_from(rule)?);
            }
        }

        Ok(rules)
    }

    /// Adds a rewrite rule with highest priority. If `rule` already exists, this
    /// API will prioritize it over other rules.
    pub async fn add(&self, rule: Rule) -> Result<(), EditTransactionError> {
        self.transaction
            .add(&rule.into())
            .await?
            .map_err(|err| EditTransactionError::AddError(zx::Status::err_from_raw(err)))
    }
}

/// Perform a rewrite rule edit transaction, retrying as necessary if another edit transaction runs
/// concurrently.
///
/// The given callback `cb` should perform the needed edits to the state of the rewrite rules but
/// not attempt to `commit()` the transaction. `do_transaction` will internally attempt to commit
/// the transaction and trigger a retry if necessary.
pub async fn do_transaction<T, R>(engine: &EngineProxy, cb: T) -> Result<(), EditTransactionError>
where
    T: Fn(EditTransaction) -> R,
    R: Future<Output = Result<EditTransaction, EditTransactionError>>,
{
    // Make a reasonable effort to retry the edit after a concurrent edit, but don't retry forever.
    for _ in 0..RETRY_ATTEMPTS {
        let (transaction, transaction_server_end) =
            engine.domain().create_proxy::<rewrite::EditTransactionMarker>();

        let () = engine
            .start_edit_transaction(transaction_server_end)
            .map_err(EditTransactionError::from)?;

        let transaction = cb(EditTransaction { transaction }).await?;

        let response =
            transaction.transaction.commit().await.map_err(EditTransactionError::from)?;

        // Retry edit transaction on concurrent edit
        return match response.map_err(zx::Status::err_from_raw) {
            Ok(()) => Ok(()),
            Err(zx::Status::UNAVAILABLE) => {
                continue;
            }
            Err(status) => Err(EditTransactionError::CommitError(status)),
        };
    }

    Err(EditTransactionError::CommitError(zx::Status::UNAVAILABLE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use flex_fuchsia_pkg_rewrite::{
        EditTransactionRequest, EngineMarker, EngineRequest, RuleIteratorRequest,
    };
    use fuchsia_async as fasync;
    use futures::TryStreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, PartialEq)]
    enum Event {
        ResetAll,
        ListDynamic,
        IteratorNext,
        Add(Rule),
        CommitFailed,
        Commit,
    }

    struct Engine {
        engine: EngineProxy,
        events: Arc<Mutex<Vec<Event>>>,
        #[cfg(feature = "fdomain")]
        _client: Arc<flex_client::Client>,
    }

    macro_rules! rule {
        ($host_match:expr => $host_replacement:expr,
         $path_prefix_match:expr => $path_prefix_replacement:expr) => {
            Rule::new($host_match, $host_replacement, $path_prefix_match, $path_prefix_replacement)
                .unwrap()
        };
    }

    impl Engine {
        fn new() -> Self {
            Self::with_fail_attempts(0, zx::Status::INTERNAL)
        }

        fn with_fail_attempts(mut fail_attempts: usize, fail_status: zx::Status) -> Self {
            #[cfg(feature = "fdomain")]
            let client = fdomain_local::local_client_empty();
            #[cfg(not(feature = "fdomain"))]
            let client = flex_client::fidl::ZirconClient;
            let events = Arc::new(Mutex::new(Vec::new()));
            let events_task = Arc::clone(&events);

            let (engine, mut engine_stream) = client.create_proxy_and_stream::<EngineMarker>();

            fasync::Task::local(async move {
                while let Some(req) = engine_stream.try_next().await.unwrap() {
                    match req {
                        EngineRequest::StartEditTransaction { transaction, control_handle: _ } => {
                            let mut tx_stream = transaction.into_stream();

                            while let Some(req) = tx_stream.try_next().await.unwrap() {
                                match req {
                                    EditTransactionRequest::ResetAll { control_handle: _ } => {
                                        events_task.lock().unwrap().push(Event::ResetAll);
                                    }
                                    EditTransactionRequest::ListDynamic {
                                        iterator,
                                        control_handle: _,
                                    } => {
                                        events_task.lock().unwrap().push(Event::ListDynamic);
                                        let mut stream = iterator.into_stream();

                                        let mut rules = vec![
                                            rule!("fuchsia.com" => "example.com", "/" => "/"),
                                            rule!("fuchsia.com" => "mycorp.com", "/" => "/"),
                                        ]
                                        .into_iter();

                                        while let Some(req) = stream.try_next().await.unwrap() {
                                            let RuleIteratorRequest::Next { responder } = req;
                                            events_task.lock().unwrap().push(Event::IteratorNext);

                                            if let Some(rule) = rules.next() {
                                                responder.send(&[rule.into()]).unwrap();
                                            } else {
                                                responder.send(&[]).unwrap();
                                            }
                                        }
                                    }
                                    EditTransactionRequest::Add { rule, responder } => {
                                        events_task
                                            .lock()
                                            .unwrap()
                                            .push(Event::Add(rule.try_into().unwrap()));
                                        responder.send(Ok(())).unwrap();
                                    }
                                    EditTransactionRequest::Commit { responder } => {
                                        if fail_attempts > 0 {
                                            fail_attempts -= 1;
                                            events_task.lock().unwrap().push(Event::CommitFailed);
                                            responder.send(Err(fail_status.into_raw())).unwrap();
                                        } else {
                                            events_task.lock().unwrap().push(Event::Commit);
                                            responder.send(Ok(())).unwrap();
                                        }
                                    }
                                }
                            }
                        }
                        _ => {
                            panic!("unexpected reqest: {:?}", req);
                        }
                    }
                }
            })
            .detach();

            Self {
                engine,
                events,
                #[cfg(feature = "fdomain")]
                _client: client,
            }
        }

        fn take_events(&self) -> Vec<Event> {
            self.events.lock().unwrap().drain(..).collect()
        }
    }

    #[fuchsia::test]
    async fn test_do_transaction_empty_always_commits() {
        let engine = Engine::new();

        do_transaction(&engine.engine, |transaction| async { Ok(transaction) }).await.unwrap();

        assert_eq!(engine.take_events(), vec![Event::Commit]);
    }

    #[fuchsia::test]
    async fn test_do_transaction_reset_all() {
        let engine = Engine::new();

        do_transaction(&engine.engine, |transaction| async {
            transaction.reset_all()?;
            Ok(transaction)
        })
        .await
        .unwrap();

        assert_eq!(engine.take_events(), vec![Event::ResetAll, Event::Commit]);
    }

    #[fuchsia::test]
    async fn test_do_transaction_list_dynamic() {
        let engine = Engine::new();

        do_transaction(&engine.engine, |transaction| async {
            let rules = transaction.list_dynamic().await?;
            assert_eq!(
                rules,
                vec![
                    rule!("fuchsia.com" => "example.com", "/" => "/"),
                    rule!("fuchsia.com" => "mycorp.com", "/" => "/"),
                ]
            );
            Ok(transaction)
        })
        .await
        .unwrap();

        assert_eq!(
            engine.take_events(),
            // We should get three iterators. The first two get the rules, the last gets nothing.
            vec![
                Event::ListDynamic,
                Event::IteratorNext,
                Event::IteratorNext,
                Event::IteratorNext,
                Event::Commit
            ]
        );
    }

    #[fuchsia::test]
    async fn test_do_transaction_add() {
        let engine = Engine::new();

        let attempts = Arc::new(AtomicUsize::new(0));
        do_transaction(&engine.engine, |transaction| async {
            attempts.fetch_add(1, Ordering::SeqCst);
            transaction.add(rule!("foo.com" => "bar.com", "/" => "/")).await?;
            transaction.add(rule!("baz.com" => "boo.com", "/" => "/")).await?;
            Ok(transaction)
        })
        .await
        .unwrap();

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            engine.take_events(),
            vec![
                Event::Add(rule!("foo.com" => "bar.com", "/" => "/")),
                Event::Add(rule!("baz.com" => "boo.com", "/" => "/")),
                Event::Commit,
            ],
        );
    }

    #[fuchsia::test]
    async fn test_do_transaction_closure_error_does_not_commit() {
        let engine = Engine::new();

        let attempts = Arc::new(AtomicUsize::new(0));
        let err = do_transaction(&engine.engine, |_transaction| async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(EditTransactionError::AddError(zx::Status::INTERNAL))
        })
        .await
        .unwrap_err();

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_matches!(err, EditTransactionError::AddError(zx::Status::INTERNAL));
        assert_eq!(engine.take_events(), vec![]);
    }

    #[fuchsia::test]
    async fn test_do_transaction_retries_commit_errors() {
        let engine = Engine::with_fail_attempts(5, zx::Status::UNAVAILABLE);

        let attempts = Arc::new(AtomicUsize::new(0));
        do_transaction(&engine.engine, |transaction| async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(transaction)
        })
        .await
        .unwrap();

        assert_eq!(attempts.load(Ordering::SeqCst), 6);
        assert_eq!(
            engine.take_events(),
            vec![
                Event::CommitFailed,
                Event::CommitFailed,
                Event::CommitFailed,
                Event::CommitFailed,
                Event::CommitFailed,
                Event::Commit,
            ]
        );
    }

    #[fuchsia::test]
    async fn test_do_transaction_eventually_gives_up() {
        let engine = Engine::with_fail_attempts(RETRY_ATTEMPTS + 1, zx::Status::UNAVAILABLE);

        let attempts = Arc::new(AtomicUsize::new(0));
        let err = do_transaction(&engine.engine, |transaction| async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(transaction)
        })
        .await
        .unwrap_err();

        assert_eq!(attempts.load(Ordering::SeqCst), RETRY_ATTEMPTS);
        assert_matches!(err, EditTransactionError::CommitError(zx::Status::UNAVAILABLE));
        assert_eq!(
            engine.take_events(),
            (0..RETRY_ATTEMPTS).map(|_| Event::CommitFailed).collect::<Vec<_>>(),
        );
    }

    #[fuchsia::test]
    async fn test_do_transaction_does_not_retry_other_errors() {
        let engine = Engine::with_fail_attempts(5, zx::Status::INTERNAL);

        let attempts = Arc::new(AtomicUsize::new(0));
        let err = do_transaction(&engine.engine, |transaction| async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(transaction)
        })
        .await
        .unwrap_err();

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_matches!(err, EditTransactionError::CommitError(zx::Status::INTERNAL));
        assert_eq!(engine.take_events(), vec![Event::CommitFailed,]);
    }
}
