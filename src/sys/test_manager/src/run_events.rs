// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ClientEnd;
use fidl_fuchsia_test_manager as ftest_manager;
use ftest_manager::{
    CaseStatus, DebugDataIteratorMarker, Event as FidlEvent, EventDetails as FidlEventDetails,
    RunEvent as FidlRunEvent, RunEventPayload as FidlRunEventPayload, SuiteEvent as FidlSuiteEvent,
    SuiteEventPayload as FidlSuiteEventPayload, SuiteResult, SuiteStatus, TestCaseResult,
};

pub(crate) enum RunEventPayload {
    DebugData(ClientEnd<DebugDataIteratorMarker>),
}

pub(crate) struct RunEvent {
    timestamp: i64,
    payload: RunEventPayload,
}

impl Into<FidlRunEvent> for RunEvent {
    fn into(self) -> FidlRunEvent {
        match self.payload {
            RunEventPayload::DebugData(client) => FidlRunEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlRunEventPayload::Artifact(ftest_manager::Artifact::DebugData(
                    client,
                ))),
                ..Default::default()
            },
        }
    }
}

impl RunEvent {
    pub fn debug_data(client: ClientEnd<DebugDataIteratorMarker>) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: RunEventPayload::DebugData(client),
        }
    }

    #[cfg(test)]
    pub fn into_payload(self) -> RunEventPayload {
        self.payload
    }
}

pub(crate) enum SuiteEventPayload {
    CaseFound(String, u32),
    CaseStarted(u32),
    CaseStopped(u32, CaseStatus),
    CaseFinished(u32),
    CaseStdout(u32, zx::Socket),
    CaseStderr(u32, zx::Socket),
    CustomArtifact(ftest_manager::CustomArtifact),
    SuiteSyslog(ftest_manager::Syslog),
    SuiteStderr(zx::Socket),
    SuiteStarted,
    SuiteStopped(SuiteStatus),
    DebugData(ClientEnd<DebugDataIteratorMarker>),
}

pub struct SuiteEvents {
    pub timestamp: i64,
    pub payload: SuiteEventPayload,
}

impl Into<FidlSuiteEvent> for SuiteEvents {
    fn into(self) -> FidlSuiteEvent {
        match self.payload {
            SuiteEventPayload::CaseFound(name, identifier) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::CaseFound(ftest_manager::CaseFound {
                    test_case_name: name,
                    identifier,
                })),
                ..Default::default()
            },
            SuiteEventPayload::CaseStarted(identifier) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::CaseStarted(ftest_manager::CaseStarted {
                    identifier,
                })),
                ..Default::default()
            },
            SuiteEventPayload::CaseStopped(identifier, status) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::CaseStopped(ftest_manager::CaseStopped {
                    identifier,
                    status,
                })),
                ..Default::default()
            },
            SuiteEventPayload::CaseFinished(identifier) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::CaseFinished(ftest_manager::CaseFinished {
                    identifier,
                })),
                ..Default::default()
            },
            SuiteEventPayload::CaseStdout(identifier, socket) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::CaseArtifact(ftest_manager::CaseArtifact {
                    identifier,
                    artifact: ftest_manager::Artifact::Stdout(socket),
                })),
                ..Default::default()
            },
            SuiteEventPayload::CaseStderr(identifier, socket) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::CaseArtifact(ftest_manager::CaseArtifact {
                    identifier,
                    artifact: ftest_manager::Artifact::Stderr(socket),
                })),
                ..Default::default()
            },
            SuiteEventPayload::CustomArtifact(custom) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::SuiteArtifact(ftest_manager::SuiteArtifact {
                    artifact: ftest_manager::Artifact::Custom(custom),
                })),
                ..Default::default()
            },
            SuiteEventPayload::SuiteSyslog(syslog) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::SuiteArtifact(ftest_manager::SuiteArtifact {
                    artifact: ftest_manager::Artifact::Log(syslog),
                })),
                ..Default::default()
            },
            SuiteEventPayload::SuiteStderr(socket) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::SuiteArtifact(ftest_manager::SuiteArtifact {
                    artifact: ftest_manager::Artifact::Stderr(socket),
                })),
                ..Default::default()
            },
            SuiteEventPayload::SuiteStarted => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::SuiteStarted(ftest_manager::SuiteStarted {})),
                ..Default::default()
            },
            SuiteEventPayload::SuiteStopped(status) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::SuiteStopped(ftest_manager::SuiteStopped {
                    status,
                })),
                ..Default::default()
            },
            SuiteEventPayload::DebugData(client) => FidlSuiteEvent {
                timestamp: Some(self.timestamp),
                payload: Some(FidlSuiteEventPayload::SuiteArtifact(ftest_manager::SuiteArtifact {
                    artifact: ftest_manager::Artifact::DebugData(client),
                })),
                ..Default::default()
            },
        }
    }
}

