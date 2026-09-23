// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ATraceEvent<'a> {
    Begin { pid: u64, name: &'a str },
    End { pid: u64 },
    Instant { pid: u64, name: &'a str },
    AsyncBegin { pid: u64, name: &'a str, correlation_id: u64 },
    AsyncEnd { pid: u64, name: &'a str, correlation_id: u64 },
    Counter { pid: u64, name: &'a str, value: i64 },
    AsyncTrackBegin { pid: u64, track_name: &'a str, name: &'a str, cookie: i32 },
    AsyncTrackEnd { pid: u64, track_name: &'a str, cookie: i32 },
    Track { pid: u64, track_name: &'a str, name: &'a str },
}

impl<'a> ATraceEvent<'a> {
    // Arbitrary data is allowed to be written to tracefs, and we only care about identifying ATrace
    // events to forward to Fuchsia tracing. Since we would throw away any detailed parsing error, this
    // function returns an Option rather than a Result. If we did return a Result, this could be
    // put in a TryFrom impl, if desired.
    pub(crate) fn parse(s: &'a str) -> Option<Self> {
        let mut chunks = s.trim().split('|');
        let event_type = chunks.next()?;

        // event_type matches the systrace phase. See systrace_parser.h in perfetto.
        match event_type {
            "B" => {
                let pid = chunks.next()?.parse::<u64>().ok()?;
                // It is ok to have an unnamed begin event, so insert a default name.
                let name = chunks.next().unwrap_or("[empty name]");
                Some(ATraceEvent::Begin { pid, name })
            }
            "E" => {
                // End thread scoped event. Since it is thread scoped, it is OK to not have the TGID
                // not present.
                let pid = chunks.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
                Some(ATraceEvent::End { pid })
            }
            "I" => {
                let pid = chunks.next()?.parse::<u64>().ok()?;
                let name = chunks.next()?;
                Some(ATraceEvent::Instant { pid, name })
            }
            "S" => {
                let pid = chunks.next()?.parse::<u64>().ok()?;
                let name = chunks.next()?;
                let correlation_id = chunks.next()?.parse::<u64>().ok()?;
                Some(ATraceEvent::AsyncBegin { pid, name, correlation_id })
            }
            "F" => {
                let pid = chunks.next()?.parse::<u64>().ok()?;
                let name = chunks.next()?;
                let correlation_id = chunks.next()?.parse::<u64>().ok()?;
                Some(ATraceEvent::AsyncEnd { pid, name, correlation_id })
            }
            "C" => {
                let pid = chunks.next()?.parse::<u64>().ok()?;
                let name = chunks.next()?;
                let value = chunks.next()?.parse::<i64>().ok()?;
                Some(ATraceEvent::Counter { pid, name, value })
            }
            "G" => {
                let pid = chunks.next()?.parse::<u64>().ok()?;
                let track_name = chunks.next()?;
                let name = chunks.next()?;
                let cookie = chunks.next()?.parse::<i32>().ok()?;
                Some(ATraceEvent::AsyncTrackBegin { pid, track_name, name, cookie })
            }
            "H" => {
                let pid = chunks.next()?.parse::<u64>().ok()?;
                let track_name = chunks.next()?;
                let cookie = chunks.next()?.parse::<i32>().ok()?;
                Some(ATraceEvent::AsyncTrackEnd { pid, track_name, cookie })
            }
            "N" => {
                let pid = chunks.next()?.parse::<u64>().ok()?;
                let track_name = chunks.next()?;
                let name = chunks.next()?;
                Some(ATraceEvent::Track { pid, track_name, name })
            }
            _ => None,
        }
    }

    pub(crate) fn data(&self) -> String {
        match self {
            ATraceEvent::Begin { pid, name } => format!("B|{pid}|{name}\n"),
            ATraceEvent::End { pid } => {
                if *pid != 0 {
                    format!("E|{pid}\n")
                } else {
                    "E|\n".into()
                }
            }
            ATraceEvent::Instant { pid, name } => format!("I|{pid}|{name}\n"),
            ATraceEvent::AsyncBegin { pid, name, correlation_id } => {
                format!("S|{pid}|{name}|{correlation_id}\n")
            }
            ATraceEvent::AsyncEnd { pid, name, correlation_id } => {
                format!("F|{pid}|{name}|{correlation_id}\n")
            }
            ATraceEvent::Counter { pid, name, value } => format!("C|{pid}|{name}|{value}\n"),
            ATraceEvent::AsyncTrackBegin { pid, track_name, name, cookie } => {
                format!("G|{pid}|{track_name}|{name}|{cookie}\n")
            }
            ATraceEvent::AsyncTrackEnd { pid, track_name, cookie } => {
                format!("H|{pid}|{track_name}|{cookie}\n")
            }
            ATraceEvent::Track { pid, track_name, name } => {
                format!("N|{pid}|{track_name}|{name}\n")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn atrace_event_parsing() {
        assert_eq!(
            ATraceEvent::parse("B|1636|slice_name"),
            Some(ATraceEvent::Begin { pid: 1636, name: "slice_name" }),
        );

        let no_name_event = ATraceEvent::parse("B|1166");
        match no_name_event {
            Some(ATraceEvent::Begin { pid: 1166, .. }) => (),
            _ => panic!("Unexpected parsing result: {no_name_event:?} from \"B|1166\""),
        };

        assert_eq!(ATraceEvent::parse("E|1636"), Some(ATraceEvent::End { pid: 1636 }),);
        assert_eq!(
            ATraceEvent::parse("I|1636|instant_name"),
            Some(ATraceEvent::Instant { pid: 1636, name: "instant_name" }),
        );

        assert_eq!(ATraceEvent::parse("E|"), Some(ATraceEvent::End { pid: 0 }));
        assert_eq!(ATraceEvent::parse("E"), Some(ATraceEvent::End { pid: 0 }));

        assert_eq!(
            ATraceEvent::parse("S|1636|async_name|123"),
            Some(ATraceEvent::AsyncBegin { pid: 1636, name: "async_name", correlation_id: 123 }),
        );
        assert_eq!(
            ATraceEvent::parse("F|1636|async_name|123"),
            Some(ATraceEvent::AsyncEnd { pid: 1636, name: "async_name", correlation_id: 123 }),
        );
        assert_eq!(
            ATraceEvent::parse("C|1636|counter_name|123"),
            Some(ATraceEvent::Counter { pid: 1636, name: "counter_name", value: 123 }),
        );
        assert_eq!(
            ATraceEvent::parse("G|1636|a track|async_name|123"),
            Some(ATraceEvent::AsyncTrackBegin {
                pid: 1636,
                track_name: "a track",
                name: "async_name",
                cookie: 123
            }),
        );
        assert_eq!(
            ATraceEvent::parse("H|1636|a track|123"),
            Some(ATraceEvent::AsyncTrackEnd { pid: 1636, track_name: "a track", cookie: 123 }),
        );
        assert_eq!(
            ATraceEvent::parse("N|1636|a track|instant_name"),
            Some(ATraceEvent::Track { pid: 1636, track_name: "a track", name: "instant_name" }),
        );
    }
}
