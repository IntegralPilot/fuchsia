// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use anyhow::{Context as _, Error};
use assert_matches::assert_matches;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_virtualization_guest_interaction as fguest_interaction;
use packet::ParsablePacket as _;
use rand::distr::DistString as _;
use std::io::Write as _;

#[fuchsia::test]
async fn multiple_guests_disallowed() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("test_network").await.expect("failed to create network");
    let guest = netemul::guest::Controller::new("test_guest_1", &network, None)
        .await
        .expect("setup installed guest failed");

    // Validate that create fails when a guest already exists.
    match netemul::guest::Controller::new("test_guest_2", &network, None).await {
        Err(_) => (),
        Ok(_) => panic!("expect guest sandbox disallows creating a guest when one already exists"),
    }

    drop(guest);

    // Now that the guest was dropped, we should be able to create a new guest.
    assert_matches!(netemul::guest::Controller::new("test_guest_4", &network, None).await, Ok(_));
}

fn create_file_with_random_data(path: &str) -> Result<(), Error> {
    const FILE_SIZE: usize = 4096;
    let file_contents =
        rand::distributions::Alphanumeric.sample_string(&mut rand::rng(), FILE_SIZE);
    let mut file = std::fs::File::create(path).context("failed to create file")?;
    file.write_all(file_contents.as_bytes()).context("failed to write to file")?;
    Ok(())
}

#[fuchsia::test]
async fn put_then_get_file() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("test_network").await.expect("failed to create network");
    let guest = netemul::guest::Controller::new("test_guest", &network, None)
        .await
        .expect("setup installed guest failed");
    const LOCAL_DST_PATH: &str = "/tmp/dst.txt";
    const LOCAL_SRC_PATH: &str = "/tmp/src.txt";
    const REMOTE_PATH: &str = "/root/input/data.txt";

    create_file_with_random_data(LOCAL_SRC_PATH).expect("failed to write test data to src file");
    guest.put_file(LOCAL_SRC_PATH, REMOTE_PATH).await.expect("failed to transfer file to guest");

    let () =
        guest.get_file(LOCAL_DST_PATH, REMOTE_PATH).await.expect("failed to get file from guest");

    // Expect the contents match after both transfers.
    let original_contents = std::fs::read_to_string(LOCAL_SRC_PATH).expect("read to string failed");
    let final_contents = std::fs::read_to_string(LOCAL_DST_PATH).expect("read to string failed");
    assert_eq!(original_contents, final_contents);
}

#[fuchsia::test]
async fn exec_script() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("test_network").await.expect("failed to create network");
    let guest = netemul::guest::Controller::new("test_guest", &network, None)
        .await
        .expect("setup installed guest failed");

    const LOCAL_SRC_PATH: &str = "/pkg/data/test_script.sh";
    const REMOTE_DST_PATH: &str = "/root/input/test_script.sh";

    guest
        .put_file(LOCAL_SRC_PATH, REMOTE_DST_PATH)
        .await
        .expect("failed to transfer file to guest");

    let command_to_run: &str = &format!("/bin/sh -c {}", REMOTE_DST_PATH);
    const STDIN_INPUT: &str = "hello\n";
    const STDOUT_ENV_VAR_NAME: &str = "STDOUT_STRING";
    const STDERR_ENV_VAR_NAME: &str = "STDERR_STRING";
    const STDOUT_EXPECTED: &str = "stdout";
    const STDERR_EXPECTED: &str = "stderr";

    let env = vec![
        fguest_interaction::EnvironmentVariable {
            key: STDOUT_ENV_VAR_NAME.to_string(),
            value: STDOUT_EXPECTED.to_string(),
        },
        fguest_interaction::EnvironmentVariable {
            key: STDERR_ENV_VAR_NAME.to_string(),
            value: STDERR_EXPECTED.to_string(),
        },
    ];

    // Request that the guest run the command.
    let (return_code, stdout, stderr) =
        guest.exec(command_to_run, env, Some(STDIN_INPUT)).await.expect("exec failed");

    // Validate the return code, stdout, and stderr.
    assert_eq!(return_code, 0);
    assert_eq!(stdout.trim(), STDOUT_EXPECTED);
    assert_eq!(stderr.trim(), STDERR_EXPECTED);

    // Pull the file that was created by the script and validate its contents.
    const LOCAL_DST_PATH: &str = "/tmp/script_output_copy.txt";
    const REMOTE_PATH: &str = "/root/output/script_output.txt";

    let () =
        guest.get_file(LOCAL_DST_PATH, REMOTE_PATH).await.expect("failed to get file from guest");

    let file_contents = std::fs::read_to_string(LOCAL_DST_PATH).expect("read to string failed");
    assert_eq!(file_contents, STDIN_INPUT.to_string());
}

#[fuchsia::test]
async fn guest_attached_to_network() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("test_network").await.expect("failed to create network");
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");
    let net_mac = net_declare::net_mac!("aa:bb:cc:dd:ee:ff");
    let _guest = netemul::guest::Controller::new(
        "test_guest",
        &network,
        Some(fnet::MacAddress { octets: net_mac.bytes() }),
    )
    .await
    .expect("setup installed guest failed");

    // Linux generates frames on its own; validate that Netemul propagates them to endpoints
    // on the virtual network.
    let (buf, _dropped_frames): (Vec<u8>, u64) =
        fake_ep.read().await.expect("failed to read from endpoint");
    let mut buf = &buf[..];
    let frame = packet_formats::ethernet::EthernetFrame::parse(
        &mut buf,
        packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck,
    )
    .expect("failed to parse ethernet frame");

    assert_eq!(frame.src_mac(), net_mac);
}
