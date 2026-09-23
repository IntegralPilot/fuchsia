// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

macro_rules! make_cases {
    ($case_suffix:ident) => {
        paste::paste! {
            #[test]
            fn [<case_ $case_suffix>]() {
                let case_name = stringify!([<case_ $case_suffix>]);
                println!("ran {}::{}.", std::module_path!(), case_name);
                #[cfg(make_case_f_fail)]
                {
                    if case_name == "case_f" {
                        panic!("making case_f fail");
                    }
                }
            }
        }
    };
    ($case_suffix:ident, $($others:ident),+) => {
        make_cases! { $case_suffix }

        make_cases! { $($others),* }
    };
}

macro_rules! make_sections {
    ($section_suffix:expr) => {
        paste::paste! {
            mod [<section_ $section_suffix>] {
                make_cases! { a, b, c, d, e, f, g, h, i, j, k, l }
            }
        }
    };
    ($section_suffix:expr, $($others:expr),+) => {
        make_sections! { $section_suffix }

        make_sections! { $($others),* }
    };
}

make_sections! { 1, 2, 3, 4, 5, 6, 7, 8, 9 }
