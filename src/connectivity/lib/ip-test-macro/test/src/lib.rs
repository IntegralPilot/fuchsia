// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// `ip_test` looks for `Ipv4` and `Ipv6` within `net_types::ip`, so we create the same hierarchy
/// in this test.
#[cfg(test)]
pub(crate) mod net_types {
    pub(crate) mod ip {
        /// Simple trait with the identifier `Ip` to test that the `ip_test`
        /// attribute specializes function for the various types that implement
        /// this trait.
        pub(crate) trait Ip {
            type Addr: IpAddress;
            const VERSION: usize;
        }

        /// Helper trait for defining an associated type on `Ip`.
        pub(crate) trait IpAddress {
            fn new() -> Self;
        }

        /// Test type with the identifier `Ipv4` that the `ip_test` attribute
        /// can specialize a function for.
        pub(crate) struct Ipv4;
        impl Ip for Ipv4 {
            type Addr = Ipv4Addr;
            const VERSION: usize = 4;
        }

        /// Test type with the identifier `Ipv6` that the `ip_test` attribute
        /// can specialize a function for.
        pub(crate) struct Ipv6;
        impl Ip for Ipv6 {
            type Addr = Ipv6Addr;
            const VERSION: usize = 6;
        }

        /// Helper to implement `Ip` for `Ipv4`.
        pub(crate) struct Ipv4Addr;
        impl IpAddress for Ipv4Addr {
            fn new() -> Self {
                Self
            }
        }

        /// Helper to implement `Ip` for `Ipv6`.
        pub(crate) struct Ipv6Addr;
        impl IpAddress for Ipv6Addr {
            fn new() -> Self {
                Self
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::net_types::ip::{Ip, IpAddress, Ipv4, Ipv6};
    use ip_test_macro::ip_test;

    trait IpExt: Ip {
        const SIMPLE_VALUE: u8;
    }

    impl IpExt for Ipv4 {
        const SIMPLE_VALUE: u8 = 1;
    }

    impl IpExt for Ipv6 {
        const SIMPLE_VALUE: u8 = 2;
    }

    fn simple_specialized_for_ip<I: IpExt>() -> u8 {
        I::SIMPLE_VALUE
    }

    #[ip_test(I, net_types = "crate::net_types")]
    fn test_ip_test<I: IpExt>() {
        let x = simple_specialized_for_ip::<I>();
        if I::VERSION == 4 {
            assert!(x == 1);
        } else {
            assert!(x == 2);
        }
    }

    // test that all the `ip_test` functions were generated
    #[test]
    fn test_all_tests_generation() {
        test_ip_test_v4();
        test_ip_test_v6();
    }

    #[ip_test(I, net_types = "crate::net_types")]
    #[should_panic]
    fn test_should_panic<I: IpExt>() {
        assert_eq!(0, simple_specialized_for_ip::<I>());
    }

    mod real_test_case {
        use ::test_case::test_case;

        use super::*;

        #[ip_test(I, net_types = "crate::net_types")]
        #[test_case(true, "bool")]
        #[test_case(1usize, "usize")]
        #[test_case(I::VERSION, "usize")]
        fn test_with_test_case<I: Ip, A: core::fmt::Debug>(_arg: A, name: &str) {
            assert!(std::any::type_name::<A>().contains(name))
        }

        fn templated_fn<I>() -> &'static str {
            std::any::type_name::<I>()
        }
        #[ip_test(I, net_types = "crate::net_types")]
        #[test_case(std::any::type_name::<I>(), "Ipv")]
        #[test_case(Some(templated_fn::<I>()), "Ipv"; "with_name")]
        fn test_with_i_as_template_param<I: Ip>(full_name: impl core::fmt::Debug, name: &str) {
            assert!(format!("{:?}", full_name).contains(name))
        }

        #[ip_test(I, net_types = "crate::net_types")]
        #[test_case(Some(Some(I::Addr::new())))]
        fn test_with_i_in_arg_type<I: Ip>(addr: Option<Option<I::Addr>>) {
            let _: I::Addr = addr.unwrap().unwrap();
        }

        fn produce_default_addr<I: Ip>() -> I::Addr {
            <I::Addr as IpAddress>::new()
        }

        #[ip_test(I, net_types = "crate::net_types")]
        #[test_case(produce_default_addr::<I>)]
        fn test_with_i_in_other_type_trait_bounds<I: Ip, F: FnOnce() -> I::Addr>(f: F) {
            let _addr = f();
        }
    }

    mod fake_test_case {
        use ::fake_test_case::test_case;

        use super::*;

        /// The #[ip_test] macro can't distinguish between the real #[test_case]
        /// macro and the fake one used here. Use the fake one, which inserts
        /// the test name in the argument list, to verify that the test name
        /// is being passed along with the arguments by the #[ip_test] macro.
        #[ip_test(I, net_types = "crate::net_types")]
        #[test_case(123, 456; "name_from_macro")]
        fn case_with_name<I: Ip>(a: u8, b: u16, name: &str) {
            assert_eq!((a, b), (123, 456));
            assert_eq!(name, "name_from_macro");
        }
    }
}
