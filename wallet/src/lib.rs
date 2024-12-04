mod bitcoin;
mod ord_canister;
mod state;
mod transaction_handler;
mod types;
mod updater;
mod utils;

use std::{collections::HashMap, time::Duration};

use bitcoin::{
    account_to_p2pkh_address, combined_txn::CombinedTransactionRequest, get_fee_per_vbyte,
    multi_sender_txn::MultiSendTransactionArgument, runestone::RuneTransferArgs,
};
use candid::Principal;
// re export
use ic_cdk::{
    api::management_canister::{
        bitcoin::{bitcoin_get_balance, BitcoinNetwork, GetBalanceRequest},
        ecdsa::{
            ecdsa_public_key, EcdsaKeyId, EcdsaPublicKeyArgument,
            EcdsaPublicKeyResponse as EcdsaPublicKey,
        },
    },
    init, post_upgrade, pre_upgrade, query, update,
};
use icrc_ledger_types::icrc1::account::Account;
use state::{read_config, read_utxo_manager, write_config};
use transaction_handler::SubmittedTransactionIdType;
use types::RuneId;
use updater::TargetType;
use utils::{generate_addresses_from_principal, subaccount_with_num, Addresses};

async fn lazy_ecdsa_setup() {
    let ecdsa_keyid: EcdsaKeyId = read_config(|config| config.ecdsakeyid());
    let ecdsa_response = ecdsa_public_key(EcdsaPublicKeyArgument {
        canister_id: None,
        derivation_path: vec![],
        key_id: ecdsa_keyid,
    })
    .await
    .expect("Failed to get ecdsa key")
    .0;

    write_config(|config| {
        let mut temp = config.get().clone();
        temp.ecdsa_public_key = Some(ecdsa_response);
        let _ = config.set(temp);
    });
}

#[init]
pub fn init(bitcoin_network: BitcoinNetwork) {
    let keyname = match bitcoin_network {
        BitcoinNetwork::Mainnet => "key_1".to_string(),
        BitcoinNetwork::Testnet => "test_key_1".to_string(),
        BitcoinNetwork::Regtest => "dfx_test_key".to_string(),
    };
    write_config(|config| {
        let mut temp = config.get().clone();
        temp.keyname.replace(keyname);
        temp.bitcoin_network.replace(bitcoin_network);
        let _ = config.set(temp);
    });
    ic_cdk_timers::set_timer(Duration::from_secs(0), || ic_cdk::spawn(lazy_ecdsa_setup()));
}

#[pre_upgrade]
pub fn pre_upgrade() {}

#[post_upgrade]
pub fn post_upgrade() {}

#[update]
pub async fn withdraw_bitcoin(
    to: String,
    amount: u64,
    fee_per_vbytes: Option<u64>,
) -> SubmittedTransactionIdType {
    let caller = ic_cdk::caller();
    let addresses = generate_addresses_from_principal(&caller);
    let to = bitcoin::address_validation(&to).unwrap();
    let from = bitcoin::address_validation(&addresses.bitcoin).unwrap();
    let mut utxo_synced = false;
    let mut current_balance =
        read_utxo_manager(|manager| manager.get_bitcoin_balance(&addresses.bitcoin));
    if current_balance < amount {
        utxo_synced = true;
        updater::fetch_utxos_and_update_balances(
            &addresses.bitcoin,
            TargetType::Bitcoin { target: amount },
        )
        .await;
        current_balance =
            read_utxo_manager(|manager| manager.get_bitcoin_balance(&addresses.bitcoin));
        if current_balance < amount {
            ic_cdk::trap("not enough balance")
        }
    }
    let fee_per_vbytes = match fee_per_vbytes {
        None => get_fee_per_vbyte().await,
        Some(fee) => fee,
    };
    let txn = match bitcoin::transfer(
        &addresses.bitcoin,
        addresses.icrc1,
        from.clone(),
        to.clone(),
        amount,
        true,
        fee_per_vbytes,
    ) {
        Err(required_value) => {
            if utxo_synced && required_value < current_balance {
                ic_cdk::trap("not enough balance")
            }
            updater::fetch_utxos_and_update_balances(
                &addresses.bitcoin,
                TargetType::Bitcoin {
                    target: required_value,
                },
            )
            .await;
            if let Ok(txn) = bitcoin::transfer(
                &addresses.bitcoin,
                addresses.icrc1,
                from,
                to,
                amount,
                true,
                fee_per_vbytes,
            ) {
                txn
            } else {
                ic_cdk::trap("not enough balance")
            }
        }
        Ok(txn) => txn,
    };
    txn.build_and_submit().await.expect("should submit the txn")
}

