// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::context::LowpanCtlContext;
use crate::prelude::*;
use fidl_fuchsia_lowpan_device::{Credential, Identity, ProvisioningParams};

const PROVISION_CMD_NAME_LEN: usize = 63;
const PROVISION_CMD_CRED_MASTER_LEY_LEN: &[usize] = &[16, 32];

/// Contains the arguments decoded for the `provision` command.
#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "provision")]
pub struct ProvisionCommand {
    /// name
    #[argh(option)]
    pub name: String,

    /// extended PANID, input hex string
    #[argh(option)]
    pub xpanid: Option<String>,

    /// string identifying the type of network
    #[argh(option)]
    pub net_type: Option<String>,

    /// channel index
    #[argh(option)]
    pub channel: Option<u16>,

    /// panid for 802.14.5-based networks (or the equivalent)
    #[argh(option)]
    pub panid: Option<String>,

    /// credential master key
    #[argh(option)]
    pub credential_master_key: Option<String>,
}

impl ProvisionCommand {
    fn get_name_vec_from_str(&self) -> Result<Option<Vec<u8>>, Error> {
        if self.name.as_bytes().len() > PROVISION_CMD_NAME_LEN {
            return Err(format_err!("name should be less or equal to 63 bytes"));
        }
        Ok(Some(self.name.as_bytes().to_vec()))
    }

    fn get_panid_u16(&self) -> Result<Option<u16>, Error> {
        self.panid.as_ref().map(|value| Ok(u16::from_str_radix(value, 16)?)).transpose()
    }

    fn get_xpanid(&self) -> Result<Option<[u8; 8]>, Error> {
        self.xpanid
            .as_ref()
            .map(|value| {
                let vec = hex::decode(value)?;
                let octets = vec
                    .try_into()
                    .map_err(|_: Vec<u8>| format_err!("malformed xpanid {}", value))?;
                Ok(octets)
            })
            .transpose()
    }

    fn get_cred_master_key_vec_from_str(&self) -> Result<Option<Vec<u8>>, Error> {
        self.credential_master_key
            .as_ref()
            .map(|value| {
                let res = hex::decode(value)?;
                if res.len() != PROVISION_CMD_CRED_MASTER_LEY_LEN[0]
                    && res.len() != PROVISION_CMD_CRED_MASTER_LEY_LEN[1]
                {
                    return Err(format_err!("credential master key must be 16 or 32 bytes"));
                }
                Ok(res)
            })
            .transpose()
    }

    fn get_identity(&self) -> Result<Identity, Error> {
        Ok(Identity {
            raw_name: self.get_name_vec_from_str()?,
            xpanid: self.get_xpanid()?,
            net_type: self.net_type.clone(),
            channel: self.channel.clone(),
            panid: self.get_panid_u16()?,
            ..Default::default()
        })
    }

    fn get_credential(&self) -> Result<Option<Box<Credential>>, Error> {
        let cred_master_key_vec = self.get_cred_master_key_vec_from_str()?;
        Ok(cred_master_key_vec.map(|value| Box::new(Credential::NetworkKey(value))))
    }

    fn get_provisioning_params(&self) -> Result<ProvisioningParams, Error> {
        Ok(ProvisioningParams {
            identity: self.get_identity()?,
            credential: self.get_credential()?,
        })
    }

    pub async fn exec(&self, context: &mut LowpanCtlContext) -> Result<(), Error> {
        let provision_args = self.get_provisioning_params()?;
        let device = context.get_default_device().await.context("Unable to get device instance")?;
        device
            .provision_network(&provision_args)
            .await
            .context("Unable to send provision network command")?;
        Ok(())
    }
}
