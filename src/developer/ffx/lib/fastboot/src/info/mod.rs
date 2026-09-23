// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::FfxFastbootError;
type Result<T> = std::result::Result<T, FfxFastbootError>;
use ffx_fastboot_interface::fastboot_interface::{FastbootInterface, Variable};
use std::io::Write;
use tokio::sync::mpsc::{Receiver, Sender};

/// Aggregates fastboot variables from a callback listener.
pub async fn handle_variables_for_fastboot<W: Write>(
    writer: &mut W,
    mut var_server: Receiver<Variable>,
) -> Result<()> {
    loop {
        match var_server.recv().await {
            Some(Variable { name, value, .. }) => {
                writeln!(writer, "{}: {}", name, value)?;
            }
            None => return Ok(()),
        }
    }
}

pub async fn info<F: FastbootInterface>(
    messenger: Sender<Variable>,
    fastboot_interface: &mut F,
) -> Result<()> {
    fastboot_interface.get_all_vars(messenger).await?;
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    type Result<T> = std::result::Result<T, anyhow::Error>;
    use ffx_fastboot_interface::test::setup;
    use tokio::sync::mpsc;

    #[fuchsia::test]
    async fn test_showing_variables() -> Result<()> {
        let (_, mut proxy) = setup();
        let (client, mut server) = mpsc::channel(1);
        info(client, &mut proxy).await?;
        let e = server.recv().await.unwrap();
        assert_eq!(e, Variable { name: "test".to_string(), value: "test".to_string() });
        Ok(())
    }
}
