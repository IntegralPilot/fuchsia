# `gn_desc`: A tool for rapidly querying the GN build graph

The `gn_desc` cli tool uses the `project.json` produced by GN to more rapidly
query GN's build graph than can be done using `fx gn desc`.

## Example Usage

To list targets that match a pattern:

`fx gn_desc -v --file out/default/project.json match <pattern> list`

The `<pattern>` is a regex.

# Usage

If the GN files have been changed, and a build hasn't been performed, run
`fx gen` to update `project.json`.

## Operation

The tool does the following when run:

1. parses the given `project.json` file
1. creates a graph of all targets and their dependencies
1. selects some subset of the targets
1. runs a command on each selected target

## Target Selection

`gn_desc` has the following target selection mechanisms:

- `match <regex>` - selects all targets that match the given regex
- `match-file [-a|-ois] <regex>` - selects all targets that have files that
match the given regex:
  - `-a` - any file (the default)
  - `-i` - inputs
  - `-o` - outputs
  - `-s` - sources
  - `-x` - scripts (executable things)
- `label <label>` - Selects only given label (via an exact match, including the
toolchain).

## Commands

`gn_desc` has the following commands that can be run on the selected targets:

- `list` - lists the labels of the selected targets, one per line
- `summarize` - provides a summary description of the targets, which can be
customized by various flags, see the tool's own help for more information: `gn_desc --file <file> exact <label> summarize --help`

# Future Work

Planned future commands are:

- `dep-tree` - prints an ascii-art or dot-file format dependency tree
- `file-tree` - prints an ascii-art or dor-file format file input->output tree
- `metadata query` - performs a metadata-query similar to what `gn` does

Planned future selectors are:

- `match-metadata` - selects all targets that have a given metadata key




