// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use guest_cli_args::*;

#[fuchsia::main(logging_tags = ["guest"])]
async fn main() -> Result<(), anyhow::Error> {
    let options: GuestOptions = argh::from_env();
    let services = guest_cli::platform::FuchsiaPlatformServices::new();

    match options.nested {
        SubCommands::Attach(attach_args) => {
            let output = guest_cli::attach::handle_attach(&services, &attach_args).await?;
            println!("{}", output);
            Ok(())
        }
        SubCommands::Launch(launch_args) => {
            let output = guest_cli::launch::handle_launch(&services, &launch_args).await;
            println!("{}", output);
            Ok(())
        }
        SubCommands::Stop(stop_args) => {
            let output = guest_cli::stop::handle_stop(&services, &stop_args).await?;
            println!("{}", output);
            Ok(())
        }
        SubCommands::Balloon(balloon_args) => {
            let output = guest_cli::balloon::handle_balloon(&services, &balloon_args).await;
            println!("{}", output);
            Ok(())
        }
        SubCommands::List(list_args) => {
            let output = guest_cli::list::handle_list(&services, &list_args).await?;
            println!("{}", output);
            Ok(())
        }
        SubCommands::VsockPerf(vsockperf_args) => {
            let output = guest_cli::vsockperf::handle_vsockperf(&services, &vsockperf_args).await?;
            println!("{}", output);
            Ok(())
        }
        SubCommands::Socat(socat_args) => {
            let output = guest_cli::socat::handle_socat(&services, &socat_args).await;
            println!("{}", output);
            Ok(())
        }

        SubCommands::Mem(mem_args) => {
            let output = guest_cli::mem::handle_mem(&services, mem_args).await?;
            println!("{}", output);
            Ok(())
        }
    }
}
