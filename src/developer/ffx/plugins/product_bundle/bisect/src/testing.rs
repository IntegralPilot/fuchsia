// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use std::io::{self, Write};
use std::process::Command;

/// Run an automated test script and return the results.
pub async fn run_automated_test(
    script: &Utf8PathBuf,
    pb_path: &Utf8PathBuf,
    mut print_fn: impl FnMut(&str),
) -> Result<bool> {
    print_fn("");
    print_fn(&format!("Running automated validation script: {}", script));
    print_fn(&format!("  --pb {}", pb_path));
    print_fn("-----------");

    let mut child = Command::new(script)
        .arg("--pb")
        .arg(pb_path)
        .spawn()
        .with_context(|| format!("Failed to execute validation script: {}", script))?;

    let status = child.wait().with_context(|| "Failed to wait on validation script")?;

    print_fn("-----------");
    if let Some(code) = status.code() {
        match code {
            0 => {
                print_fn("Script returned 0 (Pass).");
                std::thread::sleep(std::time::Duration::from_secs(2));
                Ok(true)
            }
            1..=127 => {
                print_fn(&format!("Script returned {} (Fail).", code));
                std::thread::sleep(std::time::Duration::from_secs(2));
                Ok(false)
            }
            _ => {
                anyhow::bail!(
                    "Script returned {} (Infrastructure failure). A critical infrastructure failure occurred.",
                    code
                )
            }
        }
    } else {
        anyhow::bail!("Validation script was terminated by a signal.")
    }
}

/// Ask the user to run a test with the given fuchsia image,
/// and return the results.
pub fn prompt_for_manual_test(
    product_bundle_path: &Utf8PathBuf,
    mut print_fn: impl FnMut(&str),
) -> Result<bool> {
    print_fn("");
    let shortened_pb_path = std::env::home_dir()
        .and_then(|home| camino::Utf8PathBuf::from_path_buf(home).ok())
        .and_then(|home_utf8| product_bundle_path.strip_prefix(&home_utf8).ok())
        .map(|stripped| format!("~/{}", stripped))
        .unwrap_or_else(|| product_bundle_path.to_string());

    print_fn("Flash this pb to a local device by opening another terminal window and running:\n");
    print_fn(&format!(
        "  ffx target flash \\\n    --no-bootloader-reboot \\\n    --skip-verify \\\n    --skip-authorized-keys \\\n    -b {}\n",
        shortened_pb_path
    ));
    print_fn("Then run a test to determine whether or not the original issue remains.");
    print_fn(
        "Press Ctrl+C to cancel, and resume by repeating the same 'ffx product-bundle bisect ...' command.",
    );
    print_fn("-----");

    loop {
        let _ = io::stdout().write_all(b"\nDoes the test pass with this image? (y/n) ");
        let _ = io::stdout().flush();

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        match input.trim().to_lowercase().as_str() {
            "y" | "yes" | "pass" => return Ok(true),
            "n" | "no" | "fail" => return Ok(false),
            _ => {
                print_fn("Invalid input. Please enter 'y', 'yes', 'pass', or 'n', 'no', 'fail'.");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_automated_test_pass() {
        futures_lite::future::block_on(async move {
            let script = Utf8PathBuf::from(
                "../../src/developer/ffx/plugins/product_bundle/bisect/test_data/test_pass.sh",
            );
            let pb_path = Utf8PathBuf::from("/fake/pb");

            let result = run_automated_test(&script, &pb_path, |_| {}).await;
            assert!(result.is_ok());
            assert!(result.unwrap());
        });
    }

    #[test]
    fn test_run_automated_test_fail() {
        futures_lite::future::block_on(async move {
            let script = Utf8PathBuf::from(
                "../../src/developer/ffx/plugins/product_bundle/bisect/test_data/test_fail.sh",
            );
            let pb_path = Utf8PathBuf::from("/fake/pb");

            let result = run_automated_test(&script, &pb_path, |_| {}).await;
            assert!(result.is_ok());
            assert!(!result.unwrap());
        });
    }

    #[test]
    fn test_run_automated_test_infra_failure_code_128() {
        futures_lite::future::block_on(async move {
            let script = Utf8PathBuf::from(
                "../../src/developer/ffx/plugins/product_bundle/bisect/test_data/test_infra.sh",
            );
            let pb_path = Utf8PathBuf::from("/fake/pb");

            let result = run_automated_test(&script, &pb_path, |_| {}).await;
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("128 (Infrastructure failure)"));
        });
    }
}
