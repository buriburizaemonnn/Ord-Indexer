use ic_stable_structures::{
    memory_manager::{MemoryId, VirtualMemory},
    DefaultMemoryImpl,
};

pub type Memory = VirtualMemory<DefaultMemoryImpl>;

pub enum MemoryIds {
    Config,
    Runic,
    Bitcoin,
}

impl From<MemoryIds> for MemoryId {
    fn from(value: MemoryIds) -> Self {
        match value {
            MemoryIds::Config => MemoryId::new(0),
            MemoryIds::Runic => MemoryId::new(1),
            MemoryIds::Bitcoin => MemoryId::new(2),
        }
    }
}
