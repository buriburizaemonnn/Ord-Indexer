use self::{entry::Entry, event::Event, lot::Lot};
use super::*;
use crate::ic_log::*;
use bitcoin::block::Header;
use ic_canister_log::log;
use rune_indexer_interface::MintError;
use std::collections::BTreeMap;
use std::str::FromStr;

pub use self::entry::RuneEntry;

pub(crate) mod entry;
pub mod event;
mod lot;
mod updater;

#[allow(dead_code)]
pub const SCHEMA_VERSION: u64 = 26;

fn set_beginning_block(hash: &str) {
    let hash = BlockHash::from_str(hash).expect("valid hash");
    crate::increase_height(FIRST_HEIGHT, hash);
}

pub(crate) fn init_rune(hash: &str) {
    set_beginning_block(hash);
    let rune = Rune(2055900680524219742);

    let id = RuneId { block: 1, tx: 0 };
    let etching = Txid::all_zeros();

    rune_to_rune_id(|r| r.insert(rune.store(), id)).expect("MemoryOverflow");

    rune_id_to_rune_entry(|r| {
        r.insert(
            id,
            RuneEntry {
                block: id.block,
                burned: 0,
                divisibility: 0,
                etching,
                terms: Some(Terms {
                    amount: Some(1),
                    cap: Some(u128::MAX),
                    height: (
                        Some((SUBSIDY_HALVING_INTERVAL * 4).into()),
                        Some((SUBSIDY_HALVING_INTERVAL * 5).into()),
                    ),
                    offset: (None, None),
                }),
                mints: 0,
                premine: 0,
                spaced_rune: SpacedRune { rune, spacers: 128 },
                symbol: Some('\u{29C9}'),
                timestamp: 0,
                turbo: true,
            },
        )
    })
    .expect("MemoryOverflow");

    transaction_id_to_rune(|t| t.insert(Txid::store(etching), rune.store()))
        .expect("MemoryOverflow");
}

#[allow(dead_code)]
pub(crate) fn get_etching(txid: Txid) -> Result<Option<SpacedRune>> {
    let Some(rune) = crate::transaction_id_to_rune(|t| t.get(&Txid::store(txid)).map(|r| *r))
    else {
        return Ok(None);
    };

    let id = crate::rune_to_rune_id(|r| *r.get(&rune).unwrap());

    let entry = crate::rune_id_to_rune_entry(|r| *r.get(&id).unwrap());

    Ok(Some(entry.spaced_rune))
}

#[allow(dead_code)]
pub(crate) fn get_rune_balances_for_output(
    outpoint: OutPoint,
) -> Result<BTreeMap<SpacedRune, Pile>> {
    crate::outpoint_to_rune_balances(|o| match o.get(&OutPoint::store(outpoint)) {
        Some(balances) => {
            let mut result = BTreeMap::new();
            for rune in balances.iter() {
                let rune = *rune;

                let entry = rune_id_to_rune_entry(|r| r.get(&rune.id).map(|r| *r).unwrap());

                result.insert(
                    entry.spaced_rune,
                    Pile {
                        amount: rune.balance,
                        divisibility: entry.divisibility,
                        symbol: entry.symbol,
                    },
                );
            }
            Ok(result)
        }
        None => Ok(BTreeMap::new()),
    })
}

pub(crate) async fn get_best_from_rpc() -> Result<(u32, BlockHash)> {
    let url = get_url();
    let hash = rpc::get_best_block_hash(&url).await?;
    let header = rpc::get_block_header(&url, hash).await?;
    Ok((header.height.try_into().expect("usize to u32"), hash))
}

#[cfg(feature = "cmp-header")]
pub(crate) async fn cmp_header(height: u32, from_rpc: &BlockHash) {
    match crate::btc_canister::get_block_hash(height).await {
        Ok(hash) => log!(
            INFO,
            "cross compare at {}, canister={:x}, rpc={:x}",
            height,
            hash,
            from_rpc
        ),
        Err(e) => log!(ERROR, "error: {:?}", e),
    }
}

pub fn sync(secs: u64) {
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(secs), || {
        ic_cdk::spawn(async move {
            let (height, current) = crate::highest_block();
            // uncomment this to test
            if height >= 840_000 {
                ic_cdk::println!("we are done!");
                return;
            }
            match get_best_from_rpc().await {
                Ok((best, _)) => {
                    log!(INFO, "our best = {}, their best = {}", height, best);
                    if height + REQUIRED_CONFIRMATIONS >= best {
                        sync(5);
                    } else {
                        match updater::get_block(height + 1).await {
                            Ok(block) => {
                                #[cfg(feature = "cmp-header")]
                                cmp_header(height + 1, &block.header.block_hash()).await;
                                if block.header.prev_blockhash != current {
                                    log!(
                    CRITICAL,
                    "reorg detected! our best = {}({:x}), the new block to be applied {:?}",
                    height,
                    current,
                    block.header
                  );
                                    sync(5);
                                    return;
                                }
                                if let Err(e) = updater::index_block(height + 1, block).await {
                                    log!(CRITICAL, "index error: {:?}", e);
                                }
                                sync(0);
                            }
                            Err(e) => {
                                log!(ERROR, "error: {:?}", e);
                                sync(5);
                            }
                        }
                    }
                }
                Err(e) => {
                    log!(ERROR, "error: {:?}", e);
                    sync(5);
                }
            }
        });
    });
}
