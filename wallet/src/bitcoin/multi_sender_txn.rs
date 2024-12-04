use bitcoin::{
    absolute::LockTime, hashes::Hash, transaction::Version, Address, Amount, OutPoint, ScriptBuf,
    Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use ic_cdk::api::management_canister::bitcoin::Utxo;
use icrc_ledger_types::icrc1::account::Account;

use crate::{
    bitcoin::signer::mock_signature, state::write_utxo_manager,
    transaction_handler::TransactionType,
};

pub struct MultiSendTransactionArgument<'a> {
    pub addr0: &'a str,
    pub addr1: &'a str,
    pub address0: Address,
    pub address1: Address,
    pub receiver: Address,
    pub account0: Account,
    pub account1: Account,
    pub amount0: u64,
    pub amount1: u64,
    pub fee_per_vbytes: u64,
    pub paid_by_sender: bool,
}

pub fn transfer(
    MultiSendTransactionArgument {
        addr0,
        addr1,
        address0,
        address1,
        receiver,
        account0,
        account1,
        amount0,
        amount1,
        fee_per_vbytes,
        paid_by_sender,
    }: MultiSendTransactionArgument,
) -> Result<TransactionType, (u64, u64)> {
    let mut total_fee = 0;
    loop {
        let (txn, utxos0, utxos1) = build_transaction_with_fee(
            addr0,
            addr1,
            &address0,
            &address1,
            &receiver,
            amount0,
            amount1,
            total_fee,
            paid_by_sender,
        )?;
        let signed_txn = mock_signature(&txn);
        let txn_vsize = signed_txn.vsize() as u64;
        if (txn_vsize * fee_per_vbytes) / 1000 == total_fee {
            return Ok(TransactionType::LegoBitcoin {
                addr0: addr0.to_string(),
                addr1: addr1.to_string(),
                account0,
                account1,
                address0,
                address1,
                utxos0,
                utxos1,
                amount0,
                amount1,
                fee: total_fee,
                paid_by_sender,
                receiver,
            });
        } else {
            write_utxo_manager(|manager| {
                manager.record_btc_utxos(addr0, utxos0);
                manager.record_btc_utxos(addr1, utxos1);
            });
            total_fee = (txn_vsize * fee_per_vbytes) / 1000;
        }
    }
}

/*
 * returns
 * Ok => (txn, utxos_owned_by_addr0, utxos_owned_by_addr1)
 * Err => (required_amount0, required_amount1)
*/
fn build_transaction_with_fee(
    addr0: &str,
    addr1: &str,
    address0: &Address,
    address1: &Address,
    receiver: &Address,
    amount0: u64,
    amount1: u64,
    fee: u64,
    paid_by_sender: bool,
) -> Result<(Transaction, Vec<Utxo>, Vec<Utxo>), (u64, u64)> {
    const DUST_THRESHOLD: u64 = 1_000;

    let (fee0, fee1) = {
        let is_even = fee % 2 == 0;
        if is_even {
            let amount_in_half = fee / 2;
            (amount_in_half, amount_in_half)
        } else {
            let amount_in_half = (fee - 1) / 2;
            (amount_in_half, amount_in_half + 1)
        }
    };

    let (total_amount0, total_amount1) = if paid_by_sender {
        (amount0 + fee0, amount1 + fee1)
    } else {
        (amount0, amount1)
    };
    let (utxo_to_spend0, total_spent0, utxo_to_spend1, total_spent1) =
        write_utxo_manager(|manager| {
            let (mut utxos0, mut utxos1) = (vec![], vec![]);
            let (mut total_spent0, mut total_spent1) = (0, 0);

            while let Some(utxo) = manager.get_bitcoin_utxo(addr0) {
                total_spent0 += utxo.value;
                utxos0.push(utxo);
                if total_spent0 >= total_amount0 {
                    break;
                }
            }

            while let Some(utxo) = manager.get_bitcoin_utxo(addr1) {
                total_spent1 += utxo.value;
                utxos1.push(utxo);
                if total_spent1 >= total_amount1 {
                    break;
                }
            }

            if (total_spent0 < total_amount0) || (total_spent1 < total_amount1) {
                manager.record_btc_utxos(addr0, utxos0);
                manager.record_btc_utxos(addr1, utxos1);
                return Err((total_amount0, total_amount1));
            }
            Ok((utxos0, total_spent0, utxos1, total_spent1))
        })?;

    let mut input = vec![];

    utxo_to_spend0.iter().for_each(|utxo| {
        let txin = TxIn {
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
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

    utxo_to_spend1.iter().for_each(|utxo| {
        let txin = TxIn {
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
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

    let mut output = vec![TxOut {
        script_pubkey: receiver.script_pubkey(),
        value: if paid_by_sender {
            Amount::from_sat(amount0 + amount1)
        } else {
            Amount::from_sat(amount0 + amount1 - fee0 - fee1)
        },
    }];

    // block responsible for calculating and adding remaining account
    {
        let remaining0 = total_spent0 - total_amount0;
        if remaining0 > DUST_THRESHOLD {
            output.push(TxOut {
                script_pubkey: address0.script_pubkey(),
                value: Amount::from_sat(remaining0),
            });
        }
        let remaining1 = total_spent1 - total_amount1;
        if remaining1 > DUST_THRESHOLD {
            output.push(TxOut {
                script_pubkey: address1.script_pubkey(),
                value: Amount::from_sat(remaining1),
            })
        }
    }
    let txn = Transaction {
        version: Version(2),
        lock_time: LockTime::ZERO,
        input,
        output,
    };
    Ok((txn, utxo_to_spend0, utxo_to_spend1))
}
