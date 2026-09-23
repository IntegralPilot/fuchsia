// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_debugger as fdebugger;

/// A representation of a DebugAgent instance.
pub struct Agent {
    /// The generated child component name.
    pub name: String,
    /// Client end proxy to this DebugAgent.
    pub debug_agent_proxy: fdebugger::DebugAgentProxy,
    /// A list of the names of all currently attached processes.
    pub attached_processes: Vec<String>,
}

/// Iterates over all DebugAgent instances and collects all of the processes that each is attached
/// to.
pub async fn get_all_debug_agents(launcher_proxy: &fdebugger::LauncherProxy) -> Result<Vec<Agent>> {
    let (iter, server_end) =
        launcher_proxy.domain().create_proxy::<fdebugger::AgentIteratorMarker>();
    launcher_proxy.get_agents(server_end)?;

    let mut agent_vec = Vec::<Agent>::new();
    let mut agents = iter.get_next().await?;
    while !agents.is_empty() {
        for agent in agents.into_iter() {
            let debug_agent_proxy = agent.client_end.into_proxy();
            let (iter_proxy, iter_server) = debug_agent_proxy
                .domain()
                .create_proxy::<fdebugger::AttachedProcessIteratorMarker>(
            );
            debug_agent_proxy.get_attached_processes(iter_server)?;

            let mut agent =
                Agent { name: agent.name, debug_agent_proxy, attached_processes: vec![] };

            let mut procs = iter_proxy.get_next().await?;
            while !procs.is_empty() {
                agent.attached_processes.extend(procs);

                procs = iter_proxy.get_next().await?;
            }

            agent_vec.push(agent);
        }

        agents = iter.get_next().await?;
    }

    Ok(agent_vec)
}

/// Pretty prints the list of DebugAgents and their attached processes returned by
/// get_all_debug_agents above.
pub fn print_debug_agents(agent_vec: &Vec<Agent>) {
    for (i, agent) in agent_vec.iter().enumerate() {
        println!("[{}] {}", i + 1, agent.name);

        // Only print the first three things that are attached.
        for name in agent.attached_processes.iter().take(3) {
            println!("    + {name}");
        }

        if agent.attached_processes.len() > 3 {
            println!("    ...");
        }
    }
}
