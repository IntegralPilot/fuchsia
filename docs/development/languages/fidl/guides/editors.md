# FIDL editors

Several editors have support for FIDL:

* [IntelliJ/Android Studio](#intellij)
* [Sublime Text](#sublime)
* [Vim](#vim)
* [NeoVim](#nvim)
* [Helix](#helix)
* [Visual Studio Code](#visual-studio-code)

## IntelliJ / Android Studio {#intellij}

There is an IntelliJ plugin available for FIDL. It adds syntax and parsing
support.To install it, select **Settings**, then **Plugins**, and then click
**Browse Repositories** and search for **FIDL**.

## Sublime Text {#sublime}

[Sublime syntax highlighting support](/tools/fidl/editors/sublime).

To install, select **Sublime Text**, then **Preferences**, then
**Browse Packages** and copy or symlink the files `FIDL.sublime-syntax`, and
`Comments (FIDL).tmPreferences` into the **User** package.

## Vim {#vim}

[Vim syntax highlighting support and instructions](/tools/fidl/editors/vim).

## NeoVim {#nvim}

[Tree Sitter FIDL](https://github.com/google/tree-sitter-fidl).

Require NeoVim version >= 0.9 to use
[nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter) plugin.

For Googlers: You may want to build the latest NeoVim, see http://go/neovim.

1. `:TSInstall fidl` to install the parser.
2. Add filetype mapping, you may add this to <nvim-config>/lua/options.lua:
   `vim.filetype.add({ extension = { fidl = "fidl" } })`.

## Helix {#helix}

Helix uses [Tree Sitter FIDL](https://github.com/google/tree-sitter-fidl).

add following to `~/.config/helix/languages.toml`, or wait use a build with
[commit 358ac6bc1f512ca7303856dc904d4b4cdc1fe718](https://github.com/helix-editor/helix/commit/358ac6bc1f512ca7303856dc904d4b4cdc1fe718)

```toml
[[language]]
name = "fidl"
scope = "source.fidl"
injection-regex = "fidl"
file-types = ["fidl"]
comment-token = "//"
indent = { tab-width = 4, unit = "    " }

[[grammar]]
name = "fidl"
source = { git = "https://github.com/google/tree-sitter-fidl", rev = "bdbb635a7f5035e424f6173f2f11b9cd79703f8d" }
```

then fetch and build the parser and copy queries files to runtime dir:

```sh
hx --grammar fetch fidl
hx --grammar build fidl
mkdir -p ~/.config/helix/runtime/queries/
cp -r <path to helix source>/runtime/queries/fidl ~/.config/helix/runtime/queries
```

## Visual Studio Code {#visual-studio-code}

There is a an extension,
[Visual Studio Code extension available](https://marketplace.visualstudio.com/items?itemName=fuchsia-authors.vscode-fuchsia).

## Contributing

Contributions to other plugins are welcome. Their respective code is in:

* [IntelliJ](https://fuchsia.googlesource.com/intellij-language-fidl/)
* [Sublime](/tools/fidl/editors/sublime)
* [vim](/tools/fidl/editors/vim/)
