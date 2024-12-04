use bitcoin::{
    absolute::LockTime,
    hashes::Hash,
    script::{Builder, PushBytesBuf},
    sighash::{EcdsaSighashType, SighashCache},
    transaction::Version,
    Address, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use candid::CandidType;
use ic_cdk::api::management_canister::bitcoin::{
    bitcoin_send_transaction, SendTransactionRequest, Utxo,
};
use ic_management_canister_types::DerivationPath;
use icrc_ledger_types::icrc1::account::Account;
use ordinals::{Edict, Runestone};

use crate::{
    bitcoin::{account_to_derivation_path, derive_public_key, ecdsa_sign, sec1_to_der},
    state::{read_config, RunicUtxo},
    types::RuneId,
};

pub enum TransactionType {
    Bitcoin {
        addr: String,
        utxos: Vec<Utxo>,
        signer_account: Account,
        signer_address: Address,
        txn: Transaction,
    },
    LegoBitcoin {
        addr0: String,
        addr1: String,
        account0: Account,
        account1: Account,
        address0: Address,
        address1: Address,
        utxos0: Vec<Utxo>,
        utxos1: Vec<Utxo>,
        amount0: u64,
        amount1: u64,
        fee: u64,
        paid_by_sender: bool,
        receiver: Address,
    },
    Runestone {
        sender_addr: String,
        receiver_addr: String,
        sender_account: Account,
        receiver_account: Account,
        runeid: RuneId,
        amount: u128,
        fee: u64,
        runic_utxos: Vec<RunicUtxo>,
        fee_utxos: Vec<Utxo>,
        paid_by_sender: bool,
        sender_address: Address,
        receiver_address: Address,
        postage: Amount,
    },
    Combined {
        sender_addr: String,
        receiver_addr: String,
        sender_address: Address,
        receiver_address: Address,
        sender_account: Account,
        receiver_account: Account,
        runic_utxos: Vec<RunicUtxo>,
        btc_utxos: Vec<Utxo>,
        fee_utxos: Vec<Utxo>,
        runeid: RuneId,
        rune_amount: u128,
        btc_amount: u64,
        fee: u64,
        postage: Amount,
        paid_by_sender: bool,
    },
}

#[derive(CandidType)]
pub enum SubmittedTransactionIdType {
    Bitcoin { txid: String },
}

impl TransactionType {
    pub async fn build_and_submit(&self) -> Option<SubmittedTransactionIdType> {
        match self {
            Self::Bitcoin {
                addr: _,
                utxos: _,
                signer_account,
                signer_address,
                txn,
            } => {
                let mut txn = txn.clone();
                let (path, pubkey) = read_config(|config| {
                    let ecdsa_key = config.ecdsa_public_key();
                    let path = account_to_derivation_path(signer_account);
                    let pubkey = derive_public_key(&ecdsa_key, &path).public_key;
                    (DerivationPath::new(path), pubkey)
                });
                let txn_cache = SighashCache::new(txn.clone());
                for (index, input) in txn.input.iter_mut().enumerate() {
                    let sighash = txn_cache
                        .legacy_signature_hash(
                            index,
                            &signer_address.script_pubkey(),
                            EcdsaSighashType::All.to_u32(),
                        )
                        .unwrap();
                    let signature = ecdsa_sign(
                        sighash.to_raw_hash().to_byte_array().to_vec(),
                        path.clone().into_inner(),
                    )
                    .await
                    .signature;
                    let mut signature = sec1_to_der(signature);
                    signature.push(EcdsaSighashType::All.to_u32() as u8);
                    let signature = PushBytesBuf::try_from(signature).unwrap();
                    let pubkey = PushBytesBuf::try_from(pubkey.clone()).unwrap();
                    input.script_sig = Builder::new()
                        .push_slice(signature)
                        .push_slice(pubkey)
                        .into_script();
                    input.witness.clear();
                }
                let txid = txn.compute_txid().to_string();
                let txn_bytes = bitcoin::consensus::serialize(&txn);
                ic_cdk::println!("{}", hex::encode(&txn_bytes));
                bitcoin_send_transaction(SendTransactionRequest {
                    transaction: txn_bytes,
                    network: read_config(|config| config.bitcoin_network()),
                })
                .await
                .unwrap();
                Some(SubmittedTransactionIdType::Bitcoin { txid })
            }
            Self::LegoBitcoin {
                addr0: _,
                addr1: _,
                account0,
                account1,
                address0,
                address1,
                utxos0,
                utxos1,
                amount0,
                amount1,
                fee,
                paid_by_sender,
                receiver,
            } => {
                const DUST_THRESHOLD: u64 = 1_000;
                let mut input = Vec::with_capacity(utxos0.len() + utxos1.len());
                let mut index_of_utxos_of_addr0 = vec![];
                let mut index_of_utxos_of_addr1 = vec![];
                let (mut total_spent0, mut total_spent1) = (0, 0);

                utxos0.iter().for_each(|utxo| {
                    let txin = TxIn {
                        sequence: Sequence::MAX,
                        script_sig: ScriptBuf::new(),
                        witness: Witness::new(),
                        previous_output: OutPoint {
                            txid: Txid::from_raw_hash(
                                Hash::from_slice(&utxo.outpoint.txid).expect("should return hash"),
                            ),
                            vout: utxo.outpoint.vout,
                        },
                    };
                    total_spent0 += utxo.value;
                    let current_len = input.len();
                    input.insert(current_len, txin);
                    index_of_utxos_of_addr0.push(current_len);
                });
                utxos1.iter().for_each(|utxo| {
                    let txin = TxIn {
                        sequence: Sequence::MAX,
                        script_sig: ScriptBuf::new(),
                        witness: Witness::new(),
                        previous_output: OutPoint {
                            txid: Txid::from_raw_hash(
                                Hash::from_slice(&utxo.outpoint.txid).expect("should return hash"),
                            ),
                            vout: utxo.outpoint.vout,
                        },
                    };
                    total_spent1 += utxo.value;
                    let current_len = input.len();
                    input.insert(current_len, txin);
                    index_of_utxos_of_addr1.push(current_len);
                });

                let mut output = vec![TxOut {
                    script_pubkey: receiver.script_pubkey(),
                    value: if *paid_by_sender {
                        Amount::from_sat(amount0 + amount1)
                    } else {
                        Amount::from_sat(amount0 + amount1 - fee)
                    },
                }];

                // block responsible for calculating and adding remaining account
                {
                    let (fee0, fee1) = {
                        let is_even = fee % 2 == 0;
                        if is_even {
                            let fee_in_half = fee / 2;
                            (fee_in_half, fee_in_half)
                        } else {
                            let fee_in_half = (fee - 1) / 2;
                            (fee_in_half, fee_in_half + 1)
                        }
                    };
                    let (amount0, amount1) = if *paid_by_sender {
                        (amount0 + fee0, amount1 + fee1)
                    } else {
                        (*amount0, *amount1)
                    };
                    let remaining0 = total_spent0 - amount0;
                    if remaining0 > DUST_THRESHOLD {
                        output.push(TxOut {
                            script_pubkey: address0.script_pubkey(),
                            value: Amount::from_sat(remaining0),
                        });
                    }
                    let remaining1 = total_spent1 - amount1;
                    if remaining1 > DUST_THRESHOLD {
                        output.push(TxOut {
                            script_pubkey: address1.script_pubkey(),
                            value: Amount::from_sat(remaining1),
                        })
                    }
                }

                let mut txn = Transaction {
                    input,
                    output,
                    lock_time: LockTime::ZERO,
                    version: Version(2),
                };

                // signing the transaction

                let (path0, pubkey0, path1, pubkey1) = read_config(|config| {
                    let ecdsa_key = config.ecdsa_public_key();
                    let path0 = account_to_derivation_path(account0);
                    let path1 = account_to_derivation_path(account1);
                    let pubkey0 = derive_public_key(&ecdsa_key, &path0).public_key;
                    let pubkey1 = derive_public_key(&ecdsa_key, &path1).public_key;
                    (
                        DerivationPath::new(path0),
                        pubkey0,
                        DerivationPath::new(path1),
                        pubkey1,
                    )
                });
                let txn_cache = SighashCache::new(txn.clone());
                for (i, input) in txn.input.iter_mut().enumerate() {
                    if index_of_utxos_of_addr0.contains(&i) {
                        let sighash = txn_cache
                            .legacy_signature_hash(
                                i,
                                &address0.script_pubkey(),
                                EcdsaSighashType::All.to_u32(),
                            )
                            .unwrap();
                        let signature = ecdsa_sign(
                            sighash.as_byte_array().to_vec(),
                            path0.clone().into_inner(),
                        )
                        .await
                        .signature;
                        let mut signature = sec1_to_der(signature);
                        signature.push(EcdsaSighashType::All.to_u32() as u8);
                        let signature = PushBytesBuf::try_from(signature).unwrap();
                        let pubkey = PushBytesBuf::try_from(pubkey0.clone()).unwrap();
                        input.script_sig = Builder::new()
                            .push_slice(signature)
                            .push_slice(pubkey)
                            .into_script();
                        input.witness.clear();
                    } else {
                        let sighash = txn_cache
                            .legacy_signature_hash(
                                i,
                                &address1.script_pubkey(),
                                EcdsaSighashType::All.to_u32(),
                            )
                            .unwrap();
                        let signature = ecdsa_sign(
                            sighash.as_byte_array().to_vec(),
                            path1.clone().into_inner(),
                        )
                        .await
                        .signature;
                        let mut signature = sec1_to_der(signature);
                        signature.push(EcdsaSighashType::All.to_u32() as u8);
                        let signature = PushBytesBuf::try_from(signature).unwrap();
                        let pubkey = PushBytesBuf::try_from(pubkey1.clone()).unwrap();
                        input.script_sig = Builder::new()
                            .push_slice(signature)
                            .push_slice(pubkey)
                            .into_script();
                        input.witness.clear();
                    }
                }
                let txid = txn.compute_txid().to_string();
                let txn_bytes = bitcoin::consensus::serialize(&txn);
                ic_cdk::println!("{}", hex::encode(&txn_bytes));
                bitcoin_send_transaction(SendTransactionRequest {
                    network: read_config(|config| config.bitcoin_network()),
                    transaction: txn_bytes,
                })
                .await
                .expect("failed to submit transaction");
                Some(SubmittedTransactionIdType::Bitcoin { txid })
            }
            Self::Runestone {
                sender_addr: _,
                receiver_addr: _,
                sender_account,
                receiver_account,
                runeid,
                amount,
                fee,
                runic_utxos,
                fee_utxos,
                paid_by_sender,
                sender_address,
                receiver_address,
                postage,
            } => {
                const DUST_THRESHOLD: u64 = 1_000;

                let mut runic_total_spent = 0;
                let mut btc_in_runic_spent = 0;
                let mut fee_total_spent = 0;

                let mut index_of_utxos_of_sender = vec![];

                let mut input = vec![];
                runic_utxos.iter().for_each(|r_utxo| {
                    runic_total_spent += r_utxo.balance;
                    btc_in_runic_spent += r_utxo.utxo.value;
                    let txin = TxIn {
                        script_sig: ScriptBuf::new(),
                        witness: Witness::new(),
                        sequence: Sequence::MAX,
                        previous_output: OutPoint {
                            txid: Txid::from_raw_hash(
                                Hash::from_slice(&r_utxo.utxo.outpoint.txid)
                                    .expect("should return hash"),
                            ),
                            vout: r_utxo.utxo.outpoint.vout,
                        },
                    };
                    let i = input.len();
                    index_of_utxos_of_sender.push(i);
                    input.push(txin);
                });

                let need_change_rune_output = runic_total_spent > *amount || runic_utxos.len() > 1;

                let required_btc_for_rune_output = if need_change_rune_output {
                    *postage * 2
                } else {
                    *postage
                };

                let actual_required_btc =
                    required_btc_for_rune_output.to_sat() - btc_in_runic_spent;

                fee_utxos.iter().for_each(|utxo| {
                    fee_total_spent += utxo.value;
                    let txin = TxIn {
                        script_sig: ScriptBuf::new(),
                        witness: Witness::new(),
                        sequence: Sequence::MAX,
                        previous_output: OutPoint {
                            txid: Txid::from_raw_hash(
                                Hash::from_slice(&utxo.outpoint.txid).expect("should return hash"),
                            ),
                            vout: utxo.outpoint.vout,
                        },
                    };
                    let i = input.len();
                    if *paid_by_sender {
                        index_of_utxos_of_sender.push(i);
                    }
                    input.push(txin);
                });

                let id = ordinals::RuneId {
                    block: runeid.block,
                    tx: runeid.tx,
                };
                let runestone = Runestone {
                    edicts: vec![Edict {
                        id,
                        amount: *amount,
                        output: 2,
                    }],
                    ..Default::default()
                };

                let mut output = if need_change_rune_output {
                    vec![
                        TxOut {
                            script_pubkey: runestone.encipher(),
                            value: Amount::from_sat(0),
                        },
                        TxOut {
                            script_pubkey: sender_address.script_pubkey(),
                            value: *postage,
                        },
                        TxOut {
                            script_pubkey: receiver_address.script_pubkey(),
                            value: *postage,
                        },
                    ]
                } else {
                    vec![TxOut {
                        script_pubkey: receiver_address.script_pubkey(),
                        value: *postage,
                    }]
                };

                let remaining = fee_total_spent - fee - actual_required_btc;

                if remaining > DUST_THRESHOLD {
                    if *paid_by_sender {
                        output.push(TxOut {
                            script_pubkey: sender_address.script_pubkey(),
                            value: Amount::from_sat(remaining),
                        });
                    } else {
                        output.push(TxOut {
                            script_pubkey: receiver_address.script_pubkey(),
                            value: Amount::from_sat(remaining),
                        });
                    }
                }

                let mut txn = Transaction {
                    input,
                    output,
                    lock_time: LockTime::ZERO,
                    version: Version(2),
                };

                // signing the transaction
                let (sender_path, sender_pubkey, receiver_path, receiver_pubkey) =
                    read_config(|config| {
                        let ecdsa_key = config.ecdsa_public_key();
                        let sender_path = account_to_derivation_path(sender_account);
                        let receiver_path = account_to_derivation_path(receiver_account);
                        let pubkey0 = derive_public_key(&ecdsa_key, &sender_path).public_key;
                        let pubkey1 = derive_public_key(&ecdsa_key, &receiver_path).public_key;
                        (
                            DerivationPath::new(sender_path),
                            pubkey0,
                            DerivationPath::new(receiver_path),
                            pubkey1,
                        )
                    });

                let txn_cache = SighashCache::new(txn.clone());
                for (index, input) in txn.input.iter_mut().enumerate() {
                    if index_of_utxos_of_sender.contains(&index) {
                        let sighash = txn_cache
                            .legacy_signature_hash(
                                index,
                                &sender_address.script_pubkey(),
                                EcdsaSighashType::All.to_u32(),
                            )
                            .unwrap();
                        let signature = ecdsa_sign(
                            sighash.as_byte_array().to_vec(),
                            sender_path.clone().into_inner(),
                        )
                        .await
                        .signature;
                        let mut signature = sec1_to_der(signature);
                        signature.push(EcdsaSighashType::All.to_u32() as u8);
                        let signature = PushBytesBuf::try_from(signature).unwrap();
                        let pubkey = PushBytesBuf::try_from(sender_pubkey.clone()).unwrap();
                        input.script_sig = Builder::new()
                            .push_slice(signature)
                            .push_slice(pubkey)
                            .into_script();
                        input.witness.clear();
                    } else {
                        let sighash = txn_cache
                            .legacy_signature_hash(
                                index,
                                &receiver_address.script_pubkey(),
                                EcdsaSighashType::All.to_u32(),
                            )
                            .unwrap();
                        let signature = ecdsa_sign(
                            sighash.as_byte_array().to_vec(),
                            receiver_path.clone().into_inner(),
                        )
                        .await
                        .signature;
                        let mut signature = sec1_to_der(signature);
                        signature.push(EcdsaSighashType::All.to_u32() as u8);
                        let signature = PushBytesBuf::try_from(signature).unwrap();
                        let pubkey = PushBytesBuf::try_from(receiver_pubkey.clone()).unwrap();
                        input.script_sig = Builder::new()
                            .push_slice(signature)
                            .push_slice(pubkey)
                            .into_script();
                        input.witness.clear();
                    }
                }
                /* let total_btc_in_ouput: u64 =
                    txn.output.iter().map(|output| output.value.to_sat()).sum();
                ic_cdk::println!("btc in outout: {}", total_btc_in_ouput); */
                let txid = txn.compute_txid().to_string();
                let txn_bytes = bitcoin::consensus::serialize(&txn);
                ic_cdk::println!("{}", hex::encode(&txn_bytes));
                bitcoin_send_transaction(SendTransactionRequest {
                    network: read_config(|config| config.bitcoin_network()),
                    transaction: txn_bytes,
                })
                .await
                .expect("failed to submit transaction");
                Some(SubmittedTransactionIdType::Bitcoin { txid })
            }
            Self::Combined {
                sender_addr: _,
                receiver_addr: _,
                sender_address,
                receiver_address,
                sender_account,
                receiver_account,
                runic_utxos,
                btc_utxos,
                fee_utxos,
                runeid,
                rune_amount,
                btc_amount,
                fee,
                postage,
                paid_by_sender,
            } => {
                const DUST_THRESHOLD: u64 = 1_000;
                let (
                    mut runic_total_spent,
                    mut btc_in_runic_spent,
                    mut btc_total_spent,
                    mut fee_total_spent,
                ) = (0, 0, 0, 0);

                let mut input = vec![];
                let mut index_of_utxos_receiver = vec![];

                runic_utxos.iter().for_each(|utxo| {
                    runic_total_spent += utxo.balance;
                    btc_in_runic_spent += utxo.utxo.value;
                    let txin = TxIn {
                        sequence: Sequence::MAX,
                        script_sig: ScriptBuf::new(),
                        witness: Witness::new(),
                        previous_output: OutPoint {
                            txid: Txid::from_raw_hash(
                                Hash::from_slice(&utxo.utxo.outpoint.txid)
                                    .expect("should return hash"),
                            ),
                            vout: utxo.utxo.outpoint.vout,
                        },
                    };
                    input.push(txin);
                });

                btc_utxos.iter().for_each(|utxo| {
                    btc_total_spent += utxo.value;
                    let txin = TxIn {
                        sequence: Sequence::MAX,
                        script_sig: ScriptBuf::new(),
                        witness: Witness::new(),
                        previous_output: OutPoint {
                            txid: Txid::from_raw_hash(
                                Hash::from_slice(&utxo.outpoint.txid).expect("should return hash"),
                            ),
                            vout: utxo.outpoint.vout,
                        },
                    };
                    input.push(txin);
                });

                fee_utxos.iter().for_each(|utxo| {
                    fee_total_spent += utxo.value;
                    let txin = TxIn {
                        sequence: Sequence::MAX,
                        witness: Witness::new(),
                        script_sig: ScriptBuf::new(),
                        previous_output: OutPoint {
                            txid: Txid::from_raw_hash(
                                Hash::from_slice(&utxo.outpoint.txid).expect("should return hash"),
                            ),
                            vout: utxo.outpoint.vout,
                        },
                    };
                    if !paid_by_sender {
                        let len = input.len();
                        index_of_utxos_receiver.push(len);
                    }
                    input.push(txin);
                });

                let need_change_rune_output =
                    runic_total_spent > *rune_amount || runic_utxos.len() > 1;

                let required_btc_for_rune_output = if need_change_rune_output {
                    *postage * 2
                } else {
                    *postage
                };

                let actual_required_btc =
                    required_btc_for_rune_output.to_sat() - btc_in_runic_spent;

                let id = ordinals::RuneId {
                    block: runeid.block,
                    tx: runeid.tx,
                };
                let runestone = Runestone {
                    edicts: vec![Edict {
                        id,
                        amount: *rune_amount,
                        output: 2,
                    }],
                    ..Default::default()
                };

                // output for rune transfer
                let mut output = if need_change_rune_output {
                    vec![
                        TxOut {
                            script_pubkey: runestone.encipher(),
                            value: Amount::from_sat(0),
                        },
                        TxOut {
                            script_pubkey: sender_address.script_pubkey(),
                            value: *postage,
                        },
                        TxOut {
                            script_pubkey: receiver_address.script_pubkey(),
                            value: *postage,
                        },
                    ]
                } else {
                    vec![TxOut {
                        script_pubkey: receiver_address.script_pubkey(),
                        value: *postage,
                    }]
                };

                // output for bitcoin transfer
                output.push(TxOut {
                    value: Amount::from_sat(*btc_amount),
                    script_pubkey: receiver_address.script_pubkey(),
                });

                if *paid_by_sender {
                    let remaining = btc_total_spent - *btc_amount - *fee - actual_required_btc;
                    if remaining > DUST_THRESHOLD {
                        output.push(TxOut {
                            value: Amount::from_sat(remaining),
                            script_pubkey: sender_address.script_pubkey(),
                        });
                    }
                } else {
                    let remaining_sender_btc = btc_total_spent - *btc_amount;
                    if remaining_sender_btc > DUST_THRESHOLD {
                        output.push(TxOut {
                            value: Amount::from_sat(remaining_sender_btc),
                            script_pubkey: sender_address.script_pubkey(),
                        });
                    }
                    let remaining_balance = fee_total_spent - fee - actual_required_btc;
                    if remaining_balance > DUST_THRESHOLD {
                        output.push(TxOut {
                            value: Amount::from_sat(remaining_balance),
                            script_pubkey: receiver_address.script_pubkey(),
                        });
                    }
                }

                let mut txn = Transaction {
                    input,
                    output,
                    version: Version(2),
                    lock_time: LockTime::ZERO,
                };

                ic_cdk::println!(
                    "input's length to be signed by receiver: {}\nfee: {}",
                    index_of_utxos_receiver.len(),
                    *fee
                );

                // signing logic

                let (sender_path, sender_pubkey, receiver_path, receiver_pubkey) =
                    read_config(|config| {
                        let ecdsa_key = config.ecdsa_public_key();
                        let sender_path = account_to_derivation_path(sender_account);
                        let receiver_path = account_to_derivation_path(receiver_account);
                        let pubkey0 = derive_public_key(&ecdsa_key, &sender_path).public_key;
                        let pubkey1 = derive_public_key(&ecdsa_key, &receiver_path).public_key;
                        (
                            DerivationPath::new(sender_path),
                            pubkey0,
                            DerivationPath::new(receiver_path),
                            pubkey1,
                        )
                    });

                let txn_cache = SighashCache::new(txn.clone());
                for (index, input) in txn.input.iter_mut().enumerate() {
                    if index_of_utxos_receiver.contains(&index) {
                        let sighash = txn_cache
                            .legacy_signature_hash(
                                index,
                                &receiver_address.script_pubkey(),
                                EcdsaSighashType::All.to_u32(),
                            )
                            .unwrap();
                        let signature = ecdsa_sign(
                            sighash.as_byte_array().to_vec(),
                            receiver_path.clone().into_inner(),
                        )
                        .await
                        .signature;
                        let mut signature = sec1_to_der(signature);
                        signature.push(EcdsaSighashType::All.to_u32() as u8);
                        let signature = PushBytesBuf::try_from(signature).unwrap();
                        let pubkey = PushBytesBuf::try_from(receiver_pubkey.clone()).unwrap();
                        input.script_sig = Builder::new()
                            .push_slice(signature)
                            .push_slice(pubkey)
                            .into_script();
                        input.witness.clear();
                    } else {
                        let sighash = txn_cache
                            .legacy_signature_hash(
                                index,
                                &sender_address.script_pubkey(),
                                EcdsaSighashType::All.to_u32(),
                            )
                            .unwrap();
                        let signature = ecdsa_sign(
                            sighash.as_byte_array().to_vec(),
                            sender_path.clone().into_inner(),
                        )
                        .await
                        .signature;
                        let mut signature = sec1_to_der(signature);
                        signature.push(EcdsaSighashType::All.to_u32() as u8);
                        let signature = PushBytesBuf::try_from(signature).unwrap();
                        let pubkey = PushBytesBuf::try_from(sender_pubkey.clone()).unwrap();
                        input.script_sig = Builder::new()
                            .push_slice(signature)
                            .push_slice(pubkey)
                            .into_script();
                        input.witness.clear();
                    }
                }
                let txid = txn.compute_txid().to_string();
                let txn_bytes = bitcoin::consensus::serialize(&txn);
                ic_cdk::println!("{}", hex::encode(&txn_bytes));
                bitcoin_send_transaction(SendTransactionRequest {
                    network: read_config(|config| config.bitcoin_network()),
                    transaction: txn_bytes,
                })
                .await
                .expect("failed to submit transaction");
                Some(SubmittedTransactionIdType::Bitcoin { txid })
            }
        }
    }
}
