use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::Account;
use tiny_keccak::{Hasher, Sha3};

use crate::bitcoin::account_to_p2pkh_address;

#[derive(CandidType)]
pub struct Addresses {
    pub bitcoin: String,
    pub icrc1: Account,
}

pub fn principal_to_subaccount(principal: &Principal) -> [u8; 32] {
    let mut hash = [0; 32];
    let mut hasher = Sha3::v256();
    hasher.update(principal.as_slice());
    hasher.finalize(&mut hash);
    hash
}

pub fn generate_addresses_from_principal(principal: &Principal) -> Addresses {
    let canister_id = ic_cdk::id();
    let subaccount = principal_to_subaccount(principal);
    let account = Account {
        owner: canister_id,
        subaccount: Some(subaccount),
    };
    let bitcoin_address = account_to_p2pkh_address(&account);
    Addresses {
        icrc1: account,
        bitcoin: bitcoin_address,
    }
}

pub fn subaccount_with_num(num: u128) -> [u8; 32] {
    let mut hash = [8; 32];
    let mut hasher = Sha3::v256();
    hasher.update(ic_cdk::id().as_slice());
    hasher.update(&num.to_be_bytes());
    hasher.finalize(&mut hash);
    hash
}
