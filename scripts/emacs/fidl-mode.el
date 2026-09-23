;;; fidl-mode.el - A major mode for editing fidl files.

;;; Copyright (c) 2022 The Fuchsia Authors. All rights reserved.
;;; Use of this source code is governed by a BSD-style license that can be
;;; found in the LICENSE file.

;; Author: Max Regan <maxregan@google.org>
;; Created: December 2, 2022
;; Keywords: tools, fuchsia, fidl

;; This file is not part of GNU Emacs.

;;; Commentary:

;; A major mode for editing FIDL files. FIDL stands for Fuchsia Interface
;; Definition Language, which is the specification for IPC messages in Fuchsia.
;; For more information on FIDL, see the Fuchsia documentation:
;; https://fuchsia.dev/fuchsia-src/development/languages/fidl

;; This source was adapted from gn-mode.el:
;; https://gn.googlesource.com/gn/+/refs/heads/main/misc/emacs/gn-mode.el

;;; Code:

(require 'smie)

(defgroup fidl nil
  "Major mode for editing FIDL files."
  :prefix "fidl-"
  :group 'languages)

(defcustom fidl-indent-basic 4
  "The number of spaces to indent a new scope."
  :group 'fidl
  :type 'integer)

(defgroup fidl-faces nil
  "Faces used in FIDL mode."
  :group 'fidl
  :group 'faces)

(defvar fidl-font-lock-reserved-keywords
  '("alias" "as" "bits" "compose" "const" "enum" "error" "flexible" "library"
    "optional" "protocol" "reserved" "resource" "service" "struct" "strict"
    "type" "union" "using" "table"))

(defvar fidl-font-lock-type-keywords
  '("bool" "byte" "float32" "float64" "int8" "int16" "int32" "int64" "uint8"
  "uint16" "uint32" "uint64"))

(defvar fidl-font-lock-constant-keywords
  '("true" "false"))

;; Currently, the syntax highlighting does little more than color keywords
(defconst fidl-font-lock-keywords
  `((,(regexp-opt fidl-font-lock-reserved-keywords 'words) .
     font-lock-keyword-face)
    (,(regexp-opt fidl-font-lock-type-keywords 'words) .
     font-lock-type-face)
    (,(regexp-opt fidl-font-lock-constant-keywords 'words) .
     font-lock-constant-face)
    ;; Literal hexidecimal integers
    ("0x[0-9A-Fa-f]+" . font-lock-constant-face)
    ;; Literal binary representation
    ("0b[01]*" . font-lock-constant-face)
    ;; Floating-point literals
    ("[-+]?[0-9]*\\.?" . font-lock-constant-face)
  ))

;; These SMIE rules are like those from the example in the docs, with tweaks to
;; the 'basic' element so they only trigger additional indent if they are
;; hanging, e.g. instead of:
;;
;;     Foo() -> ( resource struct {
;;            bar
;;        })
;;
;; get:
;;
;;     Foo() -> ( resource struct {
;;         bar
;;     })
;;
;; https://www.gnu.org/software/emacs/manual/html_node/elisp/SMIE-Indentation-Example.html
(defun fidl-smie-rules (method arg)
  "This method is passed to `smie-setup' to handle FIDL's indentation rules.
METHOD and ARG are documented in smie."
  (pcase (cons method arg)
    ;; See comment above
    (`(:elem . basic) (if (smie-rule-hanging-p) fidl-indent-basic 0))
    ;; Treat items between ; as list elements
    (`(,_ . ";") (smie-rule-separator method))
    ;; Don't indent 'list' elements after the first
    (`(:list-intro . "") t)
    ;; See docs, needed for virtual indentation.
    (`(:before . ,(or `"[" `"(" `"{")) (smie-rule-parent))))

(defun fuchsia-fill-paragraph (&optional justify)
  "We only fill inside of comments in FIDL mode."
  (interactive "P")
  (or (fill-comment-paragraph justify)
      ;; Never return nil; `fill-paragraph' will perform its default behavior
      ;; if we do.
      t))

;;;###autoload
(define-derived-mode fidl-mode prog-mode "FIDL"
  "Major mode for editing FIDL (Fuchsia Interface Definition Language)."
  :group 'fidl

  (setq-local comment-use-syntax t)
  (setq-local comment-start "//")
  (setq-local comment-end "")
  ;; Teach emacs fidl-mode about C-style double-slash comments
  (modify-syntax-entry ?/ "< 12" fidl-mode-syntax-table)
  (modify-syntax-entry ?\n "> " fidl-mode-syntax-table)
  ;; 80 characters for comments, per
  ;; https://fuchsia.dev/fuchsia-src/development/languages/fidl/guides/style#comments
  (setq-local fill-column 80)

  (setq-local fill-paragraph-function 'fuchsia-fill-paragraph)

  (setq-local font-lock-defaults '(fidl-font-lock-keywords))

  (setq-local indent-tabs-mode nil)
  (smie-setup nil #'fidl-smie-rules)
  (setq-local smie-indent-basic fidl-indent-basic))

;;;###autoload
(add-to-list 'auto-mode-alist '("\\.fidl\\'" . fidl-mode))

(provide 'fidl-mode)
