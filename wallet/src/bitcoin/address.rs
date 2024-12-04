use bitcoin::{address::NetworkUnchecked, Address};
use icrc_ledger_types::icrc1::account::Account;

use crate::{bitcoin::utils::derive_public_key, state::read_config};

use bitcoin::Network;
use ic_cdk::api::management_canister::bitcoin::BitcoinNetwork as IcBitcoinNetwork;

use super::utils::{account_to_derivation_path, ripemd160, sha256};

pub fn address_validation(addr: &str) -> Result<Address, String> {
    read_config(|config| {
        let bitcoin_network = match config.bitcoin_network() {
            IcBitcoinNetwork::Mainnet => Network::Bitcoin,
            IcBitcoinNetwork::Testnet => Network::Testnet,
            IcBitcoinNetwork::Regtest => Network::Regtest,
        };
        let parsed_addr: Address<NetworkUnchecked> = match addr.parse() {
            Err(_e) => return Err(String::from("failed to parse into bitcoin address")),
            Ok(addr) => addr,
        };
        if !parsed_addr.is_valid_for_network(bitcoin_network) {
            let msg = format!(
                "Invalid Address.\n{} isn't valid for {:?} network",
                addr, bitcoin_network
            );
            return Err(msg);
        }
        match parsed_addr.require_network(bitcoin_network) {
            Ok(addr) => Ok(addr),
            Err(_) => Err(String::from("Failed to validate with network")),
        }
    })
}

pub fn account_to_p2pkh_address(account: &Account) -> String {
    read_config(|config| {
        let prefix = match config.bitcoin_network() {
            IcBitcoinNetwork::Mainnet => 0x00,
            _ => 0x6f, // Regtest | Testnet
        };
        let ecdsa_public_key = config.ecdsa_public_key();
        let path = account_to_derivation_path(account);
        let derived_public_key = derive_public_key(&ecdsa_public_key, &path).public_key;
        let ripemd_pk = ripemd160(&sha256(&derived_public_key));
        let mut raw_address = vec![prefix];
        raw_address.extend(ripemd_pk);
        let checksum = &sha256(&sha256(&raw_address.clone()))[..4];
        raw_address.extend(checksum);
        bs58::encode(raw_address).into_string()
    })
}
