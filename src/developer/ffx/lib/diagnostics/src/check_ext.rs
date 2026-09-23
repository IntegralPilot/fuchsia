// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Check, CheckFut};
use futures::future::{TryFuture, TryFutureExt};
use std::marker::Unpin;

/// A trait for adding some convenience methods to chain together checks with each other.
pub trait CheckExt<'a, C, T: 'a>:
    TryFuture<Ok = (T, &'a mut <C as Check>::Notifier), Error = anyhow::Error>
where
    C: Check<Input = T> + Unpin + 'a,
    C::Notifier: Sized + 'a,
    Self: 'a,
{
    fn and_then_check(self, next: C) -> CheckFut<'a, (C::Output, &'a mut C::Notifier)>
    where
        C::Output: 'a,
        Self: Sized + 'a,
        Self::Ok: 'a,
    {
        Box::pin(self.and_then(move |(out, notifier)| next.check_with_notifier(out, notifier)))
    }
}

impl<'a, T: 'a, C: Check<Input = T> + Unpin + 'a> CheckExt<'a, C, T>
    for CheckFut<'a, (T, &'a mut C::Notifier)>
where
    C::Notifier: Sized + 'a,
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NotificationType, Notifier};
    use std::io::Write;

    #[derive(Default, Debug)]
    struct DefaultNotifier {
        output: Vec<u8>,
    }

    impl Notifier for DefaultNotifier {
        fn update_status(
            &mut self,
            _ty: NotificationType,
            status: impl Into<String>,
        ) -> anyhow::Result<()> {
            writeln!(&mut self.output, "{}", status.into()).map_err(Into::into)
        }
    }

    struct First;

    struct Second;

    struct Third;

    impl Check for First {
        type Input = u32;
        type Output = u64;
        type Notifier = DefaultNotifier;

        fn write_preamble(
            &self,
            input: &Self::Input,
            notifier: &mut Self::Notifier,
        ) -> anyhow::Result<()> {
            notifier.update_status(
                NotificationType::Info,
                format!("First check, looking at input: {input}"),
            )
        }

        fn check<'a>(
            &'a mut self,
            input: Self::Input,
            _notifier: &'a mut Self::Notifier,
        ) -> CheckFut<'a, Self::Output> {
            assert_eq!(input, 2);
            Box::pin(std::future::ready(Ok(25)))
        }
    }

    impl Check for Second {
        type Input = u64;
        type Output = String;
        type Notifier = DefaultNotifier;

        fn write_preamble(
            &self,
            input: &Self::Input,
            notifier: &mut Self::Notifier,
        ) -> anyhow::Result<()> {
            notifier.update_status(
                NotificationType::Info,
                format!("Second check, looking at input: {input}"),
            )
        }

        fn check<'a>(
            &'a mut self,
            input: Self::Input,
            _notifier: &'a mut Self::Notifier,
        ) -> CheckFut<'a, Self::Output> {
            assert_eq!(input, 25);
            Box::pin(std::future::ready(Ok("foobar".to_string())))
        }
    }

    impl Check for Third {
        type Input = String;
        type Output = u8;
        type Notifier = DefaultNotifier;

        fn write_preamble(
            &self,
            input: &Self::Input,
            notifier: &mut Self::Notifier,
        ) -> anyhow::Result<()> {
            notifier.update_status(
                NotificationType::Info,
                format!("Third check, looking at input: {input}"),
            )
        }

        fn check<'a>(
            &'a mut self,
            input: Self::Input,
            _notifier: &'a mut Self::Notifier,
        ) -> CheckFut<'a, Self::Output> {
            assert_eq!(&input, "foobar");
            Box::pin(std::future::ready(Ok(5)))
        }
    }

    #[fuchsia::test]
    async fn test_combinator() {
        let chain = First;
        let mut output = DefaultNotifier::default();
        let res = chain
            .check_with_notifier(2, &mut output)
            .and_then_check(Second)
            .and_then_check(Third)
            .await
            .unwrap();
        assert_eq!(res.0, 5);
        assert_eq!(
            "First check, looking at input: 2\nSecond check, looking at input: 25\nThird check, looking at input: foobar\n",
            String::from_utf8(output.output).unwrap()
        );
    }

    struct FailingCheck;

    impl Check for FailingCheck {
        type Input = String;
        type Output = String;
        type Notifier = DefaultNotifier;

        fn write_preamble(
            &self,
            input: &Self::Input,
            notifier: &mut Self::Notifier,
        ) -> anyhow::Result<()> {
            notifier.update_status(
                NotificationType::Info,
                format!("About to do a failing check, looking at input: {input}"),
            )
        }

        fn check<'a>(
            &'a mut self,
            _input: Self::Input,
            _notifier: &'a mut Self::Notifier,
        ) -> CheckFut<'a, Self::Output> {
            Box::pin(std::future::ready(Err(anyhow::anyhow!("bad things happened"))))
        }
    }

    #[fuchsia::test]
    async fn test_combinator_fails() {
        let chain = First;
        let mut output = DefaultNotifier::default();
        let res = chain
            .check_with_notifier(2, &mut output)
            .and_then_check(Second)
            .and_then_check(FailingCheck)
            .and_then_check(Third)
            .await;
        assert!(res.unwrap_err().to_string().contains("bad things happened"));
        assert_eq!(
            "First check, looking at input: 2\nSecond check, looking at input: 25\nAbout to do a failing check, looking at input: foobar\n",
            String::from_utf8(output.output).unwrap()
        );
    }
}
