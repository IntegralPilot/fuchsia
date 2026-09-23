// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use discovery::query::TargetInfoQuery;
use ffx_config::EnvironmentContext;
use ffx_diagnostics::{Check, CheckFut, Notifier};
use std::marker::PhantomData;
use termio::Colors;

pub struct GetTargetSpecifier<'a, N>(pub(crate) &'a EnvironmentContext, pub(crate) PhantomData<N>);

impl<'a, N> GetTargetSpecifier<'a, N> {
    pub fn new(ctx: &'a EnvironmentContext) -> Self {
        Self(ctx, Default::default())
    }
}

impl<N> Check for GetTargetSpecifier<'_, N>
where
    N: Notifier + Sized,
{
    type Input = ();
    type Output = TargetInfoQuery;
    type Notifier = N;

    fn write_preamble(
        &self,
        _input: &Self::Input,
        notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        notifier.info("Getting target specifier from config... ")
    }

    fn on_success(
        &self,
        output: &Self::Output,
        notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        let ffx_diagnostics_formatting::ReadableQuery { kind, value } =
            ffx_diagnostics_formatting::format_query(output);
        if value.is_empty() {
            notifier.on_success(format!("The target specifier is {kind}"))
        } else {
            let colors = Colors::current();
            let safe_value = safe_string::TermSafe::from_str_escaped(&value);
            notifier.on_success(format!(
                "The target specifier is {kind} and is \"{}{}{}\"",
                colors.green, safe_value, colors.reset
            ))
        }
    }

    fn check<'a>(
        &'a mut self,
        _input: Self::Input,
        _notifier: &'a mut Self::Notifier,
    ) -> CheckFut<'a, Self::Output> {
        Box::pin(std::future::ready(
            ffx_target::get_target_specifier(self.0)
                .and_then(|opt_s| TargetInfoQuery::try_from(opt_s).map_err(anyhow::Error::from)),
        ))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    async fn test_target_identifier() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::TARGET_DEFAULT_KEY, "foobar")
            .build()
            .expect("initializing config");
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let (target, _) = GetTargetSpecifier::new(&env.context)
            .check_with_notifier((), &mut notifier)
            .await
            .expect("running checks");
        if let TargetInfoQuery::NodenameOrId(n) = target {
            assert_eq!(n, "foobar");
        } else {
            panic!("Unexpected target: {target:?}")
        };
    }

    #[fuchsia::test]
    async fn test_target_identifier_empty() {
        let env = ffx_config::test_env().build().expect("initializing config");
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let (target, _) = GetTargetSpecifier::new(&env.context)
            .check_with_notifier((), &mut notifier)
            .await
            .expect("running checks");
        assert!(matches!(target, TargetInfoQuery::First));
    }

    #[fuchsia::test]
    async fn test_target_identifier_escapes_control_characters() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::TARGET_DEFAULT_KEY, "target\x1b[31m_evil\t\0")
            .build()
            .expect("initializing config");
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let (target, _) = GetTargetSpecifier::new(&env.context)
            .check_with_notifier((), &mut notifier)
            .await
            .expect("running checks");
        if let TargetInfoQuery::NodenameOrId(n) = target {
            assert_eq!(n, "target\x1b[31m_evil\t\0");
        } else {
            panic!("Unexpected target: {target:?}")
        };

        let output: String = notifier.into();
        assert!(!output.contains('\x1b'));
        assert!(!output.contains('\t'));
        assert!(!output.contains('\0'));
        assert!(output.contains("target\\u{1b}[31m_evil\\t\\u{0}"));
    }
}