#[update]
pub async fn withdraw_bitcoin_from_multiple_addresses(
    principal0: Principal,
    to: String,
    amount: u64,
    fee_per_vbytes: Option<u64>,
) -> SubmittedTransactionIdType {
    let caller = ic_cdk::caller();
    let (amount0, amount1) = {
        let is_even = amount % 2 == 0;
        if is_even {
            let amount_in_half = amount / 2;
            (amount_in_half, amount_in_half)
        } else {
            let amount_in_half = (amount - 1) / 2;
            (amount_in_half + 1, amount_in_half)
        }
    };
    let addresses0 = generate_addresses_from_principal(&principal0);
    let addresses1 = generate_addresses_from_principal(&caller);
    let address0 = bitcoin::address_validation(&addresses0.bitcoin).unwrap();
    let address1 = bitcoin::address_validation(&addresses1.bitcoin).unwrap();
    let to = bitcoin::address_validation(&to).unwrap();
    let fee_per_vbytes = match fee_per_vbytes {
        None => get_fee_per_vbyte().await,
        Some(fee) => fee,
    };
    let (mut utxo_synced0, mut utxo_synced1) = (false, false);
    let (mut current_balance0, mut current_balance1) = read_utxo_manager(|manager| {
        let balance0 = manager.get_bitcoin_balance(&addresses0.bitcoin);
        let balance1 = manager.get_bitcoin_balance(&addresses1.bitcoin);
        (balance0, balance1)
    });
    if current_balance0 < amount0 {
        utxo_synced0 = true;
        updater::fetch_utxos_and_update_balances(
            &addresses0.bitcoin,
            TargetType::Bitcoin { target: amount0 },
        )
        .await;
    }
    if current_balance1 < amount1 {
        utxo_synced1 = true;
        updater::fetch_utxos_and_update_balances(
            &addresses1.bitcoin,
            TargetType::Bitcoin { target: amount1 },
        )
        .await;
    }
    read_utxo_manager(|manager| {
        current_balance0 = manager.get_bitcoin_balance(&addresses0.bitcoin);
        current_balance1 = manager.get_bitcoin_balance(&addresses1.bitcoin);
    });
    if current_balance0 < amount0 || current_balance1 < amount1 {
        ic_cdk::trap("not enough balance")
    }
    let txn = match bitcoin::multi_sender_txn::transfer(MultiSendTransactionArgument {
        addr0: &addresses0.bitcoin,
        addr1: &addresses1.bitcoin,
        address0: address0.clone(),
        address1: address1.clone(),
        account0: addresses0.icrc1,
        account1: addresses1.icrc1,
        amount1,
        amount0,
        paid_by_sender: true,
        receiver: to.clone(),
        fee_per_vbytes,
    }) {
        Ok(txn) => txn,
        Err((required_amount0, required_amount1)) => {
            if required_amount0 > current_balance0 && !utxo_synced0 {
                updater::fetch_utxos_and_update_balances(
                    &addresses0.bitcoin,
                    TargetType::Bitcoin {
                        target: required_amount0,
                    },
                )
                .await;
            }
            if required_amount1 > current_balance1 && !utxo_synced1 {
                updater::fetch_utxos_and_update_balances(
                    &addresses1.bitcoin,
                    TargetType::Bitcoin {
                        target: required_amount1,
                    },
                )
                .await;
            }
            read_utxo_manager(|manager| {
                current_balance0 = manager.get_bitcoin_balance(&addresses0.bitcoin);
                current_balance1 = manager.get_bitcoin_balance(&addresses1.bitcoin);
            });
            if current_balance0 < required_amount0 || current_balance1 < required_amount1 {
                ic_cdk::trap("not enough balance")
            }
            if let Ok(txn) = bitcoin::multi_sender_txn::transfer(MultiSendTransactionArgument {
                addr0: &addresses0.bitcoin,
                addr1: &addresses1.bitcoin,
                address0,
                address1,
                account0: addresses0.icrc1,
                account1: addresses1.icrc1,
                amount1,
                amount0,
                paid_by_sender: true,
                receiver: to,
                fee_per_vbytes,
            }) {
                txn
            } else {
                ic_cdk::trap("not enough balance")
            }
        }
    };
    txn.build_and_submit().await.expect("failed to submit txn")
}

