use std::io;

use sqlx::SqlitePool;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use uuid::Uuid;

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

#[derive(Debug, Clone)]
struct ConnectionState {
    user_id: Option<String>,
    display_name: Option<String>,
    session_id: Option<String>,
}

pub async fn run_tcp_server(
    addr: String,
    db: SqlitePool,
) -> io::Result<()> {
    let listener = TcpListener::bind(&addr).await?;

    println!("Protocol TCP listening on {addr}");

    loop {
        let (stream, peer) = listener.accept().await?;

        println!("TCP client connected: {peer}");

        let client_db = db.clone();

        tokio::spawn(async move {
            if let Err(error) =
                handle_client(stream, client_db).await
            {
                eprintln!(
                    "TCP client error ({peer}): {error}"
                );
            }

            println!("TCP client disconnected: {peer}");
        });
    }
}

async fn handle_client(
    mut stream: TcpStream,
    db: SqlitePool,
) -> Result<
    (),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let mut state = ConnectionState {
        user_id: None,
        display_name: None,
        session_id: None,
    };

    loop {
        let mut header = [0u8; HEADER_SIZE];

        match stream.read_exact(&mut header).await {
            Ok(_) => {}

            Err(error)
                if error.kind()
                    == io::ErrorKind::UnexpectedEof =>
            {
                return Ok(());
            }

            Err(error) => {
                return Err(error.into());
            }
        }

        let payload_len = u32::from_be_bytes([
            header[11],
            header[12],
            header[13],
            header[14],
        ]) as usize;

        if payload_len > MAX_PAYLOAD_SIZE {
            return Err(
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "payload exceeds maximum size",
                )
                .into(),
            );
        }

        let mut frame =
            Vec::with_capacity(HEADER_SIZE + payload_len);

        frame.extend_from_slice(&header);

        let mut payload =
            vec![0u8; payload_len];

        stream.read_exact(&mut payload).await?;

        frame.extend_from_slice(&payload);

        // =====================================================
        // DEBUG: PRINT RAW TCP PAYLOAD
        // =====================================================

        println!(
            "TCP RAW PAYLOAD: {}",
            String::from_utf8_lossy(&payload)
        );

        println!(
            "TCP PAYLOAD LENGTH: {}",
            payload_len
        );

        // =====================================================
        // DECODE PACKET
        // =====================================================

        let decoded =
            decode_frame::<ClientMessage>(&frame);

        let (message_id, request_id, message) =
            match decoded {
                Ok(value) => value,

                Err(error) => {
                    eprintln!(
                        "TCP DECODE ERROR: {error}"
                    );

                    eprintln!(
                        "TCP RAW PAYLOAD BYTES: {:?}",
                        payload
                    );

                    return Err(error);
                }
            };

        println!(
            "TCP MESSAGE ID: {}",
            message_id
        );

        println!(
            "TCP REQUEST ID: {}",
            request_id
        );

        let response = handle_message(
            message,
            &db,
            &mut state,
        )
        .await;

        let response_message_id =
            message_id | 0x8000;

        let response_frame =
            encode_frame(
                response_message_id,
                request_id,
                &response,
            )?;

        stream
            .write_all(&response_frame)
            .await?;

        stream.flush().await?;
    }
}

