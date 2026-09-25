use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Packet<T> {
    pub version: u16,
    pub request_id: u32,
    pub payload: T,
}
