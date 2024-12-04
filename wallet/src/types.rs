use candid::{CandidType, Decode, Deserialize, Encode};
use ic_stable_structures::{storable::Bound, Storable};

#[derive(CandidType, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RuneId {
    pub block: u64,
    pub tx: u32,
}

impl Storable for RuneId {
    fn to_bytes(&self) -> std::borrow::Cow<[u8]> {
        std::borrow::Cow::Owned(Encode!(self).expect("should encode"))
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        Decode!(bytes.as_ref(), Self).expect("should decode")
    }

    const BOUND: Bound = Bound::Unbounded;
}

#[derive(CandidType, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenType {
    Bitcoin,
    Icp,
    CkBTC,
    Runestone(RuneId),
}

impl Storable for TokenType {
    fn to_bytes(&self) -> std::borrow::Cow<[u8]> {
        std::borrow::Cow::Owned(Encode!(self).expect("should encode"))
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        Decode!(bytes.as_ref(), Self).expect("should decode")
    }

    const BOUND: Bound = Bound::Unbounded;
}
