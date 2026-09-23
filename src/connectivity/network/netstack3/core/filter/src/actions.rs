// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use netstack3_base::Mark;

/// An action that can be applied to a mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkAction {
    /// This action sets the mark specified by the `mark` and `clearing_mask`.
    ///
    /// If the current mark is [`None`], it sets it to `mark`.
    SetMark {
        /// This value is combined with the result of the masking using a
        /// bitwise OR to get the final mark.
        mark: u32,
        /// The bits specified by this mask are cleared out from the existing
        /// mark.
        clearing_mask: u32,
    },
}

impl MarkAction {
    /// Applies the change to the given [`Mark`].
    pub fn apply(self, mark: &mut Mark) {
        let Mark(cur) = mark;
        match self {
            MarkAction::SetMark { clearing_mask, mark } => {
                *cur = Some(match cur {
                    Some(v) => (*v & !clearing_mask) | mark,
                    None => mark,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(Mark(None), MarkAction::SetMark { mark: 1, clearing_mask: 0 } => Mark(Some(1))
            ; "set new mark")]
    #[test_case(Mark(Some(1)), MarkAction::SetMark { mark: 2, clearing_mask: 1 } => Mark(Some(2))
            ; "clear with mask")]
    #[test_case(Mark(Some(1)), MarkAction::SetMark { mark: 2, clearing_mask: 0 } => Mark(Some(3))
            ; "or mark")]
    fn apply_mark_action(mut mark: Mark, action: MarkAction) -> Mark {
        action.apply(&mut mark);
        mark
    }
}