async fn handle_message(
    message: ClientMessage,
    db: &SqlitePool,
    state: &mut ConnectionState,
) -> ServerMessage {
    match message {
        ClientMessage::Authenticate {
            player_id,
            player_name,
        } => {
            let token = Uuid::new_v4().to_string();

            let result = sqlx::query(
                r#"
                INSERT INTO sessions (
                    user_id,
                    display_name,
                    token
                )
                VALUES (?, ?, ?)
                ON CONFLICT(user_id)
                DO UPDATE SET
                    display_name = excluded.display_name,
                    token = excluded.token,
                    last_seen = CURRENT_TIMESTAMP
                "#,
            )
            .bind(&player_id)
            .bind(&player_name)
            .bind(&token)
            .execute(db)
            .await;

            match result {
                Ok(_) => {
                    state.user_id =
                        Some(player_id.clone());

                    state.display_name =
                        Some(player_name.clone());

                    ServerMessage::Authenticated {
                        player: super::types::Player {
                            id: player_id,
                            name: player_name,
                        },
                    }
                }

                Err(error) => {
                    eprintln!(
                        "TCP authentication database error: {error}"
                    );

                    ServerMessage::Error {
                        code: 1001,
                        message:
                            "authentication_failed"
                                .to_string(),
                    }
                }
            }
        }

        ClientMessage::Heartbeat => {
            if let Some(user_id) =
                state.user_id.as_ref()
            {
                let _ = sqlx::query(
                    r#"
                    UPDATE sessions
                    SET last_seen = CURRENT_TIMESTAMP
                    WHERE user_id = ?
                    "#,
                )
                .bind(user_id)
                .execute(db)
                .await;
            }

            ServerMessage::Pong
        }

        ClientMessage::CreateSession => {
            let Some(user_id) =
                state.user_id.as_ref()
            else {
                return ServerMessage::Error {
                    code: 1002,
                    message:
                        "authentication_required"
                            .to_string(),
                };
            };

            let session_id =
                Uuid::new_v4().to_string();

            let lobby_name =
                format!("KLAY Lobby {}", &session_id[..8]);

            let max_players = 8i32;

            let result = sqlx::query(
                r#"
                INSERT INTO lobbies (
                    lobby_id,
                    name,
                    host_user_id,
                    max_players
                )
                VALUES (?, ?, ?, ?)
                "#,
            )
            .bind(&session_id)
            .bind(&lobby_name)
            .bind(user_id)
            .bind(max_players)
            .execute(db)
            .await;

            if let Err(error) = result {
                eprintln!(
                    "Failed to create TCP session: {error}"
                );

                return ServerMessage::Error {
                    code: 1003,
                    message:
                        "session_create_failed"
                            .to_string(),
                };
            }

            let join_result = sqlx::query(
                r#"
                INSERT INTO lobby_players (
                    lobby_id,
                    user_id
                )
                VALUES (?, ?)
                "#,
            )
            .bind(&session_id)
            .bind(user_id)
            .execute(db)
            .await;

            if let Err(error) = join_result {
                eprintln!(
                    "Failed to add session host: {error}"
                );

                let _ = sqlx::query(
                    "DELETE FROM lobbies WHERE lobby_id = ?",
                )
                .bind(&session_id)
                .execute(db)
                .await;

                return ServerMessage::Error {
                    code: 1004,
                    message:
                        "session_host_join_failed"
                            .to_string(),
                };
            }

            state.session_id =
                Some(session_id.clone());

            match load_session(db, &session_id).await {
                Ok(session) => {
                    ServerMessage::SessionCreated {
                        session,
                    }
                }

                Err(error) => {
                    eprintln!(
                        "Failed to load created session: {error}"
                    );

                    ServerMessage::Error {
                        code: 1005,
                        message:
                            "session_load_failed"
                                .to_string(),
                    }
                }
            }
        }

        ClientMessage::JoinSession {
            session_id,
        } => {
            let Some(user_id) =
                state.user_id.as_ref()
            else {
                return ServerMessage::Error {
                    code: 1002,
                    message:
                        "authentication_required"
                            .to_string(),
                };
            };

            let lobby =
                sqlx::query_as::<
                    _,
                    (String, i32),
                >(
                    r#"
                    SELECT
                        name,
                        max_players
                    FROM lobbies
                    WHERE lobby_id = ?
                    "#,
                )
                .bind(&session_id)
                .fetch_optional(db)
                .await;

            let Some((_name, max_players)) =
                lobby.ok().flatten()
            else {
                return ServerMessage::Error {
                    code: 1006,
                    message:
                        "session_not_found"
                            .to_string(),
                };
            };

            let players: i64 =
                sqlx::query_scalar(
                    r#"
                    SELECT COUNT(*)
                    FROM lobby_players
                    WHERE lobby_id = ?
                    "#,
                )
                .bind(&session_id)
                .fetch_one(db)
                .await
                .unwrap_or(0);

            let already_joined: bool =
                sqlx::query_scalar::<_, i64>(
                    r#"
                    SELECT COUNT(*)
                    FROM lobby_players
                    WHERE lobby_id = ?
                    AND user_id = ?
                    "#,
                )
                .bind(&session_id)
                .bind(user_id)
                .fetch_one(db)
                .await
                .unwrap_or(0)
                    > 0;

            if !already_joined
                && players >= max_players as i64
            {
                return ServerMessage::Error {
                    code: 1007,
                    message:
                        "session_full".to_string(),
                };
            }

            if !already_joined {
                let result = sqlx::query(
                    r#"
                    INSERT INTO lobby_players (
                        lobby_id,
                        user_id
                    )
                    VALUES (?, ?)
                    "#,
                )
                .bind(&session_id)
                .bind(user_id)
                .execute(db)
                .await;

                if let Err(error) = result {
                    eprintln!(
                        "Failed to join TCP session: {error}"
                    );

                    return ServerMessage::Error {
                        code: 1008,
                        message:
                            "session_join_failed"
                                .to_string(),
                    };
                }
            }

            state.session_id =
                Some(session_id.clone());

            match load_session(db, &session_id).await {
                Ok(session) => {
                    ServerMessage::SessionJoined {
                        session,
                    }
                }

                Err(error) => {
                    eprintln!(
                        "Failed to load joined session: {error}"
                    );

                    ServerMessage::Error {
                        code: 1005,
                        message:
                            "session_load_failed"
                                .to_string(),
                    }
                }
            }
        }

        ClientMessage::LeaveSession {
            session_id,
        } => {
            let Some(user_id) =
                state.user_id.as_ref()
            else {
                return ServerMessage::Error {
                    code: 1002,
                    message:
                        "authentication_required"
                            .to_string(),
                };
            };

            let result = sqlx::query(
                r#"
                DELETE FROM lobby_players
                WHERE lobby_id = ?
                AND user_id = ?
                "#,
            )
            .bind(&session_id)
            .bind(user_id)
            .execute(db)
            .await;

            match result {
                Ok(_) => {
                    if state.session_id.as_deref()
                        == Some(&session_id)
                    {
                        state.session_id = None;
                    }

                    ServerMessage::SessionLeft {
                        session_id,
                    }
                }

                Err(error) => {
                    eprintln!(
                        "Failed to leave TCP session: {error}"
                    );

                    ServerMessage::Error {
                        code: 1009,
                        message:
                            "session_leave_failed"
                                .to_string(),
                    }
                }
            }
        }
    }
}

async fn load_session(
    db: &SqlitePool,
    session_id: &str,
) -> Result<
    super::types::Session,
    sqlx::Error,
> {
    let exists: Option<(String,)> =
        sqlx::query_as(
            r#"
            SELECT lobby_id
            FROM lobbies
            WHERE lobby_id = ?
            "#,
        )
        .bind(session_id)
        .fetch_optional(db)
        .await?;

    if exists.is_none() {
        return Err(sqlx::Error::RowNotFound);
    }

    let rows =
        sqlx::query_as::<_, (String, String)>(
            r#"
            SELECT
                s.user_id,
                s.display_name
            FROM lobby_players lp
            INNER JOIN sessions s
                ON s.user_id = lp.user_id
            WHERE lp.lobby_id = ?
            ORDER BY lp.joined_at ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(db)
        .await?;

    let players = rows
        .into_iter()
        .map(|(id, name)| {
            super::types::Player {
                id,
                name,
            }
        })
        .collect();

    Ok(super::types::Session {
        id: session_id.to_string(),
        players,
    })
}
