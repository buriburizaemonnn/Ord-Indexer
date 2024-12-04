#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]

mod address;
pub mod combined_txn;
pub mod multi_sender_txn;
pub mod runestone;
mod signer;
mod transaction;
mod utils;

pub use address::*;
use ic_cdk::api::management_canister::bitcoin::{
    bitcoin_get_current_fee_percentiles, GetCurrentFeePercentilesRequest,
};
pub use signer::ecdsa_sign;
pub use transaction::transfer;
pub use utils::*;

use crate::state::read_config;

pub async fn get_fee_per_vbyte() -> u64 {
    let network = read_config(|config| config.bitcoin_network());
    // Get fee percentiles from previous transactions to estimate our own fee.
    let fee_percentiles =
        bitcoin_get_current_fee_percentiles(GetCurrentFeePercentilesRequest { network })
            .await
            .unwrap()
            .0;

    if fee_percentiles.is_empty() {
        // There are no fee percentiles. This case can only happen on a regtest
        // network where there are no non-coinbase transactions. In this case,
        // we use a default of 2000 millisatoshis/byte (i.e. 2 satoshi/byte)
        2000
    } else {
        // Choose the 50th percentile for sending fees.
        fee_percentiles[50]
    }
}
