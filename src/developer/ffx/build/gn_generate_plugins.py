#!/usr/bin/env fuchsia-vendored-python
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
import argparse
import sys


def to_camel_case(snake_str: str) -> str:
    components = snake_str.split("_")
    return "".join(x.title() for x in components[0:])


def wrap_deps(dep: str) -> dict[str, str]:
    return {"enum": to_camel_case(dep), "lib": dep}


TEMPLATE = """pub async fn ffx_plugin_impl(
  _env: &fho::FhoEnvironment,
  _cmd: {suite_args_lib}::FfxPluginCommand,
) -> fho::Result<()>
{{
{plugin_impl_body}
}}

pub fn ffx_plugin_redact_args(
  _app: &fho::FfxCommandLine,
  _cmd: &{suite_args_lib}::FfxPluginCommand
) -> Vec<String>
{{
{redact_args_body}
}}

// Since the subcommands can be implemented in separate libraries, these
// need to be collected and inspect the writers
// for each sub command to see if any of the subcommands support
// structured machine output.
pub fn ffx_plugin_supports_structured_output(
  _cmd: &{suite_args_lib}::FfxPluginCommand,
) -> bool {{
{supports_structured_output_body}
}}

// Since the subcommands can be implemented in separate libraries, these
// need to be collected and inspect the writers
// for each sub command to see if any of the subcommands support
// a schema for the machine output.
pub fn ffx_plugin_has_schema(
  _cmd: &{suite_args_lib}::FfxPluginCommand,
) -> bool {{
{has_schema_body}
}}
{suite_adapter}"""

SUITE_ADAPTER_TEMPLATE = """
/// SuiteAdapter is used to stitch multiple subcommands together
/// and delegate the processing of the command line down the
/// sub command tree.
/// This makes working loosely coupled subcommands insanely hard,
/// (if you are reading this I think you agree).
///
/// Changes to this template most likely need changes to
/// //src/developer/ffx/lib/fho/src/adapters.rs as well.
///
/// The old plugin macro might also have to change:
/// //src/developer/ffx/core/macro/src/impl.rs
struct SuiteAdapter {{
    cmd: {suite_args_lib}::FfxPluginCommand,
    env:  fho::FhoEnvironment,
}}

#[fho::macro_deps::async_trait(?Send)]
impl fho::FfxTool for SuiteAdapter {{
    type Command = {suite_args_lib}::FfxPluginCommand;

    fn supports_structured_output(&self) -> bool {{
      ffx_plugin_supports_structured_output(&self.cmd)
    }}

    fn has_schema(&self) -> bool {{
      ffx_plugin_has_schema(&self.cmd)
    }}

    // This is provided only for completeness of the trait. Do not expect
    // it be meaningful. The correct value for plugins is determined within
    // ffx_plugin_impl().
    fn requires_target() -> bool {{
      false
    }}

    async fn from_env(env: fho::FhoEnvironment, cmd: Self::Command) -> fho::Result<Self> {{
        Ok(SuiteAdapter {{ cmd, env }})
    }}
}}

#[fho::macro_deps::async_trait(?Send)]
impl fho::FfxMain for SuiteAdapter {{
    // The Writer type here should not matter, and as such, do not
    // rely on it be accurate. The correct writer is created
    // as part of ffx_plugin_impl().
    type Writer = fho::null_writer::NullWriter;
    type Error = fho::Error;

    async fn main(self, _writer:  Self::Writer) -> fho::Result<()> {{
        ffx_plugin_impl(&self.env, self.cmd).await
    }}

    async fn try_print_schema(self, _writer: Self::Writer)  -> fho::Result<()> {{
      ffx_plugin_impl(&self.env, self.cmd).await
    }}
}}

pub async fn fho_suite_main() {{
  use fho::FfxTool;

  SuiteAdapter::execute_tool().await
}}
"""


