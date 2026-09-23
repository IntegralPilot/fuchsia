// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::LazyLock;

static AGENTS_ENV_VARS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    include_str!("../../../../tools/devshell/lib/agents.txt")
        .lines()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect()
});

pub trait EnvironmentSource {
    fn has_var(&self, key: &str) -> bool;
}

pub struct SystemEnvironment;
impl EnvironmentSource for SystemEnvironment {
    fn has_var(&self, key: &str) -> bool {
        std::env::var_os(key).is_some()
    }
}

/// Returns true if the current process appears to be invoked by an AI agent.
pub fn is_invoked_by_agent<E: EnvironmentSource>(env: &E) -> bool {
    AGENTS_ENV_VARS.iter().any(|&name| env.has_var(name))
}

/// Returns true if the current process appears to be invoked by an AI agent, using the system environment.
pub fn is_agent_env() -> bool {
    is_invoked_by_agent(&SystemEnvironment)
}

impl<V> EnvironmentSource for std::collections::HashMap<String, V> {
    fn has_var(&self, key: &str) -> bool {
        self.contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeEnv {
        vars: HashMap<String, String>,
    }

    impl EnvironmentSource for FakeEnv {
        fn has_var(&self, key: &str) -> bool {
            self.vars.contains_key(key)
        }
    }

    #[test]
    fn test_is_invoked_by_agent() {
        let vars = HashMap::new();
        let env = FakeEnv { vars };
        assert!(!is_invoked_by_agent(&env));

        let mut vars = HashMap::new();
        vars.insert("ANTIGRAVITY_AGENT".into(), "1".into());
        let env = FakeEnv { vars };
        assert!(is_invoked_by_agent(&env));

        let mut vars = HashMap::new();
        vars.insert("GEMINI_CLI".into(), "1".into());
        let env = FakeEnv { vars };
        assert!(is_invoked_by_agent(&env));

        let mut vars = HashMap::new();
        vars.insert("ANTIGRAVITY_EDITOR_APP_ROOT".into(), "1".into());
        let env = FakeEnv { vars };
        assert!(is_invoked_by_agent(&env));
    }
}
