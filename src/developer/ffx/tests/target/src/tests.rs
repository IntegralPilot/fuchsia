// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_isolate::Isolate;
use ffx_testing::{Emu, TestContext, emulator_fixture};
use fixture::fixture;
use fuchsia_async as _;
use futures::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use futures::{AsyncRead, AsyncWrite, AsyncWriteExt, Stream, StreamExt};
use netext::TokioAsyncReadExt;
use std::time::Duration;

/// Test `ffx target flash` by bringing up an emulator in fastboot.
///
/// The fastboot implementation used by this emulator is Gigaboot (//src/firmware/gigaboot).
#[fixture(emulator_fixture)]
#[fuchsia::test]
#[ignore] // TODO(https://fxbug.dev/387543611): Re-enable
async fn test_target_flash_gigaboot(ctx: TestContext) {
    let isolate = ctx.isolate();

    let mut emu = Emu::start(&ctx);

    {
        let serial = emu.serial().await;
        let (mut serial_lines, mut serial_writer) =
            serial_lines(serial.into_futures_stream()).await;

        // On initial boot, press `f` to enter fastboot.
        enter_fastboot(&mut serial_lines, &mut serial_writer).await;
    }

    std::thread::sleep(Duration::from_secs(5));

    emu.check_is_running().expect("Emulator exited unexpectedly");

    let product_bundle_json = Emu::product_bundle_dir();

    // Flash in fastboot, then verify that target boots to product.
    let output = isolate
        .ffx(&[
            "--target",
            emu.nodename(),
            "target",
            "flash",
            "-b",
            product_bundle_json.to_str().unwrap(),
        ])
        .await
        .expect("flash target");
    assert!(output.status.success(), "Failed to run command: {}\n{}", output.stdout, output.stderr);

    emu.check_is_running().expect("Emulator exited unexpectedly");

    // Retry ffx target show as it may take the device up to 30 seconds to initialize SSH.
    wait_for_target!(isolate, emu, 0);
    emu.check_is_running().expect("Emulator exited unexpectedly");
}

/// Test `ffx target flash` by bringing up an emulator in fastboot.
///
/// The fastboot implementation used by this emulator is Gigaboot (//src/firmware/gigaboot).
///
/// This also re-flashes the image once it is in product mode
#[fixture(emulator_fixture)]
#[fuchsia::test]
#[ignore] // TODO(https://fxbug.dev/387543611, https://fxbug.dev/42083467): Re-enable
async fn test_target_flash_from_product(ctx: TestContext) {
    let isolate = ctx.isolate();

    let mut emu = Emu::start(&ctx);

    {
        let serial = emu.serial().await;
        let (mut serial_lines, mut serial_writer) =
            serial_lines(serial.into_futures_stream()).await;

        // On initial boot, press `f` to enter fastboot.
        enter_fastboot(&mut serial_lines, &mut serial_writer).await;
    }

    std::thread::sleep(Duration::from_secs(5));

    emu.check_is_running().expect("Emulator exited unexpectedly");

    let product_bundle_json = Emu::product_bundle_dir();

    // Flash in fastboot, then verify that target boots to product.
    let output = isolate
        .ffx(&[
            "--target",
            emu.nodename(),
            "target",
            "flash",
            "-b",
            product_bundle_json.to_str().unwrap(),
        ])
        .await
        .expect("flash target");
    assert!(output.status.success(), "Failed to run command: {}\n{}", output.stdout, output.stderr);

    emu.check_is_running().expect("Emulator exited unexpectedly");

    // Retry ffx target show as it may take the device up to 30 seconds to initialize SSH.
    wait_for_target!(isolate, emu, 0);
    emu.check_is_running().expect("Emulator exited unexpectedly");

    // At this point the target is in Product mode... flash again and verify

    let output = isolate
        .ffx(&[
            "--target",
            emu.nodename(),
            "target",
            "flash",
            "-b",
            product_bundle_json.to_str().unwrap(),
        ])
        .await
        .expect("flash target");
    assert!(output.status.success(), "Failed to run command: {}\n{}", output.stdout, output.stderr);

    emu.check_is_running().expect("Emulator exited unexpectedly");

    // Retry ffx target show as it may take the device up to 30 seconds to initialize SSH.
    wait_for_target!(isolate, emu, 0);
    emu.check_is_running().expect("Emulator exited unexpectedly");
}

#[fixture(emulator_fixture)]
#[fuchsia::test]
#[ignore] // TODO(https://fxbug.dev/387543611): Re-enable
async fn test_target_reboot_to_bootloader_gigaboot(ctx: TestContext) {
    let isolate = ctx.isolate();

    let mut emu = Emu::start(&ctx);

    {
        let serial = emu.serial().await;
        let (mut serial_lines, mut serial_writer) =
            serial_lines(serial.into_futures_stream()).await;

        // On initial boot, press `f` to enter fastboot.
        enter_fastboot(&mut serial_lines, &mut serial_writer).await;
    }

    std::thread::sleep(Duration::from_secs(5));

    emu.check_is_running().expect("Emulator exited unexpectedly");

    let output = isolate
        .ffx(&["--target", emu.nodename(), "target", "reboot", "-b"])
        .await
        .expect("reboot to bootloader");
    assert!(output.status.success(), "Failed to run command: {}\n{}", output.stdout, output.stderr);

    // ffx waits for the bootloader to re-enter fastboot before returning.
    emu.check_is_running().expect("Emulator exited unexpectedly");

    let out = isolate
        .ffx(&["--target", emu.nodename(), "target", "list", emu.nodename()])
        .await
        .expect("target show");

    assert!(out.status.success(), "status is unexpected: {:?}", out);
    assert!(out.stdout.contains("Fastboot"), "stdout is unexpected: {:?}", out);
    assert!(out.stderr.lines().count() == 0, "stderr is unexpected: {:?}", out);
    emu.check_is_running().expect("Emulator exited unexpectedly");
}