def generate_plugins(
    plugins: list[dict[str, str]],
    suite_args_lib: str,
    suite_subcommand_lib: str | None,
    includes_subcommands: bool,
    includes_execution: bool,
    execution_lib: str,
) -> str:
    if includes_subcommands:
        assert (
            suite_subcommand_lib is not None
        ), "--sub_command must be provided when subcommands are included"

    # 1. ffx_plugin_impl
    plugin_impl_parts = []
    if includes_subcommands:
        match_expr = (
            "_cmd.subcommand" if includes_execution else "Some(_cmd.subcommand)"
        )
        if not includes_execution:
            plugin_impl_parts.append(
                "  // If there are subcommands, match on each sub command\n"
                "  // enum variant, and call the matching ffx_suite.\n"
                "  // This passes the command line down the subcommand structure."
            )
        arms = "\n".join(
            f"    Some({suite_subcommand_lib}::SubCommand::{p['enum']}(c)) => return {p['lib']}_suite::ffx_plugin_impl(_env, *c.0).await.map_err(fho::Error::from),"
            for p in plugins
        )
        if arms:
            arms += "\n"
        plugin_impl_parts.append(
            f"  match {match_expr} {{\n"
            f"{arms}"
            f"    // This handles everything that does not match. This falls through to not implemented.\n"
            f"    None => (),\n"
            f"  }};"
        )
    if includes_execution:
        plugin_impl_parts.append(
            f"  {execution_lib}::ffx_plugin_impl(_env, _cmd).await.map_err(fho::Error::from)"
        )
    else:
        plugin_impl_parts.append(
            '  eprintln!("This subCommand is not implemented yet.");\n  Ok(())'
        )
    plugin_impl_body = "\n".join(plugin_impl_parts)

    # 2. ffx_plugin_redact_args
    redact_args_parts = []
    if includes_subcommands:
        match_expr = (
            "&_cmd.subcommand"
            if includes_execution
            else "Some(&_cmd.subcommand)"
        )
        arms = "\n".join(
            f"    Some({suite_subcommand_lib}::SubCommand::{p['enum']}(c)) => return _app.redact_subcmd(&*c.0),"
            for p in plugins
        )
        if arms:
            arms += "\n"
        redact_args_parts.append(
            f"  match {match_expr} {{\n{arms}    None => {{}},\n  }}"
        )
    redact_args_parts.append("  vec![]")
    redact_args_body = "\n".join(redact_args_parts)

    # 3. ffx_plugin_supports_structured_output
    supports_parts = []
    if includes_subcommands:
        match_expr = (
            "&_cmd.subcommand"
            if includes_execution
            else "Some(&_cmd.subcommand)"
        )
        arms = "\n".join(
            f"    Some({suite_subcommand_lib}::SubCommand::{p['enum']}(c)) => return {p['lib']}_suite::ffx_plugin_supports_structured_output(&*c.0),"
            for p in plugins
        )
        if arms:
            arms += "\n"
        supports_parts.append(
            f"  match {match_expr} {{\n{arms}    None => (),\n  }};"
        )
    if includes_execution:
        supports_parts.append(
            f"  {execution_lib}::ffx_plugin_supports_structured_output()"
        )
    else:
        supports_parts.append(
            "  // If no subcommand is matched, return false.\n  false"
        )
    supports_structured_output_body = "\n".join(supports_parts)

    # 4. ffx_plugin_has_schema
    schema_parts = []
    if includes_subcommands:
        match_expr = (
            "&_cmd.subcommand"
            if includes_execution
            else "Some(&_cmd.subcommand)"
        )
        arms = "\n".join(
            f"    Some({suite_subcommand_lib}::SubCommand::{p['enum']}(c)) => return {p['lib']}_suite::ffx_plugin_has_schema(&*c.0),"
            for p in plugins
        )
        if arms:
            arms += "\n"
        schema_parts.append(
            f"  match {match_expr} {{\n{arms}    None => (),\n  }};"
        )
    if includes_execution:
        schema_parts.append(f"  {execution_lib}::ffx_plugin_has_schema()")
    else:
        schema_parts.append(
            "  // If no subcommand is matched, return false.\n  false"
        )
    has_schema_body = "\n".join(schema_parts)

    # 5. SuiteAdapter
    if includes_subcommands:
        suite_adapter = SUITE_ADAPTER_TEMPLATE.format(
            suite_args_lib=suite_args_lib
        )
    else:
        suite_adapter = ""

    return TEMPLATE.format(
        suite_args_lib=suite_args_lib,
        plugin_impl_body=plugin_impl_body,
        redact_args_body=redact_args_body,
        supports_structured_output_body=supports_structured_output_body,
        has_schema_body=has_schema_body,
        suite_adapter=suite_adapter,
    )


def main(args_list: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Generate FFX Plugin matcher")

    parser.add_argument(
        "--out", help="The output file to generate", required=True
    )

    parser.add_argument(
        "--deps",
        help="Comma-seperated libraries to generate code from",
        required=False,
    )

    parser.add_argument("--args", help="args lib", required=True)

    parser.add_argument("--sub_command", help="sub command lib", required=False)

    parser.add_argument(
        "--includes_execution",
        action="store_true",
        help="includes execution code",
        default=False,
    )

    parser.add_argument(
        "--includes_subcommands",
        action="store_true",
        help="includes subcommands",
        default=False,
    )

    parser.add_argument(
        "--execution_lib", help="name of execution lib", required=True
    )

    parser.add_argument(
        "--template",
        help="Deprecated: template file argument",
        required=False,
    )

    if args_list:
        args = parser.parse_args(args_list)
    else:
        args = parser.parse_args()

    if args.deps:
        libraries = [lib for lib in args.deps.split(",") if lib]
        plugins = list(map(wrap_deps, libraries))
    else:
        plugins = []

    rendered = generate_plugins(
        plugins=plugins,
        suite_subcommand_lib=args.sub_command,
        suite_args_lib=args.args,
        includes_subcommands=args.includes_subcommands,
        includes_execution=args.includes_execution,
        execution_lib=args.execution_lib,
    )

    try:
        with open(args.out, "r") as f:
            existing = f.read()
    except FileNotFoundError:
        existing = None

    if existing != rendered:
        with open(args.out, "w") as f:
            f.write(rendered)

    return 0


if __name__ == "__main__":
    sys.exit(main())
