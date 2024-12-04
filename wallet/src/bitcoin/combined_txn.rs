use bitcoin::{
    absolute::LockTime, hashes::Hash, transaction::Version, Address, Amount, OutPoint, ScriptBuf,
    Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use ic_cdk::api::management_canister::bitcoin::Utxo;
use icrc_ledger_types::icrc1::account::Account;
use ordinals::{Edict, Runestone};

use crate::{
    bitcoin::signer::mock_signature,
    state::{write_utxo_manager, RunicUtxo},
    transaction_handler::TransactionType,
    types::RuneId,
};

const DEFAULT_POSTAGE: u64 = 10_000;

pub struct CombinedTransactionRequest<'a> {
    pub from_addr: &'a str,
    pub receiver_addr: &'a str,
    pub sender_address: Address,
    pub receiver_address: Address,
    pub runeid: RuneId,
    pub rune_amount: u128,
    pub btc_amount: u64,
    pub sender_account: Account,
    pub receiver_account: Account,
    pub postage: Option<u64>,
    pub fee_per_vbytes: u64,
    pub paid_by_sender: bool,
}

pub fn transfer(
    CombinedTransactionRequest {
        from_addr,
        receiver_addr,
        sender_address,
        receiver_address,
        runeid,
        rune_amount,
        btc_amount,
        sender_account,
        receiver_account,
        postage,
        fee_per_vbytes,
        paid_by_sender,
    }: CombinedTransactionRequest,
) -> Result<TransactionType, (u128, u64, u64)> {
    let mut total_fee = 0;
    let postage = Amount::from_sat(postage.unwrap_or(DEFAULT_POSTAGE));
    loop {
        let (txn, runic_utxos, btc_utxos, fee_utxos) = build_transaction_with_fee(
            from_addr,
            receiver_addr,
            &sender_address,
            &receiver_address,
            &runeid,
            rune_amount,
            btc_amount,
            postage,
            total_fee,
            paid_by_sender,
        )?;

        let signed_txn = mock_signature(&txn);

        let txn_vsize = signed_txn.vsize() as u64;
        if (txn_vsize * fee_per_vbytes) / 1000 == total_fee {
            return Ok(TransactionType::Combined {
                sender_addr: from_addr.to_string(),
                receiver_addr: receiver_addr.to_string(),
                sender_address: sender_address.clone(),
                receiver_address: receiver_address.clone(),
                sender_account,
                receiver_account,
                runic_utxos,
                btc_utxos,
                fee_utxos,
                runeid,
                rune_amount,
                btc_amount,
                fee: total_fee,
                postage,
                paid_by_sender,
            });
        } else {
            write_utxo_manager(|manager| {
                manager.record_runic_utxos(from_addr, runeid.clone(), runic_utxos);
                manager.record_btc_utxos(from_addr, btc_utxos);
                manager.record_btc_utxos(receiver_addr, fee_utxos);
            });
            total_fee = (txn_vsize * fee_per_vbytes) / 1000;
        }
    }
}

