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
    return {"enum": to_camel_case(dep), "lib": dep + "_args"}


def generate_cmd(deps: list[dict[str, str]]) -> str:
    boxed_types = "\n".join(
        f"pub type Boxed{d['enum']} = Boxed<{d['lib']}::FfxPluginCommand>;"
        for d in deps
    )
    enum_variants = "\n".join(f"  {d['enum']}(Boxed{d['enum']})," for d in deps)

    return f"""#[derive(Debug, PartialEq)]
pub struct Boxed<T>(pub Box<T>);

impl<T: argh::FromArgs> argh::FromArgs for Boxed<T> {{
    fn from_args(command_name: &[&str], args: &[&str]) -> Result<Self, argh::EarlyExit> {{
        T::from_args(command_name, args).map(|t| Boxed(Box::new(t)))
    }}
    fn redact_arg_values(
        command_name: &[&str],
        args: &[&str],
    ) -> Result<Vec<String>, argh::EarlyExit> {{
        T::redact_arg_values(command_name, args)
    }}
}}

impl<T: argh::SubCommand> argh::SubCommand for Boxed<T> {{
    const COMMAND: &'static argh::CommandInfo = T::COMMAND;
}}

impl<T: argh::ArgsInfo> argh::ArgsInfo for Boxed<T> {{
    fn get_args_info() -> argh::CommandInfoWithArgs {{
        T::get_args_info()
    }}
}}

{boxed_types}

#[derive(argh::ArgsInfo, argh::FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum SubCommand {{
{enum_variants}
}}
"""


def main(args_list: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Generate FFX Command struct")

    parser.add_argument(
        "--out", help="The output file to generate", required=True
    )

    parser.add_argument(
        "--deps",
        help="Comma-seperated libraries to generate code from",
        required=True,
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

    libraries = args.deps.split(",") if args.deps else []
    deps = [wrap_deps(lib) for lib in libraries if lib]
    rendered = generate_cmd(deps)

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
