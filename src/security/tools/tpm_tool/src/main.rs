// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use tpm_device::tpm::Tpm;

fn main() -> Result<(), Error> {
    let tpm = Tpm::try_new()?;
    println!("Tpm2.0 Diagnostics:");
    println!("-------------------");
    println!("Tpm Properties:");
    println!("{:#?}", tpm.manufacturer_property().unwrap());
    println!("{:#?}", tpm.permanent_property().unwrap());
    println!("{:#?}", tpm.startup_clear_property().unwrap());

    println!("Random Numbers:");
    println!("StirRandom: {:?}", tpm.stir_random(vec![1, 2, 3]));
    println!("GetRandom: {:?}", tpm.get_random(8));

    println!("Provisioning:");
    println!("TakeOwnership: {:?}", tpm.take_ownership());

    Ok(())
}