#[update]
pub async fn withdraw_runestone(
    runeid: RuneId,
    amount: u128,
    to: String,
    fee_per_vbytes: Option<u64>,
) -> SubmittedTransactionIdType {
    let caller = ic_cdk::caller();
    let sender_addresses = generate_addresses_from_principal(&caller);

    let sender = bitcoin::address_validation(&sender_addresses.bitcoin).unwrap();
    let receiver = bitcoin::address_validation(&to).unwrap();
    let fee_per_vbytes = match fee_per_vbytes {
        None => get_fee_per_vbyte().await,
        Some(fee) => fee,
    };

    let mut utxo_synced = false;
    let mut current_rune_balance = read_utxo_manager(|manager| {
        manager.get_runestone_balance(&sender_addresses.bitcoin, &runeid)
    });

    if current_rune_balance < amount {
        utxo_synced = true;
        updater::fetch_utxos_and_update_balances(
            &sender_addresses.bitcoin,
            TargetType::Bitcoin { target: u64::MAX },
        )
        .await;
        current_rune_balance = read_utxo_manager(|manager| {
            manager.get_runestone_balance(&sender_addresses.bitcoin, &runeid)
        });

        if current_rune_balance < amount {
            ic_cdk::trap("not enough balance")
        }
    }
    let txn = match bitcoin::runestone::transfer(RuneTransferArgs {
        runeid: runeid.clone(),
        amount,
        sender_addr: &sender_addresses.bitcoin,
        receiver_addr: &to,
        sender_account: sender_addresses.icrc1,
        receiver_account: sender_addresses.icrc1, // sender is the fee payer
        sender_address: sender.clone(),
        receiver_address: receiver.clone(),
        paid_by_sender: true,
        fee_per_vbytes,
        postage: None,
    }) {
        Ok(txn) => txn,
        Err((_, fee)) => {
            // ignoring the rune amount, as it is checked earlier
            let mut current_btc_balance =
                read_utxo_manager(|manager| manager.get_bitcoin_balance(&sender_addresses.bitcoin));
            if fee > current_btc_balance && !utxo_synced {
                updater::fetch_utxos_and_update_balances(
                    &sender_addresses.bitcoin,
                    TargetType::Bitcoin { target: u64::MAX },
                )
                .await;
                current_btc_balance = read_utxo_manager(|manager| {
                    manager.get_bitcoin_balance(&sender_addresses.bitcoin)
                });
                if current_btc_balance < fee {
                    ic_cdk::trap("not enough balance")
                }
            }
            if let Ok(txn) = bitcoin::runestone::transfer(RuneTransferArgs {
                runeid,
                amount,
                sender_addr: &sender_addresses.bitcoin,
                receiver_addr: &to,
                sender_account: sender_addresses.icrc1,
                receiver_account: sender_addresses.icrc1, // sender is the fee payer
                sender_address: sender,
                receiver_address: receiver,
                paid_by_sender: true,
                fee_per_vbytes,
                postage: None,
            }) {
                txn
            } else {
                ic_cdk::trap("not enough balance")
            }
        }
    };
    txn.build_and_submit().await.unwrap()
}

