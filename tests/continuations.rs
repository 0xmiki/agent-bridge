#![cfg(feature = "records")]

mod memory {
    fn fresh_store() -> std::sync::Arc<dyn agent_bridge::records::ContinuationStore> {
        std::sync::Arc::new(agent_bridge::records::MemoryStore::default())
    }
    include!("support/continuation_contract.rs");
}

#[cfg(feature = "sqlite")]
mod sqlite {
    fn fresh_store() -> std::sync::Arc<dyn agent_bridge::records::ContinuationStore> {
        std::sync::Arc::new(agent_bridge::records::SqliteStore::open_in_memory().unwrap())
    }
    include!("support/continuation_contract.rs");
}