fn to_case_result(status: CaseStatus) -> TestCaseResult {
    match status {
        CaseStatus::Passed => TestCaseResult::Passed,
        CaseStatus::Failed => TestCaseResult::Failed,
        CaseStatus::TimedOut => TestCaseResult::TimedOut,
        CaseStatus::Skipped => TestCaseResult::Skipped,
        CaseStatus::Error => TestCaseResult::Error,
        _ => TestCaseResult::Error,
    }
}

fn to_suite_result(status: SuiteStatus) -> SuiteResult {
    match status {
        SuiteStatus::Passed => SuiteResult::Finished,
        SuiteStatus::Failed => SuiteResult::Failed,
        SuiteStatus::DidNotFinish => SuiteResult::DidNotFinish,
        SuiteStatus::TimedOut => SuiteResult::TimedOut,
        SuiteStatus::Stopped => SuiteResult::Stopped,
        SuiteStatus::InternalError => SuiteResult::InternalError,
        _ => SuiteResult::InternalError,
    }
}

impl Into<FidlEvent> for SuiteEvents {
    fn into(self) -> FidlEvent {
        match self.payload {
            SuiteEventPayload::SuiteStarted => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::SuiteStarted(
                    ftest_manager::SuiteStartedEventDetails { ..Default::default() },
                )),
                ..Default::default()
            },
            SuiteEventPayload::CaseFound(name, identifier) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::TestCaseFound(
                    ftest_manager::TestCaseFoundEventDetails {
                        test_case_name: Some(name),
                        test_case_id: Some(identifier),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            SuiteEventPayload::CaseStarted(identifier) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::TestCaseStarted(
                    ftest_manager::TestCaseStartedEventDetails {
                        test_case_id: Some(identifier),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            SuiteEventPayload::CaseStopped(identifier, status) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::TestCaseStopped(
                    ftest_manager::TestCaseStoppedEventDetails {
                        test_case_id: Some(identifier),
                        result: Some(to_case_result(status)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            SuiteEventPayload::CaseFinished(identifier) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::TestCaseFinished(
                    ftest_manager::TestCaseFinishedEventDetails {
                        test_case_id: Some(identifier),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            SuiteEventPayload::CaseStdout(identifier, socket) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::TestCaseArtifactGenerated(
                    ftest_manager::TestCaseArtifactGeneratedEventDetails {
                        test_case_id: Some(identifier),
                        artifact: Some(ftest_manager::Artifact::Stdout(socket)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            SuiteEventPayload::CaseStderr(identifier, socket) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::TestCaseArtifactGenerated(
                    ftest_manager::TestCaseArtifactGeneratedEventDetails {
                        test_case_id: Some(identifier),
                        artifact: Some(ftest_manager::Artifact::Stderr(socket)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            SuiteEventPayload::CustomArtifact(custom) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::SuiteArtifactGenerated(
                    ftest_manager::SuiteArtifactGeneratedEventDetails {
                        artifact: Some(ftest_manager::Artifact::Custom(custom)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            SuiteEventPayload::SuiteSyslog(syslog) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::SuiteArtifactGenerated(
                    ftest_manager::SuiteArtifactGeneratedEventDetails {
                        artifact: Some(ftest_manager::Artifact::Log(syslog)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            SuiteEventPayload::SuiteStderr(socket) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::SuiteArtifactGenerated(
                    ftest_manager::SuiteArtifactGeneratedEventDetails {
                        artifact: Some(ftest_manager::Artifact::Stderr(socket)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            SuiteEventPayload::SuiteStopped(status) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::SuiteStopped(
                    ftest_manager::SuiteStoppedEventDetails {
                        result: Some(to_suite_result(status)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            SuiteEventPayload::DebugData(client) => FidlEvent {
                timestamp: Some(self.timestamp),
                details: Some(FidlEventDetails::SuiteArtifactGenerated(
                    ftest_manager::SuiteArtifactGeneratedEventDetails {
                        artifact: Some(ftest_manager::Artifact::DebugData(client)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        }
    }
}

impl SuiteEvents {
    pub fn case_found(identifier: u32, name: String) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::CaseFound(name, identifier),
        }
    }

    pub fn case_started(identifier: u32) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::CaseStarted(identifier),
        }
    }

    pub fn case_stopped(identifier: u32, status: CaseStatus) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::CaseStopped(identifier, status),
        }
    }

    pub fn case_finished(identifier: u32) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::CaseFinished(identifier),
        }
    }

    pub fn case_stdout(identifier: u32, socket: zx::Socket) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::CaseStdout(identifier, socket),
        }
    }

    pub fn case_stderr(identifier: u32, socket: zx::Socket) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::CaseStderr(identifier, socket),
        }
    }

    pub fn suite_syslog(syslog: ftest_manager::Syslog) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::SuiteSyslog(syslog),
        }
    }

    pub fn suite_stderr(socket: zx::Socket) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::SuiteStderr(socket),
        }
    }

    pub fn suite_custom_artifact(custom: ftest_manager::CustomArtifact) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::CustomArtifact(custom),
        }
    }

    pub fn suite_started() -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::SuiteStarted,
        }
    }

    pub fn suite_stopped(status: SuiteStatus) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::SuiteStopped(status),
        }
    }

    pub fn debug_data(client: ClientEnd<DebugDataIteratorMarker>) -> Self {
        Self {
            timestamp: zx::MonotonicInstant::get().into_nanos(),
            payload: SuiteEventPayload::DebugData(client),
        }
    }

    #[cfg(test)]
    pub fn into_suite_run_event(self) -> FidlSuiteEvent {
        self.into()
    }

    #[cfg(test)]
    pub fn into_event(self) -> FidlEvent {
        self.into()
    }

    #[cfg(test)]
    pub fn into_payload(self) -> SuiteEventPayload {
        self.payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn suite_events() {
        let event = SuiteEvents::case_found(1, "case1".to_string()).into_suite_run_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.payload,
            Some(FidlSuiteEventPayload::CaseFound(ftest_manager::CaseFound {
                test_case_name: "case1".into(),
                identifier: 1
            }))
        );

        let event = SuiteEvents::case_started(2).into_suite_run_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.payload,
            Some(FidlSuiteEventPayload::CaseStarted(ftest_manager::CaseStarted { identifier: 2 }))
        );

        let event = SuiteEvents::case_stopped(2, CaseStatus::Failed).into_suite_run_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.payload,
            Some(FidlSuiteEventPayload::CaseStopped(ftest_manager::CaseStopped {
                identifier: 2,
                status: CaseStatus::Failed
            }))
        );

        let event = SuiteEvents::case_finished(2).into_suite_run_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.payload,
            Some(FidlSuiteEventPayload::CaseFinished(ftest_manager::CaseFinished {
                identifier: 2
            }))
        );

        let (sock1, _sock2) = zx::Socket::create_stream();
        let event = SuiteEvents::case_stdout(2, sock1).into_suite_run_event();
        assert_matches!(event.timestamp, Some(_));
        assert_matches!(
            event.payload,
            Some(FidlSuiteEventPayload::CaseArtifact(ftest_manager::CaseArtifact {
                identifier: 2,
                artifact: ftest_manager::Artifact::Stdout(_)
            }))
        );

        let (sock1, _sock2) = zx::Socket::create_stream();
        let event = SuiteEvents::case_stderr(2, sock1).into_suite_run_event();
        assert_matches!(event.timestamp, Some(_));
        assert_matches!(
            event.payload,
            Some(FidlSuiteEventPayload::CaseArtifact(ftest_manager::CaseArtifact {
                identifier: 2,
                artifact: ftest_manager::Artifact::Stderr(_)
            }))
        );

        let event = SuiteEvents::suite_stopped(SuiteStatus::Failed).into_suite_run_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.payload,
            Some(FidlSuiteEventPayload::SuiteStopped(ftest_manager::SuiteStopped {
                status: SuiteStatus::Failed,
            }))
        );

        let (client_end, _server_end) = zx::Socket::create_stream();
        let event = SuiteEvents::suite_syslog(ftest_manager::Syslog::Stream(client_end))
            .into_suite_run_event();
        assert_matches!(event.timestamp, Some(_));
        assert_matches!(
            event.payload,
            Some(FidlSuiteEventPayload::SuiteArtifact(ftest_manager::SuiteArtifact {
                artifact: ftest_manager::Artifact::Log(ftest_manager::Syslog::Stream(_)),
            }))
        );

        let (client_end, _server_end) = fidl::endpoints::create_endpoints();
        let event = SuiteEvents::suite_syslog(ftest_manager::Syslog::Batch(client_end))
            .into_suite_run_event();
        assert_matches!(event.timestamp, Some(_));
        assert_matches!(
            event.payload,
            Some(FidlSuiteEventPayload::SuiteArtifact(ftest_manager::SuiteArtifact {
                artifact: ftest_manager::Artifact::Log(ftest_manager::Syslog::Batch(_)),
            }))
        );

        // New Event FIDL type.

        let event = SuiteEvents::suite_started().into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.details,
            Some(FidlEventDetails::SuiteStarted(ftest_manager::SuiteStartedEventDetails {
                ..Default::default()
            }))
        );

        let event = SuiteEvents::suite_stopped(SuiteStatus::Failed).into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.details,
            Some(FidlEventDetails::SuiteStopped(ftest_manager::SuiteStoppedEventDetails {
                result: Some(SuiteResult::Failed),
                ..Default::default()
            }))
        );

        let event = SuiteEvents::case_found(1, "case1".to_string()).into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.details,
            Some(FidlEventDetails::TestCaseFound(ftest_manager::TestCaseFoundEventDetails {
                test_case_name: Some("case1".into()),
                test_case_id: Some(1),
                ..Default::default()
            }))
        );

        let event = SuiteEvents::case_started(2).into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.details,
            Some(FidlEventDetails::TestCaseStarted(ftest_manager::TestCaseStartedEventDetails {
                test_case_id: Some(2),
                ..Default::default()
            }))
        );

        let (sock1, _sock2) = zx::Socket::create_stream();
        let event = SuiteEvents::case_stdout(2, sock1).into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_matches!(
            event.details,
            Some(FidlEventDetails::TestCaseArtifactGenerated(
                ftest_manager::TestCaseArtifactGeneratedEventDetails {
                    test_case_id: Some(2),
                    artifact: Some(ftest_manager::Artifact::Stdout(_)),
                    ..
                }
            ))
        );

        let (sock1, _sock2) = zx::Socket::create_stream();
        let event = SuiteEvents::case_stderr(2, sock1).into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_matches!(
            event.details,
            Some(FidlEventDetails::TestCaseArtifactGenerated(
                ftest_manager::TestCaseArtifactGeneratedEventDetails {
                    test_case_id: Some(2),
                    artifact: Some(ftest_manager::Artifact::Stderr(_)),
                    ..
                }
            ))
        );

        let event = SuiteEvents::case_stopped(2, CaseStatus::Failed).into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.details,
            Some(FidlEventDetails::TestCaseStopped(ftest_manager::TestCaseStoppedEventDetails {
                test_case_id: Some(2),
                result: Some(TestCaseResult::Failed),
                ..Default::default()
            }))
        );

