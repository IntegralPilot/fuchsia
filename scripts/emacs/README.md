# Emacs Libraries for Fuchsia Development.

This directory contains scripts for interacting with the Fuchsia source tree.

# Utilities

# `fx` and `ffx` wrappers

`fuchsia.el` provides wrappers for some of the most common `fx` commands, such
as `fx build`. Commands are prefixed with `fuchsia-`, e.g. `fuchsia-fx-build`.

Notably, executing `fuchsia-fx-*` with the prefix argument `C-u` will give `fx`
the `-i` argument, which runs the given `fx` command every time a source file
changes.

# `fidl-mode` and `cml-mode`

These files add major modes with basic syntax highlighting and indentation in
buffers visiting their respective file types.

# Setup

## Vanilla Emacs

A minimal configuration of these tools only requires adding them to your
`load-path` and loading the package. This uses an existing checkout of the
Fuchsia source code.

```emacs-lisp
(push "<fuchsia source root>/scripts/emacs" load-path)

(require 'fuchsia)
(require 'fidl-mode)
```

## Doom Emacs

Configuration for [Doom Emacs](https://github.com/doomemacs/doomemacs) requires
adding the below to your `~/.doom.d/` configurations:

```emacs-lisp
(push "<fuchsia source root>/scripts/emacs" load-path)
(use-package! fuchsia)
(use-package! fidl-mode)
(use-package! cml-mode)
```
