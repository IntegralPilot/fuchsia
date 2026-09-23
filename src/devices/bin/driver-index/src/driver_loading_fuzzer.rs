// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::resolved_driver::ResolvedDriver;
use fuchsia_async as fasync;
use futures::SinkExt;
use rand::rngs::SmallRng;
use rand::seq::SliceRandom as _;
use rand::{Rng as _, SeedableRng as _};

pub struct Session {
    sender: futures::channel::mpsc::UnboundedSender<Vec<ResolvedDriver>>,
    max_delay: zx::MonotonicDuration,
    shuffled_boot_drivers: Vec<ResolvedDriver>,
    rng: SmallRng,
}

impl Session {
    pub fn new(
        sender: futures::channel::mpsc::UnboundedSender<Vec<ResolvedDriver>>,
        mut boot_drivers: Vec<ResolvedDriver>,
        max_delay: zx::MonotonicDuration,
        seed: Option<u64>,
    ) -> Session {
        let seed_val = seed.unwrap_or_else(rand::random::<u64>);
        log::info!("Driver loading fuzzer enabled with RNG seed: {}", seed_val);

        let mut rng = SmallRng::seed_from_u64(seed_val);
        boot_drivers.shuffle(&mut rng);
        Session {
            sender: sender,
            max_delay: max_delay,
            shuffled_boot_drivers: boot_drivers,
            rng: rng,
        }
    }

    pub async fn run(mut self) {
        if self.max_delay.into_millis() == 0 {
            push_drivers(&self.sender, self.shuffled_boot_drivers).await;
            return;
        }

        let max_load_delay = self.max_delay / self.shuffled_boot_drivers.len() as i64;

        let mut driver_buffer: Vec<ResolvedDriver> = vec![];
        for driver in self.shuffled_boot_drivers.into_iter() {
            // Add a 30% chance of injecting a delay between driver loads.
            let should_delay = (self.rng.next_u32() % 10) < 3;
            if should_delay {
                push_drivers(&self.sender, driver_buffer).await;
                driver_buffer = vec![];

                // Generate a delay between [0, max_load_delay).
                let delay = if max_load_delay.into_millis() == 0 {
                    max_load_delay
                } else {
                    zx::MonotonicDuration::from_millis(
                        (self.rng.next_u32() as i64) % max_load_delay.into_millis(),
                    )
                };

                fasync::Timer::new(delay).await;
            }

            driver_buffer.push(driver);
        }
        push_drivers(&self.sender, driver_buffer).await;
    }
}

async fn push_drivers(
    mut sender: &futures::channel::mpsc::UnboundedSender<Vec<ResolvedDriver>>,
    drivers: Vec<ResolvedDriver>,
) {
    if let Err(e) = sender.send(drivers).await {
        log::error!("Failed to send drivers to the Indexer: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolved_driver::DriverPackageType;
    use bind::interpreter::decode_bind_rules::DecodedRules;
    use futures::StreamExt as _;

    fn make_fake_boot_driver(name: &str) -> ResolvedDriver {
        let test_rules = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: std::collections::HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let decoded_rules = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(test_rules).unwrap(),
        )
        .unwrap();
        ResolvedDriver {
            component_url: cm_types::Url::new(
                &format!("fuchsia-boot:///#meta/{}.cm", name).to_owned(),
            )
            .unwrap(),
            bind_rules: decoded_rules,
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Boot,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        }
    }

    #[fuchsia::test]
    async fn test_driver_load() {
        let mut test_boot_repo = vec![];
        for i in 0..10 {
            test_boot_repo.push(make_fake_boot_driver(format!("driver-{}", i).as_str()));
        }

        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<Vec<ResolvedDriver>>();

        let test_seed = 0;
        let session = Session::new(
            sender,
            test_boot_repo.clone(),
            zx::MonotonicDuration::from_millis(10),
            Some(test_seed),
        );
        session.run().await;

        let mut received_drivers = vec![];
        while let Some(drivers) = receiver.next().await {
            received_drivers.extend(drivers.into_iter());
        }

        for driver in test_boot_repo {
            assert!(received_drivers.contains(&driver));
        }
    }
}
