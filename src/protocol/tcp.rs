use std::io;

use sqlx::SqlitePool;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use uuid::Uuid;

use super::{
    codec::{decode_frame, encode_frame, HEADER_SIZE, MAX_PAYLOAD_SIZE},
    types::{Player, Session},
    ClientMessage, ServerMessage,
};

pub const MSG_AUTHENTICATE: u16 = 0x0001;
pub const MSG_HEARTBEAT: u16 = 0x0002;
pub const MSG_CREATE_SESSION: u16 = 0x0003;
pub const MSG_JOIN_SESSION: u16 = 0x0004;
pub const MSG_LEAVE_SESSION: u16 = 0x0005;
pub const MSG_SET_READY: u16 = 0x0006;
pub const MSG_START_SESSION: u16 = 0x0007;

#[derive(Debug, Clone)]
struct ConnectionState {
    user_id: Option<String>,
    display_name: Option<String>,
    session_id: Option<String>,
    ready: bool,
    started: bool,
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
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut state = ConnectionState {
        user_id: None,
        display_name: None,
        session_id: None,
        ready: false,
        started: false,
    };

    loop {
        let mut header = [0u8; HEADER_SIZE];

        match stream.read_exact(&mut header).await {
            Ok(_) => {}

            Err(error)
                if error.kind()
                    == io::ErrorKind::UnexpectedEof =>
            {
                cleanup_connection(&db, &mut state).await;
                return Ok(());
            }

            Err(error) => {
                cleanup_connection(&db, &mut state).await;
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
            cleanup_connection(&db, &mut state).await;

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

        if let Err(error) =
            stream.read_exact(&mut payload).await
        {
            cleanup_connection(&db, &mut state).await;
            return Err(error.into());
        }

        frame.extend_from_slice(&payload);

        let decoded =
            decode_frame::<ClientMessage>(&frame);

        let (message_id, request_id, message) =
            match decoded {
                Ok(value) => value,

                Err(error) => {
                    cleanup_connection(
                        &db,
                        &mut state,
                    )
                    .await;

                    return Err(error);
                }
            };

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

        if let Err(error) =
            stream.write_all(&response_frame).await
        {
            cleanup_connection(&db, &mut state).await;
            return Err(error.into());
        }

        if let Err(error) =
            stream.flush().await
        {
            cleanup_connection(&db, &mut state).await;
            return Err(error.into());
        }
    }
}

async fn cleanup_connection(
    db: &SqlitePool,
    state: &mut ConnectionState,
) {
    let Some(user_id) = state.user_id.as_ref()
    else {
        return;
    };

    let Some(session_id) = state.session_id.as_ref()
    else {
        return;
    };

    println!(
        "Cleaning up disconnected player: user_id={user_id}, session_id={session_id}"
    );

    let delete_result = sqlx::query(
        r#"
        DELETE FROM lobby_players
        WHERE lobby_id = ?
        AND user_id = ?
        "#,
    )
    .bind(session_id)
    .bind(user_id)
    .execute(db)
    .await;

    if let Err(error) = delete_result {
        eprintln!(
            "Failed to cleanup disconnected player: {error}"
        );

        return;
    }

    let remaining_players: i64 =
        sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM lobby_players
            WHERE lobby_id = ?
            "#,
        )
        .bind(session_id)
        .fetch_one(db)
        .await
        .unwrap_or(0);

    if remaining_players == 0 {
        let delete_lobby_result =
            sqlx::query(
                r#"
                DELETE FROM lobbies
                WHERE lobby_id = ?
                "#,
            )
            .bind(session_id)
            .execute(db)
            .await;

        if let Err(error) =
            delete_lobby_result
        {
            eprintln!(
                "Failed to delete empty lobby: {error}"
            );
        } else {
            println!(
                "Deleted empty lobby: {session_id}"
            );
        }
    }

    state.session_id = None;
    state.ready = false;
    state.started = false;
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
            let token =
                Uuid::new_v4().to_string();

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
            .execute(db)
            .await;

            match result {
                Ok(_) => {
                    state.user_id =
                        Some(player_id.clone());

                    state.display_name =
                        Some(player_name.clone());

                    state.ready = false;
                    state.started = false;

                    let _ = token;

                    ServerMessage::Authenticated {
                        player: Player {
                            id: player_id,
                            name: player_name,
                            ready: false,
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
                format!(
                    "KLAY Lobby {}",
                    &session_id[..8]
                );

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

            state.ready = false;
            state.started = false;

            match load_session(
                db,
                &session_id,
                state.started,
            )
            .await
            {
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
                && players
                    >= max_players as i64
            {
                return ServerMessage::Error {
                    code: 1007,
                    message:
                        "session_full"
                            .to_string(),
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

            state.ready = false;
            state.started = false;

            match load_session(
                db,
                &session_id,
                state.started,
            )
            .await
            {
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
                    if state
                        .session_id
                        .as_deref()
                        == Some(&session_id)
                    {
                        state.session_id =
                            None;
                        state.ready = false;
                        state.started = false;
                    }

                    let remaining_players:
                        i64 =
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

                    if remaining_players == 0 {
                        let _ =
                            sqlx::query(
                                r#"
                                DELETE FROM lobbies
                                WHERE lobby_id = ?
                                "#,
                            )
                            .bind(&session_id)
                            .execute(db)
                            .await;
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

        ClientMessage::SetReady { ready } => {
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

            if state.session_id.is_none() {
                return ServerMessage::Error {
                    code: 1010,
                    message:
                        "session_required"
                            .to_string(),
                };
            }

            if state.started {
                return ServerMessage::Error {
                    code: 1011,
                    message:
                        "session_already_started"
                            .to_string(),
                };
            }

            state.ready = ready;

            ServerMessage::ReadyChanged {
                player_id: user_id.clone(),
                ready,
            }
        }

        ClientMessage::StartSession => {
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

            let Some(session_id) =
                state.session_id.clone()
            else {
                return ServerMessage::Error {
                    code: 1010,
                    message:
                        "session_required"
                            .to_string(),
                };
            };

            let host_user_id =
                sqlx::query_scalar::<_, String>(
                    r#"
                    SELECT host_user_id
                    FROM lobbies
                    WHERE lobby_id = ?
                    "#,
                )
                .bind(&session_id)
                .fetch_optional(db)
                .await;

            let Some(host_user_id) =
                host_user_id.ok().flatten()
            else {
                return ServerMessage::Error {
                    code: 1006,
                    message:
                        "session_not_found"
                            .to_string(),
                };
            };

            if host_user_id != *user_id {
                return ServerMessage::Error {
                    code: 1012,
                    message:
                        "host_required"
                            .to_string(),
                };
            }

            let player_count: i64 =
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

            if player_count == 0 {
                return ServerMessage::Error {
                    code: 1013,
                    message:
                        "session_empty"
                            .to_string(),
                };
            }

            state.started = true;

            ServerMessage::SessionStarted {
                session_id,
            }
        }
    }
}

async fn load_session(
    db: &SqlitePool,
    session_id: &str,
    started: bool,
) -> Result<Session, sqlx::Error> {
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
        return Err(
            sqlx::Error::RowNotFound
        );
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
            Player {
                id,
                name,
                ready: false,
            }
        })
        .collect();

    Ok(Session {
        id: session_id.to_string(),
        players,
        started,
    })
}
