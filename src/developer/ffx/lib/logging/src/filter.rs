// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub trait Filter {
    fn should_emit(&self, _record: &log::Metadata<'_>) -> bool {
        true
    }
}

impl Filter for log::LevelFilter {
    fn should_emit(&self, metadata: &log::Metadata<'_>) -> bool {
        match self.to_level() {
            None => false,
            Some(level) => {
                if level >= metadata.level() {
                    true
                } else {
                    false
                }
            }
        }
    }
}

pub struct TargetsFilter {
    targets: Vec<(String, log::LevelFilter)>,
}

impl TargetsFilter {
    pub fn new(targets: Vec<(String, log::LevelFilter)>) -> Self {
        Self { targets }
    }
}

impl Filter for TargetsFilter {
    fn should_emit(&self, metadata: &log::Metadata<'_>) -> bool {
        if self.targets.len() == 0 {
            true
        } else {
            for (target, level) in &self.targets {
                if metadata.target().starts_with(target) && *level >= metadata.level() {
                    return true;
                }
            }
            false
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use log::Level;

    //////////////////////////////////////////////////////////////////////
    /// TargetsFilter
    ///

    #[test]
    fn test_targetsfilter_should_emit_on_empty() {
        let targets = TargetsFilter::new(vec![]);
        let metadata = log::Metadata::builder().level(Level::Warn).target("coronabeth").build();
        assert!(targets.should_emit(&metadata));
    }

    #[test]
    fn test_targetsfilter_should_emit_on_prefix() {
        let targets = TargetsFilter::new(vec![("cor".to_string(), log::LevelFilter::Debug)]);
        let metadata = log::Metadata::builder().level(Level::Warn).target("coronabeth").build();
        assert!(targets.should_emit(&metadata));
    }

    #[test]
    fn test_targetsfilter_should_not_emit_on_prefix_mismatch() {
        let targets = TargetsFilter::new(vec![("ianthe".to_string(), log::LevelFilter::Debug)]);
        let metadata = log::Metadata::builder().level(Level::Warn).target("coronabeth").build();
        assert!(!targets.should_emit(&metadata));
    }

    #[test]
    fn test_targetsfilter_should_not_emit_on_level() {
        let targets = TargetsFilter::new(vec![("coro".to_string(), log::LevelFilter::Debug)]);
        let metadata = log::Metadata::builder().level(Level::Trace).target("coronabeth").build();
        assert!(!targets.should_emit(&metadata));
    }

    //////////////////////////////////////////////////////////////////////
    /// LevelFilter
    ///

    #[test]
    fn test_levelfilter_should_emit() {
        let level = log::LevelFilter::Debug;
        let metadata = log::Metadata::builder().level(Level::Warn).build();
        assert!(level.should_emit(&metadata));
    }
    #[test]
    fn test_levelfilter_shouldnot_emit() {
        let level = log::LevelFilter::Error;
        let metadata = log::Metadata::builder().level(Level::Warn).build();
        assert!(!level.should_emit(&metadata));
    }
}
