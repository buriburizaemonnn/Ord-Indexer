#!/bin/bash

cargo build --release --target wasm32-unknown-unknown --package wallet 
candid-extractor ./target/wasm32-unknown-unknown/release/wallet.wasm > wallet/wallet.did || true