#[fixture(emulator_fixture)]
#[fuchsia::test]
#[ignore] // TODO(https://fxbug.dev/387543611): Re-enable
async fn test_target_bootloader_info(ctx: TestContext) {
    let isolate = ctx.isolate();

    let mut emu = Emu::start(&ctx);

    {
        let serial = emu.serial().await;
        let (mut serial_lines, mut serial_writer) =
            serial_lines(serial.into_futures_stream()).await;

        // On initial boot, press `f` to enter fastboot.
        enter_fastboot(&mut serial_lines, &mut serial_writer).await;
    }

    std::thread::sleep(Duration::from_secs(5));

    let out = isolate
        .ffx(&["--target", emu.nodename(), "target", "bootloader", "info"])
        .await
        .expect("get bootloader info");

    assert!(out.status.success(), "status is unexpected: {:?}", out.status);
    assert!(out.stdout.contains("\nversion: 0.4\n"), "stdout is unexpected: {:?}", out.stdout);
    assert!(out.stderr.lines().count() == 0, "stderr is unexpected: {:?}", out.stdout);
    emu.check_is_running().expect("Emulator exited unexpectedly");
}

#[fixture(emulator_fixture)]
#[fuchsia::test]
#[ignore] // TODO(https://fxbug.dev/387543611, https://fxbug.dev/42083467): Re-enable
async fn test_target_bootloader_info_from_product(ctx: TestContext) {
    let isolate = ctx.isolate();

    let mut emu = Emu::start(&ctx);

    {
        let serial = emu.serial().await;
        let (mut serial_lines, mut serial_writer) =
            serial_lines(serial.into_futures_stream()).await;

        // On initial boot, press `f` to enter fastboot.
        enter_fastboot(&mut serial_lines, &mut serial_writer).await;
    }

    // Give emulator time to get to fastboot
    std::thread::sleep(Duration::from_secs(4));

    emu.check_is_running().expect("Emulator exited unexpectedly");

    let product_bundle_json = Emu::product_bundle_dir();

    // Flash in fastboot, then verify that target boots to product.
    let output = isolate
        .ffx(&[
            "--target",
            emu.nodename(),
            "target",
            "flash",
            "-b",
            product_bundle_json.to_str().unwrap(),
        ])
        .await
        .expect("flash target");
    assert!(output.status.success(), "Failed to run command: {}\n{}", output.stdout, output.stderr);

    std::thread::sleep(Duration::from_secs(4));

    emu.check_is_running().expect("Emulator exited unexpectedly");

    // ffx waits for the product to re-enter fastboot before returning.
    let out = isolate
        .ffx(&["--target", emu.nodename(), "target", "bootloader", "info"])
        .await
        .expect("target bootloader info");

    assert!(out.status.success(), "status is unexpected: {:?}", out.status);
    assert!(out.stdout.contains("\nversion: 0.4"), "stdout is unexpected: {:?}", out.stdout);
    assert!(out.stderr.lines().count() == 0, "stderr is unexpected: {:?}", out.stderr);
    emu.check_is_running().expect("Emulator exited unexpectedly");
}

async fn serial_lines(
    serial: impl AsyncRead + AsyncWrite,
) -> (impl Stream<Item = std::io::Result<String>>, impl AsyncWrite) {
    let (reader, writer) = serial.split();
    let reader = BufReader::new(reader);
    (reader.lines(), writer)
}

async fn enter_fastboot(
    lines: &mut (dyn Stream<Item = std::io::Result<String>> + Unpin),
    writer: &mut (dyn AsyncWrite + Unpin),
) {
    while let Some(res) = lines.next().await {
        let line = res.unwrap();
        eprintln!("{}", line);
        if line.contains("Press f to enter fastboot.") {
            eprintln!("TEST: About to press f");
            writer.write_all(b"f").await.expect("failed to press f");
            break;
        }
    }
}

async fn wait_for_target(isolate: &Isolate, emu: &Emu, timeout: i32) -> anyhow::Result<()> {
    let out = isolate
        .ffx(&["--target", emu.nodename(), "target", "wait", "-t", format!("{}", timeout).as_str()])
        .await
        .expect("target wait");

    if !out.status.success() {
        anyhow::bail!("target wait exited unsucessfully: {:?}", out);
    }
    Ok(())
}

#[macro_export]
/// Waits for an emulator to show up in the isolate by invoking
/// ffx target wait and looking for a successful command.
macro_rules! wait_for_target {
    ($isolate: ident, $emu: ident, $timeout: expr) => {
        wait_for_target(&$isolate, &$emu, $timeout).await.unwrap();
    };
    ($isolate: ident, $emu: ident) => {
        // Assume a reasonable default of 2 retries
        wait_for_target!($isolate, $emu, 120)
    };
}
