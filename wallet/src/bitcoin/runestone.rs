use bitcoin::{
    absolute::LockTime, hashes::Hash, transaction::Version, Address, Amount, OutPoint, ScriptBuf,
    Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use ic_cdk::api::management_canister::bitcoin::Utxo;
use icrc_ledger_types::icrc1::account::Account;
use ordinals::{Edict, Runestone};

use crate::{
    state::{write_utxo_manager, RunicUtxo},
    transaction_handler::TransactionType,
    types::RuneId,
};

const DEFAULT_POSTAGE: u64 = 10_000;

use super::signer::mock_signature;

pub struct RuneTransferArgs<'a> {
    pub runeid: RuneId,
    pub amount: u128,
    pub sender_addr: &'a str,
    pub receiver_addr: &'a str,
    pub sender_account: Account,
    pub receiver_account: Account,
    pub sender_address: Address,
    pub receiver_address: Address,
    pub fee_per_vbytes: u64,
    pub paid_by_sender: bool,
    pub postage: Option<u64>,
}

pub fn transfer(
    RuneTransferArgs {
        runeid,
        amount,
        sender_addr,
        receiver_addr,
        sender_account,
        receiver_account,
        sender_address,
        receiver_address,
        fee_per_vbytes,
        paid_by_sender,
        postage,
    }: RuneTransferArgs,
) -> Result<TransactionType, (u128, u64)> {
    let mut total_fee = 0;
    let postage = Amount::from_sat(postage.unwrap_or(DEFAULT_POSTAGE));
    loop {
        let (txn, runic_utxos, fee_utxos) = build_transaction_with_fee(
            &runeid,
            amount,
            sender_addr,
            receiver_addr,
            &sender_address,
            &receiver_address,
            total_fee,
            paid_by_sender,
            postage,
        )?;

        let signed_txn = mock_signature(&txn);

        let txn_vsize = signed_txn.vsize() as u64;
        if (txn_vsize * fee_per_vbytes) / 1000 == total_fee {
            return Ok(TransactionType::Runestone {
                sender_addr: sender_addr.to_string(),
                receiver_addr: receiver_addr.to_string(),
                sender_account,
                receiver_account,
                runeid,
                amount,
                fee: total_fee,
                runic_utxos,
                fee_utxos,
                paid_by_sender,
                sender_address,
                receiver_address,
                postage,
            });
        } else {
            write_utxo_manager(|manager| {
                manager.record_runic_utxos(sender_addr, runeid.clone(), runic_utxos);
                if paid_by_sender {
                    manager.record_btc_utxos(sender_addr, fee_utxos);
                } else {
                    manager.record_btc_utxos(receiver_addr, fee_utxos);
                }
            });
            total_fee = (txn_vsize * fee_per_vbytes) / 1000;
        }
    }
}

pub fn build_transaction_with_fee(
    runeid: &RuneId,
    amount: u128,
    sender_addr: &str,
    receiver_addr: &str,
    sender_address: &Address,
    receiver_address: &Address,
    fee: u64,
    paid_by_sender: bool,
    postage: Amount,
) -> Result<(Transaction, Vec<RunicUtxo>, Vec<Utxo>), (u128, u64)> {
    const DUST_THRESHOLD: u64 = 1_000;

    let (runic_utxos, runic_total_spent, btc_in_runic) = write_utxo_manager(|manager| {
        let mut r_utxos = vec![];
        let mut runic_total_spent = 0;
        let mut btc_in_runic = 0;
        while let Some(utxo) = manager.get_runic_utxo(sender_addr, runeid.clone()) {
            runic_total_spent += utxo.balance;
            btc_in_runic += utxo.utxo.value;
            r_utxos.push(utxo);
            if runic_total_spent > amount {
                break;
            }
        }

        if runic_total_spent < amount {
            manager.record_runic_utxos(sender_addr, runeid.clone(), r_utxos);
            return Err((amount, 0));
        }
        Ok((r_utxos, runic_total_spent, btc_in_runic))
    })?;

    let need_change_rune_output = runic_total_spent > amount || runic_utxos.len() > 1;

    let required_btc_for_rune_output = if need_change_rune_output {
        postage * 2
    } else {
        postage
    };

    let actual_required_btc = required_btc_for_rune_output.to_sat() - btc_in_runic;

    let (fee_utxos, fee_total_spent) = write_utxo_manager(|manager| {
        let mut utxos = vec![];
        let mut total_spent = 0;
        let fee_payer = if paid_by_sender {
            sender_addr
        } else {
            receiver_addr
        };
        while let Some(utxo) = manager.get_bitcoin_utxo(fee_payer) {
            total_spent += utxo.value;
            utxos.push(utxo);
            if total_spent > fee + actual_required_btc {
                break;
            }
        }
        if total_spent < fee + actual_required_btc {
            manager.record_btc_utxos(fee_payer, utxos);
            return Err((0, fee));
        }
        Ok((utxos, total_spent))
    })?;

    let mut input = vec![];

    runic_utxos.iter().for_each(|r_utxo| {
        let txin = TxIn {
            script_sig: ScriptBuf::new(),
            witness: Witness::new(),
            sequence: Sequence::MAX,
            previous_output: OutPoint {
                txid: Txid::from_raw_hash(
                    Hash::from_slice(&r_utxo.utxo.outpoint.txid).expect("should return hash"),
                ),
                vout: r_utxo.utxo.outpoint.vout,
            },
        };
        input.push(txin);
    });

    fee_utxos.iter().for_each(|utxo| {
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
        input.push(txin);
    });

    let id = ordinals::RuneId {
        block: runeid.block,
        tx: runeid.tx,
    };
    let runestone = Runestone {
        edicts: vec![Edict {
            id,
            amount,
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
                value: postage,
            },
            TxOut {
                script_pubkey: receiver_address.script_pubkey(),
                value: postage,
            },
        ]
    } else {
        vec![TxOut {
            script_pubkey: receiver_address.script_pubkey(),
            value: postage,
        }]
    };

    let remaining = fee_total_spent - fee - actual_required_btc;

    if remaining > DUST_THRESHOLD {
        if paid_by_sender {
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

    let txn = Transaction {
        input,
        output,
        version: Version(2),
        lock_time: LockTime::ZERO,
    };

    Ok((txn, runic_utxos, fee_utxos))
}