fn build_transaction_with_fee(
    from_addr: &str,
    receiver_addr: &str,
    sender_address: &Address,
    receiver_address: &Address,
    runeid: &RuneId,
    rune_amount: u128,
    btc_amount: u64,
    postage: Amount,
    fee: u64,
    paid_by_sender: bool,
) -> Result<(Transaction, Vec<RunicUtxo>, Vec<Utxo>, Vec<Utxo>), (u128, u64, u64)> {
    const DUST_THRESHOLD: u64 = 1_000;

    let (runic_utxos, runic_total_spent, btc_in_runic_spent) = write_utxo_manager(|manager| {
        let mut utxos = vec![];
        let mut runic_total_spent = 0;
        let mut btc_in_runic_spent = 0;
        while let Some(utxo) = manager.get_runic_utxo(from_addr, runeid.clone()) {
            runic_total_spent += utxo.balance;
            btc_in_runic_spent += utxo.utxo.value;
            utxos.push(utxo);
        }
        if runic_total_spent < rune_amount {
            manager.record_runic_utxos(from_addr, runeid.clone(), utxos);
            return Err((rune_amount, btc_amount, fee));
        }
        Ok((utxos, runic_total_spent, btc_in_runic_spent))
    })?;

    let (btc_utxos, btc_total_spent) = write_utxo_manager(|manager| {
        let mut utxos = vec![];
        let mut btc_total_spent = 0;

        while let Some(utxo) = manager.get_bitcoin_utxo(from_addr) {
            btc_total_spent += utxo.value;
            utxos.push(utxo);
            if btc_total_spent > btc_amount {
                break;
            }
        }

        if btc_total_spent < btc_amount {
            manager.record_btc_utxos(from_addr, utxos);
            return Err((rune_amount, btc_amount, fee));
        }

        Ok((utxos, btc_total_spent))
    })?;

    let need_change_rune_output = runic_total_spent > rune_amount || runic_utxos.len() > 1;

    let required_btc_for_rune_output = if need_change_rune_output {
        postage * 2
    } else {
        postage
    };

    let actual_required_btc = required_btc_for_rune_output.to_sat() - btc_in_runic_spent;

    let (fee_utxos, fee_total_spent) = write_utxo_manager(|manager| {
        let mut utxos = vec![];
        let mut fee_total_spent = 0;

        if paid_by_sender {
            if btc_total_spent < btc_amount - actual_required_btc - fee {
                return Err((rune_amount, btc_amount + actual_required_btc + fee, 0));
            }
            Ok((utxos, fee_total_spent))
        } else {
            while let Some(utxo) = manager.get_bitcoin_utxo(receiver_addr) {
                fee_total_spent += utxo.value;
                utxos.push(utxo);
                if fee_total_spent > fee + actual_required_btc {
                    break;
                }
            }

            if fee_total_spent < fee + actual_required_btc {
                manager.record_btc_utxos(receiver_addr, utxos);
                return Err((rune_amount, btc_amount, fee + actual_required_btc));
            }

            Ok((utxos, fee_total_spent))
        }
    })?;

    let mut input = vec![];

    runic_utxos.iter().for_each(|utxo| {
        let txin = TxIn {
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
            previous_output: OutPoint {
                txid: Txid::from_raw_hash(
                    Hash::from_slice(&utxo.utxo.outpoint.txid).expect("should return hash"),
                ),
                vout: utxo.utxo.outpoint.vout,
            },
        };
        input.push(txin);
    });

    btc_utxos.iter().for_each(|utxo| {
        let txin = TxIn {
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
            previous_output: OutPoint {
                txid: Txid::from_raw_hash(Hash::from_slice(&utxo.outpoint.txid).unwrap()),
                vout: utxo.outpoint.vout,
            },
        };
        input.push(txin);
    });

    fee_utxos.iter().for_each(|utxo| {
        let txin = TxIn {
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
            previous_output: OutPoint {
                txid: Txid::from_raw_hash(Hash::from_slice(&utxo.outpoint.txid).unwrap()),
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
            amount: rune_amount,
            output: 2,
        }],
        ..Default::default()
    };

    // rune transfer output

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

    // btc transfer output
    output.push(TxOut {
        script_pubkey: receiver_address.script_pubkey(),
        value: Amount::from_sat(btc_amount),
    });

    // remaining fee output
    if !paid_by_sender {
        let remaining_btc_of_sender = btc_total_spent - btc_amount;
        if remaining_btc_of_sender > DUST_THRESHOLD {
            output.push(TxOut {
                value: Amount::from_sat(remaining_btc_of_sender),
                script_pubkey: sender_address.script_pubkey(),
            });
        }
        let remaining = fee_total_spent - fee - actual_required_btc;
        if remaining > DUST_THRESHOLD {
            output.push(TxOut {
                script_pubkey: receiver_address.script_pubkey(),
                value: Amount::from_sat(remaining),
            });
        }
    } else {
        let remaining = btc_total_spent - btc_amount - fee - actual_required_btc;
        if remaining > DUST_THRESHOLD {
            output.push(TxOut {
                value: Amount::from_sat(remaining),
                script_pubkey: sender_address.script_pubkey(),
            });
        }
    }

    let txn = Transaction {
        input,
        output,
        version: Version(2),
        lock_time: LockTime::ZERO,
    };

    Ok((txn, runic_utxos, btc_utxos, fee_utxos))
}
