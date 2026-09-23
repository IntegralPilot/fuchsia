// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use errors::ffx_bail;
use ffx_config::EnvironmentContext;
use ffx_config::logging::{change_log_file, reset_log_file};
use fuchsia_async::TimeoutExt;
use serde_json::Value;
use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;
use termion::is_tty;

pub mod asserts;

// Set a config value
// (Note that due to interior mutability, the isolate will
// be changed even though it's not &mut)
async fn set_value_in_isolate(
    isolate: &ffx_isolate::Isolate,
    config_key: &str,
    value: Value,
) -> Result<()> {
    let ctx = isolate.env_context();
    let env = ctx.load()?;
    let mut config = ffx_config::Config::from_env(&env)?;
    config.set(config_key, ffx_config::ConfigLevel::User, value)?;
    config.save()?;
    Ok(())
}

fn subtest_log_file(isolate: &ffx_isolate::Isolate) -> PathBuf {
    let mut dir = isolate.log_dir().to_path_buf();
    dir.push("self_test.log");
    dir
}

/// Create a new ffx isolate. This method relies on the environment provided by
/// the ffx binary and should only be called within ffx.
pub async fn new_isolate(env: &EnvironmentContext, name: &str) -> Result<ffx_isolate::Isolate> {
    let ssh_key = env.get::<String, _>("ssh.priv")?.into();
    let isolate = ffx_isolate::Isolate::new_with_sdk(name, ssh_key, &env).await?;
    set_value_in_isolate(&isolate, "watchdogs.host_pipe.enabled", true.into()).await?;
    set_value_in_isolate(&isolate, "ffx.isolated", true.into()).await?;
    set_value_in_isolate(&isolate, "fastboot.network.tcp.retry_count", 0.into()).await?;
    // Globally change the log file to one appropriate to the isolate.  We'll reset it after
    // the test completes
    change_log_file(&subtest_log_file(&isolate))?;
    Ok(isolate)
}

// For tests, we use the target address rather than node name to be in sync
// with isolates, which also use FUCHSIA_DEVICE_ADDR.
pub fn get_target_addr() -> String {
    // We check for this in src/lib.rs when it is invoked
    std::env::var("FUCHSIA_DEVICE_ADDR").expect("FUCHSIA_DEVICE_ADDR unset")
}

/// run runs the given set of tests printing results to stdout and exiting
/// with 0 or 1 if the tests passed or failed, respectively.
pub async fn run(
    context: &EnvironmentContext,
    tests: Vec<TestCase>,
    timeout: Duration,
    case_timeout: Duration,
) -> Result<()> {
    let mut writer = std::io::stdout();
    let color = is_tty(&writer);
    let green = green(color);
    let red = red(color);
    let nocol = nocol(color);

    let test_result = async {
        let num_tests = tests.len();

        writeln!(&mut writer, "1..{}", num_tests)?;

        let mut num_errs: usize = 0;
        for (i, tc) in tests.iter().enumerate().map(|(i, tc)| (i + 1, tc)) {
            write!(&mut writer, "{nocol}{i}. {name} - ", name = tc.name)?;
            writer.flush()?;
            match (tc.f)(context.clone())
                .on_timeout(case_timeout, || ffx_bail!("timed out after {:?}", case_timeout))
                .await
            {
                Ok(_) => {
                    writeln!(&mut writer, "{green}ok{nocol}",)?;
                }
                Err(err) => {
                    // Unfortunately we didn't get a chance to get back any
                    // Isolate the test may have used, which means we can't
                    // clean up the daemon. The hope is that this is not a big
                    // deal -- when the Isolate drops(), it will remove the
                    // daemon.socket file which should cause the daemon to exit
                    // anyway -- this code was just a backstop to improve our
                    // chances of having it exit. Plus, the failure path is when
                    // we error out, which is the unusual case (and honestly,
                    // if we can't pass our self-test, there are no guarantees
                    // about the behavior anyway.)
                    writeln!(&mut writer, "{red}not ok{nocol}:\n{err:?}\n",)?;
                    num_errs = num_errs + 1;
                }
            }
            reset_log_file()?;
        }

        if num_errs != 0 {
            ffx_bail!("{red}{num_errs}/{num_tests} failed{nocol}");
        } else {
            writeln!(&mut writer, "{green}{num_tests}/{num_tests} passed{nocol}",)?;
        }

        Ok(())
    }
    .on_timeout(timeout, || ffx_bail!("timed out after {:?}", timeout))
    .await;

    test_result
}

fn green(color: bool) -> &'static str {
    if color { termion::color::Green.fg_str() } else { "" }
}
fn red(color: bool) -> &'static str {
    if color { termion::color::Red.fg_str() } else { "" }
}
fn nocol(color: bool) -> &'static str {
    if color { termion::color::Reset.fg_str() } else { "" }
}

#[macro_export]
macro_rules! tests {
    ( $( $x:expr ),* $(,)* ) => {
        {
            vec![$(
                // We need to store a boxed, pinned future, so let's provide the closure that
                // does that.
                $crate::test::TestCase::new(stringify!($x), move |ctx| { Box::pin($x(ctx))}),
            )*]
        }
    };
}

// We need to store a boxed pinned future: boxed because we need it to be Sized since it's going
// in a Vec; pinned because it's asynchronous.
pub type TestFn = fn(EnvironmentContext) -> Pin<Box<dyn Future<Output = Result<()>>>>;

pub struct TestCase {
    pub name: &'static str,
    f: TestFn,
}

impl TestCase {
    pub fn new(name: &'static str, f: TestFn) -> Self {
        Self { name, f }
    }
}
