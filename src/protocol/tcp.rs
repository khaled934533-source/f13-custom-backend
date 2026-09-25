use std::io;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use super::{
    codec::{decode_frame, encode_frame},
    ClientMessage, ServerMessage,
};

pub const MSG_AUTHENTICATE: u16 = 0x0001;
pub const MSG_HEARTBEAT: u16 = 0x0002;
pub const MSG_CREATE_SESSION: u16 = 0x0003;
pub const MSG_JOIN_SESSION: u16 = 0x0004;
pub const MSG_LEAVE_SESSION: u16 = 0x0005;

const HEADER_SIZE: usize = 15;
const MAX_PAYLOAD_SIZE: usize = 1024 * 1024;

pub async fn run_tcp_server(addr: String) -> io::Result<()> {
    let listener = TcpListener::bind(&addr).await?;

    println!("Protocol TCP listening on {addr}");

    loop {
        let (stream, peer) = listener.accept().await?;

        println!("TCP client connected: {peer}");

        tokio::spawn(async move {
            if let Err(error) = handle_client(stream).await {
                eprintln!("TCP client error ({peer}): {error}");
            }

            println!("TCP client disconnected: {peer}");
        });
    }
}

async fn handle_client(
    mut stream: TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let mut header = [0u8; HEADER_SIZE];

        match stream.read_exact(&mut header).await {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        }

        let payload_len = u32::from_be_bytes([
            header[11],
            header[12],
            header[13],
            header[14],
        ]) as usize;

        if payload_len > MAX_PAYLOAD_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "payload exceeds maximum size",
            )
            .into());
        }

        let mut frame = Vec::with_capacity(HEADER_SIZE + payload_len);
        frame.extend_from_slice(&header);

        let mut payload = vec![0u8; payload_len];
        stream.read_exact(&mut payload).await?;
        frame.extend_from_slice(&payload);

        let (message_id, request_id, message): (u16, u32, ClientMessage) =
            decode_frame(&frame)?;

        let response = handle_message(message);

        let response_message_id = message_id | 0x8000;

        let response_frame =
            encode_frame(response_message_id, request_id, &response)?;

        stream.write_all(&response_frame).await?;
        stream.flush().await?;
    }
}

fn handle_message(message: ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::Authenticate {
            player_id,
            player_name,
        } => ServerMessage::Authenticated {
            player: super::types::Player {
                id: player_id,
                name: player_name,
            },
        },

        ClientMessage::CreateSession => ServerMessage::SessionCreated {
            session: super::types::Session {
                id: uuid::Uuid::new_v4().to_string(),
                players: Vec::new(),
            },
        },

        ClientMessage::JoinSession { session_id } => {
            ServerMessage::SessionJoined {
                session: super::types::Session {
                    id: session_id,
                    players: Vec::new(),
                },
            }
        }

        ClientMessage::LeaveSession { session_id } => {
            ServerMessage::SessionLeft { session_id }
        }

        ClientMessage::Heartbeat => ServerMessage::Pong,
    }
}
