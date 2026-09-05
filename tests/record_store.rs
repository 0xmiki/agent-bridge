#![cfg(feature = "records")]

mod memory {
    fn fresh_store() -> std::sync::Arc<dyn agent_bridge::records::RecordStore> {
        std::sync::Arc::new(agent_bridge::records::MemoryStore::default())
    }
    include!("support/record_store_contract.rs");
}

#[cfg(feature = "sqlite")]
mod sqlite {
    fn fresh_store() -> std::sync::Arc<dyn agent_bridge::records::RecordStore> {
        std::sync::Arc::new(agent_bridge::records::SqliteStore::open_in_memory().unwrap())
    }
    include!("support/record_store_contract.rs");
}