        let event = SuiteEvents::case_finished(2).into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.details,
            Some(FidlEventDetails::TestCaseFinished(ftest_manager::TestCaseFinishedEventDetails {
                test_case_id: Some(2),
                ..Default::default()
            }))
        );

        let (client_end, _server_end) = zx::Socket::create_stream();
        let event =
            SuiteEvents::suite_syslog(ftest_manager::Syslog::Stream(client_end)).into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_matches!(
            event.details,
            Some(FidlEventDetails::SuiteArtifactGenerated(
                ftest_manager::SuiteArtifactGeneratedEventDetails {
                    artifact: Some(ftest_manager::Artifact::Log(ftest_manager::Syslog::Stream(_))),
                    ..
                }
            ))
        );

        let (client_end, _server_end) = fidl::endpoints::create_endpoints();
        let event =
            SuiteEvents::suite_syslog(ftest_manager::Syslog::Batch(client_end)).into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_matches!(
            event.details,
            Some(FidlEventDetails::SuiteArtifactGenerated(
                ftest_manager::SuiteArtifactGeneratedEventDetails {
                    artifact: Some(ftest_manager::Artifact::Log(ftest_manager::Syslog::Batch(_))),
                    ..
                }
            ))
        );

        let event = SuiteEvents::suite_stopped(SuiteStatus::Failed).into_event();
        assert_matches!(event.timestamp, Some(_));
        assert_eq!(
            event.details,
            Some(FidlEventDetails::SuiteStopped(ftest_manager::SuiteStoppedEventDetails {
                result: Some(SuiteResult::Failed),
                ..Default::default()
            }))
        );
    }
}
