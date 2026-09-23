# Scripts

This repository is for scripts useful when working with the Fuchsia source code.

## fx

fx is the front-door to a collection of scripts that make many tasks related to
Fuchsia development easier. It contains a large number of subcommands, which can
be discovered by running `fx help`. If you use bash or zsh as a shell, you can
get some auto-completion for fx by sourcing `scripts/fx-env.sh` into your shell.

* See more information on the supported [fx workflows](/docs/development/build/fx.md)

* Learn how to develop fx subcommands in [fx subcommands](/tools/devshell/README.md)

## OWNERShip

Given the wide breadth of the code under //scripts and the project's desire to
encourage more local ownership, there are intentionally no OWNERS of //scripts
itself (only per-file entries). Rather, the addition of new subdirectories or
top-level files is expected to fall back toward requiring an
[OWNERS override][owners-override].

[owners-override]: https://fuchsia.dev/fuchsia-src/development/source_code/owners?hl=en#owners_override
