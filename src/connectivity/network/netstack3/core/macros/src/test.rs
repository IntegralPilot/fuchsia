// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use net_types::ip::{Ip, IpVersion, Ipv4, Ipv6};
use netstack3_macros::{CounterCollection, instantiate_ip_impl_block};

#[test]
fn instantiate_ip_impl_block() {
    struct Foo {
        ipv4: bool,
        ipv6: bool,
    }

    trait TraitOnIp<I: Ip> {
        fn foo(&mut self);
    }

    #[instantiate_ip_impl_block(I)]
    impl<I> TraitOnIp<I> for Foo {
        fn foo(&mut self) {
            let version = I::VERSION;
            let as_version = <I as Ip>::VERSION;
            assert_eq!(version, as_version);
            let hit = match version {
                IpVersion::V4 => &mut self.ipv4,
                IpVersion::V6 => &mut self.ipv6,
            };
            assert!(!core::mem::replace(hit, true));
        }
    }

    let mut foo = Foo { ipv4: false, ipv6: false };
    TraitOnIp::<Ipv4>::foo(&mut foo);
    TraitOnIp::<Ipv6>::foo(&mut foo);

    let Foo { ipv4, ipv6 } = foo;
    assert!(ipv4);
    assert!(ipv6);
}

#[test]
fn derive_counter_collection() {
    // Derive the traits on a basic struct with only a `C` type parameter.
    #[derive(CounterCollection, Debug, Default, PartialEq)]
    struct Ipv4Counters<C: netstack3_base::CounterRepr = netstack3_base::Counter> {
        foo: C,
    }

    trait IpExt: Ip {
        type Counters: netstack3_base::CounterCollectionSpec + std::fmt::Debug + Default + PartialEq;
    }

    impl IpExt for Ipv4 {
        type Counters = Ipv4Counters;
    }

    // Derive the traits on a more complex struct with an auxiliary `I` type
    // parameter
    #[derive(CounterCollection, Debug, Default, PartialEq)]
    struct IpCounters<I: IpExt, C: netstack3_base::CounterRepr = netstack3_base::Counter> {
        bar: C,
        inner: <I::Counters as netstack3_base::CounterCollectionSpec>::CounterCollection<C>,
    }

    // Prove that the traits are implemented by calling a trait method.
    let counters: IpCounters<Ipv4, netstack3_base::Counter> = Default::default();
    let _: IpCounters<Ipv4, u64> = netstack3_base::CounterCollection::cast(&counters);
}

#[test]
fn derive_counter_collection_with_counter_attr() {
    #[derive(CounterCollection, Debug, Default, PartialEq)]
    #[counter(CounterType)]
    struct Counters<CounterType: netstack3_base::CounterRepr = netstack3_base::Counter> {
        foo: CounterType,
    }

    // Prove that the traits are implemented by calling a trait method.
    let counters: Counters<netstack3_base::Counter> = Default::default();
    let _: Counters<u64> = netstack3_base::CounterCollection::cast(&counters);
}

#[test]
fn derive_counter_collection_without_counterrepr_bound() {
    // Omit the `C: netstack3_base::CounterRepr` bound.
    #[derive(CounterCollection, Debug, Default, PartialEq)]
    struct Counters<C> {
        foo: C,
    }

    // Prove that the traits are implemented by calling a trait method.
    let counters: Counters<netstack3_base::Counter> = Default::default();
    let _: Counters<u64> = netstack3_base::CounterCollection::cast(&counters);
}
