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

pub fn transfer(
    addr: &str,
    account: Account,
    from: Address,
    to: Address,
    amount: u64,
    paid_by_sender: bool,
    fee_per_vbytes: u64,
) -> Result<TransactionType, u64> {
    let mut total_fee = 0;
    loop {
        let (txn, utxos) =
            build_transaction_with_fee(addr, &from, &to, amount, total_fee, paid_by_sender)?;
        let signed_txn = mock_signature(&txn);

        let txn_vsize = signed_txn.vsize() as u64;
        if (txn_vsize * fee_per_vbytes) / 1000 == total_fee {
            return Ok(TransactionType::Bitcoin {
                addr: addr.to_string(),
                utxos,
                signer_account: account,
                signer_address: from,
                txn,
            });
        } else {
            write_utxo_manager(|state| state.record_btc_utxos(addr, utxos));
            total_fee = (txn_vsize * fee_per_vbytes) / 1000;
        }
    }
}

fn build_transaction_with_fee(
    addr: &str,
    from: &Address,
    to: &Address,
    amount: u64,
    fee: u64,
    paid_by_sender: bool,
) -> Result<(Transaction, Vec<Utxo>), u64> {
    const DUST_THRESHOLD: u64 = 1_000;
    let total_amount = if paid_by_sender { amount + fee } else { amount };

    let (utxos_to_spend, total_spent) = write_utxo_manager(|manager| {
        let mut utxos = vec![];
        let mut sum = 0;

        while let Some(utxo) = manager.get_bitcoin_utxo(addr) {
            sum += utxo.value;
            utxos.push(utxo);
            if sum > total_amount {
                break;
            }
        }
        if sum < total_amount {
            manager.record_btc_utxos(addr, utxos);
            return Err(total_amount);
        }
        Ok((utxos, sum))
    })?;

    let input: Vec<TxIn> = utxos_to_spend
        .iter()
        .map(|utxo| TxIn {
            sequence: Sequence::MAX,
            script_sig: ScriptBuf::new(),
            witness: Witness::new(),
            previous_output: OutPoint {
                txid: Txid::from_raw_hash(
                    Hash::from_slice(&utxo.outpoint.txid).expect("should return hash"),
                ),
                vout: utxo.outpoint.vout,
            },
        })
        .collect();

    let mut output = vec![TxOut {
        script_pubkey: to.script_pubkey(),
        value: if paid_by_sender {
            Amount::from_sat(amount)
        } else {
            Amount::from_sat(amount - fee)
        },
    }];

    let remaining = total_spent - total_amount;
    if remaining > DUST_THRESHOLD {
        output.push(TxOut {
            script_pubkey: from.script_pubkey(),
            value: Amount::from_sat(remaining),
        });
    }
    let txn = Transaction {
        input,
        output,
        lock_time: LockTime::ZERO,
        version: Version(2),
    };
    Ok((txn, utxos_to_spend))
}
