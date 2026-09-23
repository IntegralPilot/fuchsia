;;; fuchsia-json5.el - A major mode for editing json5 files.

;;; Copyright (c) 2022 The Fuchsia Authors. All rights reserved.
;;; Use of this source code is governed by a BSD-style license that can be
;;; found in the LICENSE file.

;; Author: Max Regan <maxregan@google.com>
;; Created: December 8, 2022
;; Keywords: tools, fuchsia, json5

;; This file is not part of GNU Emacs.

;;; Commentary:

;; A major mode for editing JSON5 files- an extension of JSON. There exist other
;; json5-modes out on the net, so this one is namespaced. Its also hinted to be
;; internal since it's leaning so heavily on yaml-mode, so don't expect the
;; behavior to be stable in case that changes.

;;; Code:

(define-derived-mode fuchsia--json5-mode
  ;; YAML is a decent approximation of JSON5, so we can start with that.
  yaml-mode "JSON5"
  "Major mode for JSON5 files."

  ;; Teach emacs json5-mode about C-style double-slash comments
  (setq-local comment-use-syntax t)
  (setq-local comment-start "//")
  (setq-local comment-end "")
  (modify-syntax-entry ?/ "< 12" fuchsia--json5-mode-syntax-table)
  (modify-syntax-entry ?\n "> " fuchsia--json5-mode-syntax-table))

;;;###autoload
(add-to-list 'auto-mode-alist '("\\.json5\\'" . fuchsia--json5-mode))

(provide 'fuchsia-json5)
