// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
mod tests {
    use ffx_command::CliArgsInfo;
    use ffx_testing::{TestContext, base_fixture};
    use fixture::fixture;
    use std::collections::HashSet;

    // base_fixture sets up the test environment this test
    #[fixture(base_fixture)]
    #[fuchsia::test]
    async fn test_top_level(ctx: TestContext) {
        // Get the top level help and make sure all the commands in `ffx commands` are listed.
        let (builtins, externals) = parse_top_level_commands(&ctx).await;

        // We're not concerned in this test how many commands were returned, just that a reasonable number was.
        assert!(builtins.len() >= 5, "Expected at least 5 builtin commands, got {builtins:?}");
        assert!(externals.len() >= 1, "Expected at least 1 external command, got {externals:?}");

        // Now get the json help and make sure all the commands are referenced.
        let output = ctx.isolate().ffx(&["--machine", "json", "--help"]).await.expect("help");
        assert!(output.status.success());

        let info: CliArgsInfo = serde_json::from_str(&output.stdout).expect("parsing json");
        assert_eq!("Ffx", info.name);
        // look for --machine and --help and --verbose, which are _some_ of the flags, just
        // making sure the flags were populated.
        assert!(info.flags.iter().any(|f| f.long == "--machine"), "expected --machine flag");
        assert!(info.flags.iter().any(|f| f.long == "--help"), "expected --help flag");
        assert!(info.flags.iter().any(|f| f.long == "--verbose"), "expected --verbose flag");

        // now check all the built-in commands are present
        for cmd in builtins {
            assert!(
                info.commands.iter().any(|c| c.name == cmd),
                "expected builtin {cmd} subcommand"
            );
        }
        // now check all the built-in commands are present
        for cmd in externals {
            assert!(
                info.commands.iter().any(|c| c.name == cmd),
                "expected external {cmd} subcommand"
            );
        }
    }

    #[fixture(base_fixture)]
    #[fuchsia::test]
    async fn test_toplevel_builtin(ctx: TestContext) {
        // Tests that `ffx --machine --json config --help` returns help for only
        // the config command (and its subcommands)

        // Get the top level commands in `ffx commands` to make sure config is still builtin
        let (builtins, _externals) = parse_top_level_commands(&ctx).await;

        assert!(builtins.iter().any(|c| c == "config"), "expected `config` to be built-in");

        // Now get the json help and make sure config is the top.
        let output =
            ctx.isolate().ffx(&["--machine", "json", "config", "--help"]).await.expect("help");
        assert!(output.status.success());

        let info: CliArgsInfo = serde_json::from_str(&output.stdout).expect("parsing json");
        assert_eq!("config", info.name);
        assert!(info.commands.iter().any(|c| c.name == "get"), "expected get subcommand");
        assert!(info.commands.iter().any(|c| c.name == "set"), "expected set subcommand");
    }

    #[fixture(base_fixture)]
    #[fuchsia::test]
    async fn test_subcommand_builtin(ctx: TestContext) {
        // Tests that `ffx --machine --json config get--help` returns help for only
        // the config get command (and its subcommands)

        // Get the top level commands in `ffx commands` to make sure config is still builtin
        let (builtins, _externals) = parse_top_level_commands(&ctx).await;

        assert!(builtins.iter().any(|c| c == "config"), "expected `config` to be built-in");

        // Now get the json help and make sure config is the top.
        let output = ctx
            .isolate()
            .ffx(&["--machine", "json", "config", "get", "--help"])
            .await
            .expect("help");
        assert!(output.status.success());

        let info: CliArgsInfo = serde_json::from_str(&output.stdout).expect("parsing json");
        assert_eq!("get", info.name);
        assert!(info.flags.iter().any(|f| f.long == "--process"), "expected --process flag");
    }

    #[fixture(base_fixture)]
    #[fuchsia::test]
    async fn test_toplevel_external(ctx: TestContext) {
        // Tests that `ffx --machine --json self-test --help` returns help for only
        // the self-test command (and its subcommands)

        // Get the top level commands in `ffx commands` to make sure config is still builtin
        let (_builtins, externals) = parse_top_level_commands(&ctx).await;

        assert!(externals.iter().any(|c| c == "self-test"), "expected `self-test` to be external");

        // Now get the json help and make sure config is the top.
        let output =
            ctx.isolate().ffx(&["--machine", "json", "self-test", "--help"]).await.expect("help");
        assert!(output.status.success());

        let info: CliArgsInfo = serde_json::from_str(&output.stdout).expect("parsing json");
        assert_eq!("self-test", info.name);
        assert!(
            info.flags.iter().any(|f| f.long == "--include-target"),
            "expected --include-target flag"
        );
        assert!(
            info.commands.iter().any(|c| c.name == "experiment"),
            "expected experiment subcommand"
        );
    }

    #[fixture(base_fixture)]
    #[fuchsia::test]
    async fn test_subcommand_external(ctx: TestContext) {
        // Tests that `ffx --machine --json self-test experiment --help` returns help for only
        // the self-test experiment subcommand.

        // Get the top level commands in `ffx commands` to make sure config is still builtin
        let (_builtins, externals) = parse_top_level_commands(&ctx).await;

        assert!(externals.iter().any(|c| c == "self-test"), "expected `self-test` to be external");

        // Now get the json help and make sure config is the top.
        let output = ctx
            .isolate()
            .ffx(&["--machine", "json", "self-test", "experiment", "--help"])
            .await
            .expect("help");
        assert!(output.status.success());

        let info: CliArgsInfo = serde_json::from_str(&output.stdout).expect("parsing json");
        assert_eq!("experiment", info.name);
        assert!(info.flags.iter().any(|f| f.long == "--help"), "expected --help flag");
    }

    #[fixture(base_fixture)]
    #[fuchsia::test]
    async fn test_unique_subcommands(ctx: TestContext) {
        // Tests that `ffx --machine --json --help` returns subcommand list with unique names.

        // Now get the json help and make sure config is the top.
        let output = ctx.isolate().ffx(&["--machine", "json", "--help"]).await.expect("help");
        assert!(output.status.success());

        let info: CliArgsInfo = serde_json::from_str(&output.stdout).expect("parsing json");

        let mut seen = HashSet::<String>::new();
        for cmd in &info.commands {
            assert!(seen.insert(cmd.name.clone()), "duplicate command {}", cmd.name);
        }
    }

    /// Parses `ffx commands` into lists of builtin and external command names.
    /// Based on the output structure like:
    ///
    /// Built-in Commands:
    ///
    /// config            View and switch default and user configurations
    /// <....>
    ///
    /// Workspace Commands:
    ///
    /// starnix           Control starnix containers
    /// audio             Interact with the audio subsystem.
    ///<...>
    ///
    async fn parse_top_level_commands(ctx: &TestContext) -> (Vec<String>, Vec<String>) {
        let mut built_ins: Vec<String> = vec![];
        let mut externals: Vec<String> = vec![];
        let mut collector: &mut Vec<String> = &mut built_ins;
        let output = ctx.isolate().ffx(&["commands"]).await.expect("commands");
        // parse the output lines as
        // Convert string outputs to vector of lines.
        for line in output.stdout.lines() {
            match line {
                "Built-in Commands:" => collector = &mut built_ins,
                "Workspace Commands:" | "SDK Commands:" => collector = &mut externals,
                "" => (),
                _ => {
                    // split on multiple spaces to avoid descriptions that wrap.
                    if let Some((name, _description)) = line.trim().split_once("  ") {
                        collector.push(name.into());
                    }
                }
            }
        }
        (built_ins, externals)
    }
}
