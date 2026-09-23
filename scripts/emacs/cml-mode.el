;;; cml-mode.el - A major mode for editing cml files.

;;; Copyright (c) 2022 The Fuchsia Authors. All rights reserved.
;;; Use of this source code is governed by a BSD-style license that can be
;;; found in the LICENSE file.

;; Author: Max Regan <maxregan@google.com>
;; Created: December 8, 2022
;; Keywords: tools, fuchsia, cml

;; This file is not part of GNU Emacs.

;;; Commentary:

;; A major mode for editing CML files: Fuchsia's Component Manifest Source files.
;; See the documentation at https://fuchsia.dev/reference/cml

;;; Code:

(require 'fuchsia-json5)

(defvar cml-font-lock-reserved-keywords
  '("capabilities" "children" "collections" "config" "disable" "environments"
    "expose" "facets" "include" "offer" "program" "use"))

;; Currently, the syntax highlighting does little more than color keywords
(defconst cml-font-lock-keywords
  `((,(regexp-opt cml-font-lock-reserved-keywords 'words) .
     font-lock-keyword-face)
    ;; Literal hexidecimal integers
    ("0x[0-9A-Fa-f]+" . font-lock-constant-face)
    ;; Literal binary representation
    ("0b[01]+" . font-lock-constant-face)
    ;; Floating-point literals
    ("[-+]?[0-9]+\\.?" . font-lock-constant-face)
  ))


(define-derived-mode cml-mode
  fuchsia--json5-mode "CML"
  "Major mode for editing Fuchsia CML (Component Manifest Source) files"
  (setq-local fill-column 80)
  (setq-local fill-paragraph-function 'fuchsia-fill-paragraph)
  (setq-local indent-tabs-mode nil)
  (setq-local yaml-indent-offset 4)
  (setq-local font-lock-defaults '(cml-font-lock-keywords)))

;;;###autoload
(add-to-list 'auto-mode-alist '("\\.cml\\'" . cml-mode))

(provide 'cml-mode)
;;; cml-mode.el ends here
