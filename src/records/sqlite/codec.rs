use super::StoreError;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const DATA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<T> {
    version: u32,
    data: T,
}

pub(super) fn encode<T: Serialize + ?Sized>(data: &T) -> Result<String, StoreError> {
    serde_json::to_string(&Document {
        version: DATA_VERSION,
        data,
    })
    .map_err(|e| StoreError::CorruptData(e.to_string()))
}

pub(super) fn decode<T: DeserializeOwned>(text: &str) -> Result<T, StoreError> {
    let document: Document<serde_json::Value> =
        serde_json::from_str(text).map_err(|e| StoreError::CorruptData(e.to_string()))?;
    if document.version != DATA_VERSION {
        return Err(StoreError::UnsupportedDataVersion(document.version));
    }
    serde_json::from_value(document.data).map_err(|e| StoreError::CorruptData(e.to_string()))
}
