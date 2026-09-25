use serde::{Deserialize, Serialize};

pub const MAGIC: [u8; 4] = *b"KLAY";
pub const VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Packet<T> {
    pub version: u8,
    pub message_id: u16,
    pub request_id: u32,
    pub payload: T,
}
