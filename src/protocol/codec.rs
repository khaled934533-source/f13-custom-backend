use serde::{de::DeserializeOwned, Serialize};
use std::io::{self, ErrorKind};

use super::packet::{MAGIC, VERSION};

/// TCP frame format:
///
/// [4 bytes]  MAGIC
/// [1 byte]   VERSION
/// [2 bytes]  MESSAGE ID
/// [4 bytes]  REQUEST ID
/// [4 bytes]  PAYLOAD LENGTH
/// [N bytes]  PAYLOAD
///
/// Integer fields use big-endian byte order.

pub fn encode_frame<T: Serialize>(
    message_id: u16,
    request_id: u32,
    payload: &T,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let payload = serde_json::to_vec(payload)?;

    let payload_len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "payload too large"))?;

    let mut frame = Vec::with_capacity(15 + payload.len());

    // Magic
    frame.extend_from_slice(&MAGIC);

    // Version
    frame.push(VERSION);

    // Message ID
    frame.extend_from_slice(&message_id.to_be_bytes());

    // Request ID
    frame.extend_from_slice(&request_id.to_be_bytes());

    // Payload length
    frame.extend_from_slice(&payload_len.to_be_bytes());

    // Payload
    frame.extend_from_slice(&payload);

    Ok(frame)
}

pub fn decode_frame<T: DeserializeOwned>(
    data: &[u8],
) -> Result<(u16, u32, T), Box<dyn std::error::Error + Send + Sync>> {
    const HEADER_SIZE: usize = 15;

    if data.len() < HEADER_SIZE {
        return Err(io::Error::new(
            ErrorKind::UnexpectedEof,
            "incomplete packet header",
        )
        .into());
    }

    // Magic
    if data[0..4] != MAGIC {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "invalid packet magic",
        )
        .into());
    }

    // Version
    if data[4] != VERSION {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "unsupported protocol version",
        )
        .into());
    }

    // Message ID
    let message_id = u16::from_be_bytes([data[5], data[6]]);

    // Request ID
    let request_id = u32::from_be_bytes([
        data[7],
        data[8],
        data[9],
        data[10],
    ]);

    // Payload length
    let payload_len = u32::from_be_bytes([
        data[11],
        data[12],
        data[13],
        data[14],
    ]) as usize;

    if data.len() < HEADER_SIZE + payload_len {
        return Err(io::Error::new(
            ErrorKind::UnexpectedEof,
            "incomplete packet payload",
        )
        .into());
    }

    let payload_start = HEADER_SIZE;
    let payload_end = payload_start + payload_len;

    let payload: T =
        serde_json::from_slice(&data[payload_start..payload_end])?;

    Ok((message_id, request_id, payload))
}