#[update]
pub async fn withdraw_runestone_with_fee_paid_by_receiver(
    runeid: RuneId,
    amount: u128,
    to: Principal,
    fee_per_vbytes: Option<u64>,
) -> SubmittedTransactionIdType {
    let caller = ic_cdk::caller();
    let sender_addresses = generate_addresses_from_principal(&caller);
    let receiver_addresses = generate_addresses_from_principal(&to);

    let sender = bitcoin::address_validation(&sender_addresses.bitcoin).unwrap();
    let receiver = bitcoin::address_validation(&receiver_addresses.bitcoin).unwrap();

    let (mut current_rune_balance, mut current_btc_balance) = read_utxo_manager(|manager| {
        (
            manager.get_runestone_balance(&sender_addresses.bitcoin, &runeid),
            manager.get_bitcoin_balance(&receiver_addresses.bitcoin),
        )
    });

    if current_rune_balance < amount {
        updater::fetch_utxos_and_update_balances(
            &sender_addresses.bitcoin,
            TargetType::Bitcoin { target: u64::MAX },
        )
        .await;
        current_rune_balance = read_utxo_manager(|manager| {
            manager.get_runestone_balance(&sender_addresses.bitcoin, &runeid)
        });

        if current_rune_balance < amount {
            ic_cdk::trap("not enough balance")
        }
    }

    let fee_per_vbytes = match fee_per_vbytes {
        None => get_fee_per_vbyte().await,
        Some(fee) => fee,
    };

    let txn = match bitcoin::runestone::transfer(RuneTransferArgs {
        runeid: runeid.clone(),
        amount,
        sender_addr: &sender_addresses.bitcoin,
        receiver_addr: &receiver_addresses.bitcoin,
        sender_address: sender.clone(),
        receiver_address: receiver.clone(),
        sender_account: sender_addresses.icrc1,
        receiver_account: receiver_addresses.icrc1,
        fee_per_vbytes,
        paid_by_sender: true,
        postage: None,
    }) {
        Ok(txn) => txn,
        Err((_, fee)) => {
            if fee > current_btc_balance {
                updater::fetch_utxos_and_update_balances(
                    &receiver_addresses.bitcoin,
                    TargetType::Bitcoin { target: u64::MAX },
                )
                .await;
                current_btc_balance = read_utxo_manager(|manager| {
                    manager.get_bitcoin_balance(&receiver_addresses.bitcoin)
                });
                if current_btc_balance < fee {
                    ic_cdk::trap("not enough balance")
                }
            }

            if let Ok(txn) = bitcoin::runestone::transfer(RuneTransferArgs {
                runeid,
                amount,
                sender_addr: &sender_addresses.bitcoin,
                receiver_addr: &receiver_addresses.bitcoin,
                sender_address: sender,
                receiver_address: receiver,
                sender_account: sender_addresses.icrc1,
                receiver_account: receiver_addresses.icrc1,
                fee_per_vbytes,
                paid_by_sender: true,
                postage: None,
            }) {
                txn
            } else {
                ic_cdk::trap("not enough balance")
            }
        }
    };
    txn.build_and_submit().await.unwrap()
}

#[update]
pub async fn withdraw_combined(
    runeid: RuneId,
    rune_amount: u128,
    btc_amount: u64,
    receiver_principal: Principal,
    fee_per_vbytes: Option<u64>,
) -> SubmittedTransactionIdType {
    let caller = ic_cdk::caller();
    let addresses = generate_addresses_from_principal(&caller);
    let receiver_addresses = generate_addresses_from_principal(&receiver_principal);
    let sender_address = bitcoin::address_validation(&addresses.bitcoin).unwrap();
    let receiver_address = bitcoin::address_validation(&receiver_addresses.bitcoin).unwrap();

    updater::fetch_utxos_and_update_balances(
        &addresses.bitcoin,
        TargetType::Bitcoin { target: u64::MAX },
    )
    .await;

    updater::fetch_utxos_and_update_balances(
        &receiver_addresses.bitcoin,
        TargetType::Bitcoin { target: u64::MAX },
    )
    .await;

    let fee_per_vbytes = match fee_per_vbytes {
        None => get_fee_per_vbyte().await,
        Some(fee) => fee,
    };
    let txn = bitcoin::combined_txn::transfer(CombinedTransactionRequest {
        from_addr: &addresses.bitcoin,
        receiver_addr: &receiver_addresses.bitcoin,
        sender_address,
        receiver_address,
        sender_account: addresses.icrc1,
        receiver_account: receiver_addresses.icrc1,
        runeid,
        rune_amount,
        btc_amount,
        postage: None,
        paid_by_sender: false,
        fee_per_vbytes,
    })
    .unwrap();
    txn.build_and_submit().await.unwrap()
}

#[query]
pub fn get_deposit_addresses() -> Addresses {
    let caller = ic_cdk::caller();
    generate_addresses_from_principal(&caller)
}

#[query]
pub fn generate_address(num: u128) -> String {
    let subaccount = subaccount_with_num(num);
    let account = Account {
        owner: ic_cdk::id(),
        subaccount: Some(subaccount),
    };
    account_to_p2pkh_address(&account)
}

#[update]
pub async fn get_bitcoin_balance_of(of: String) -> u64 {
    let network = read_config(|config| config.bitcoin_network());
    bitcoin_get_balance(GetBalanceRequest {
        address: of.to_string(),
        network,
        min_confirmations: None,
    })
    .await
    .unwrap()
    .0
}

#[update]
pub async fn get_runestone_balance_of(of: String) -> HashMap<RuneId, u128> {
    updater::fetch_utxos_and_update_balances(&of, TargetType::Bitcoin { target: u64::MAX }).await;
    read_utxo_manager(|manager| manager.all_rune_with_balances(&of))
}

ic_cdk::export_candid!();
