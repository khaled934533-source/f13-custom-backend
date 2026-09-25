use axum::{
    extract::{Path, State},
    http::HeaderValue,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::{env, net::SocketAddr};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

mod protocol;

use protocol::{
    ClientMessage,
    Packet,
    ServerMessage,
};

#[derive(Clone)]
struct AppState {
    db: SqlitePool,
    public_url: String,
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthRequest {
    token: String,
}

#[derive(Debug, Deserialize)]
struct LobbyCreateRequest {
    token: String,
    name: Option<String>,
    max_players: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct LobbyJoinRequest {
    token: String,
}

#[derive(Debug, Serialize)]
struct LoginResponse {
    success: bool,
    token: String,

    #[serde(rename = "userId")]
    user_id: String,

    #[serde(rename = "displayName")]
    display_name: String,

    status: String,
}

#[tokio::main]
async fn main() {
    let host = env::var("HOST")
        .unwrap_or_else(|_| "0.0.0.0".to_string());

    let port = env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string());

    let public_url = env::var("PUBLIC_URL")
        .unwrap_or_else(|_| {
            "https://f13-custom-backend-production.up.railway.app"
                .to_string()
        });

    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:///tmp/f13.db".to_string());

    println!("========================================");
    println!("KLAY Friday the 13th Private Server");
    println!("Version: 0.3.0");
    println!("========================================");
    println!("Host: {host}");
    println!("Port: {port}");
    println!("Server URL: {public_url}");
    println!("Database: {database_url}");
    println!("========================================");

    let db_path = database_url
        .strip_prefix("sqlite://")
        .unwrap_or(&database_url);

    if let Some(parent) = std::path::Path::new(db_path).parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("Failed to create database directory: {error}");
        }
    }

    if !std::path::Path::new(db_path).exists() {
        if let Err(error) = std::fs::File::create(db_path) {
            eprintln!("Failed to create database: {error}");
        }
    }

    let db = SqlitePoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .expect("Failed to connect to SQLite database");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            user_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            token TEXT NOT NULL UNIQUE,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP,
            last_seen TEXT DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&db)
    .await
    .expect("Failed to create sessions table");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS lobbies (
            lobby_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            host_user_id TEXT NOT NULL,
            max_players INTEGER NOT NULL,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&db)
    .await
    .expect("Failed to create lobbies table");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS lobby_players (
            lobby_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            joined_at TEXT DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (lobby_id, user_id)
        )
        "#,
    )
    .execute(&db)
    .await
    .expect("Failed to create lobby_players table");

    println!("Database connected");
    println!("Sessions table ready");
    println!("Lobbies table ready");

    let state = AppState {
        db,
        public_url: public_url.clone(),
    };

    let cors = CorsLayer::new()
        .allow_origin(
            public_url
                .parse::<HeaderValue>()
                .expect("Invalid PUBLIC_URL"),
        )
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    let app = Router::new()
        .route("/", get(home_handler))
        .route("/health", get(health_handler))

        // Protocol
        .route("/api/v1/protocol", post(protocol_handler))

        // Authentication
        .route("/api/v1/login", post(login_handler))
        .route("/api/v1/auth/psn", post(login_handler))
        .route("/api/v1/session/heartbeat", post(heartbeat_handler))
        .route("/api/v1/session/validate", post(validate_session_handler))

        // Profile
        .route("/api/v1/profiles/me", get(profile_handler))

        // Database
        .route("/api/v1/database/status", get(db_check_handler))
        .route("/api/v1/database_check", get(db_check_handler))

        // Server
        .route("/api/v1/server/info", get(server_info_handler))

        // Lobbies
        .route("/api/v1/lobbies", get(list_lobbies_handler))
        .route("/api/v1/lobbies/create", post(create_lobby_handler))
        .route("/api/v1/lobbies/:lobby_id/join", post(join_lobby_handler))
        .route("/api/v1/lobbies/:lobby_id/leave", post(leave_lobby_handler))

        .with_state(state)
        .layer(cors);

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("Invalid HOST or PORT");

    println!("Server listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind server");

    axum::serve(listener, app)
        .await
        .expect("Server failed");
}

async fn home_handler() -> &'static str {
    "KLAY Friday the 13th Private Server v0.3.0"
}

async fn health_handler(
    State(state): State<AppState>,
) -> Json<Value> {
    let database = sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .is_ok();

    Json(json!({
        "status": "ok",
        "service": "f13-custom-backend",
        "version": "0.3.0",
        "database": if database {
            "connected"
        } else {
            "disconnected"
        },
        "public_url": state.public_url
    }))
}

/* =========================================================
   PROTOCOL
   ========================================================= */

async fn protocol_handler(
    Json(packet): Json<Packet<ClientMessage>>,
) -> Json<Packet<ServerMessage>> {
    let response = match packet.payload {
        ClientMessage::Authenticate {
            player_id,
            player_name,
        } => {
            ServerMessage::Authenticated {
                player: protocol::types::Player {
                    id: player_id,
                    name: player_name,
                },
            }
        }

        ClientMessage::CreateSession => {
            ServerMessage::SessionCreated {
                session: protocol::types::Session {
                    id: Uuid::new_v4().to_string(),
                    players: Vec::new(),
                },
            }
        }

        ClientMessage::JoinSession { session_id } => {
            ServerMessage::SessionJoined {
                session: protocol::types::Session {
                    id: session_id,
                    players: Vec::new(),
                },
            }
        }

        ClientMessage::LeaveSession { session_id } => {
            ServerMessage::SessionLeft {
                session_id,
            }
        }

        ClientMessage::Heartbeat => {
            ServerMessage::Pong
        }
    };

    Json(Packet {
        version: packet.version,
        request_id: packet.request_id,
        payload: response,
    })
}

/* =========================================================
   LOGIN
   ========================================================= */

async fn login_handler(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Json<Value> {
    let display_name = request
        .display_name
        .unwrap_or_else(|| "KLAY_Player".to_string());

    let user_id = Uuid::new_v4().to_string();
    let token = Uuid::new_v4().to_string();

    let result = sqlx::query(
        r#"
        INSERT INTO sessions (
            user_id,
            display_name,
            token
        )
        VALUES (?, ?, ?)
        "#,
    )
    .bind(&user_id)
    .bind(&display_name)
    .bind(&token)
    .execute(&state.db)
    .await;

    if let Err(error) = result {
        eprintln!("Failed to create session: {error}");

        return Json(json!({
            "success": false,
            "status": "database_error"
        }));
    }

    println!(
        "New session: user={} name={}",
        user_id, display_name
    );

    Json(json!(LoginResponse {
        success: true,
        token,
        user_id,
        display_name,
        status: "success".to_string(),
    }))
}

/* =========================================================
   HEARTBEAT
   ========================================================= */

async fn heartbeat_handler(
    State(state): State<AppState>,
    Json(request): Json<AuthRequest>,
) -> Json<Value> {
    let result = sqlx::query(
        r#"
        UPDATE sessions
        SET last_seen = CURRENT_TIMESTAMP
        WHERE token = ?
        "#,
    )
    .bind(&request.token)
    .execute(&state.db)
    .await;

    match result {
        Ok(result) if result.rows_affected() > 0 => {
            Json(json!({
                "success": true,
                "status": "active"
            }))
        }

        _ => Json(json!({
            "success": false,
            "status": "invalid_session"
        })),
    }
}

/* =========================================================
   SESSION VALIDATION
   ========================================================= */

async fn validate_session_handler(
    State(state): State<AppState>,
    Json(request): Json<AuthRequest>,
) -> Json<Value> {
    let session = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT user_id, display_name
        FROM sessions
        WHERE token = ?
        "#,
    )
    .bind(&request.token)
    .fetch_optional(&state.db)
    .await;

    match session {
        Ok(Some((user_id, display_name))) => {
            Json(json!({
                "success": true,
                "valid": true,
                "userId": user_id,
                "displayName": display_name
            }))
        }

        _ => Json(json!({
            "success": true,
            "valid": false
        })),
    }
}

/* =========================================================
   PROFILE
   ========================================================= */

async fn profile_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Json<Value> {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .trim_start_matches("Bearer ");

    if !token.is_empty() {
        let valid = sqlx::query(
            "SELECT user_id FROM sessions WHERE token = ?",
        )
        .bind(token)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some();

        if !valid {
            return Json(json!({
                "success": false,
                "error": "invalid_session"
            }));
        }
    }

    Json(json!({
        "success": true,
        "level": 150,
        "cp": 999999,
        "dlc_unlocked": true,
        "unlocked_assets": [
            "savini_jason",
            "backers_clothing_pack",
            "counselor_clothing_dlc"
        ],
        "customization": {
            "perkSlots": 3,
            "badgeSlots": 3
        }
    }))
}

/* =========================================================
   CREATE LOBBY
   ========================================================= */

async fn create_lobby_handler(
    State(state): State<AppState>,
    Json(request): Json<LobbyCreateRequest>,
) -> Json<Value> {
    let session = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT user_id, display_name
        FROM sessions
        WHERE token = ?
        "#,
    )
    .bind(&request.token)
    .fetch_optional(&state.db)
    .await;

    let Some((user_id, _)) = session.ok().flatten() else {
        return Json(json!({
            "success": false,
            "error": "invalid_session"
        }));
    };

    let lobby_id = Uuid::new_v4().to_string();

    let name = request
        .name
        .unwrap_or_else(|| "KLAY Lobby".to_string());

    let max_players = request
        .max_players
        .unwrap_or(8)
        .clamp(1, 8);

    if sqlx::query(
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
    .bind(&lobby_id)
    .bind(&name)
    .bind(&user_id)
    .bind(max_players)
    .execute(&state.db)
    .await
    .is_err()
    {
        return Json(json!({
            "success": false,
            "error": "lobby_create_failed"
        }));
    }

    let _ = sqlx::query(
        r#"
        INSERT INTO lobby_players (
            lobby_id,
            user_id
        )
        VALUES (?, ?)
        "#,
    )
    .bind(&lobby_id)
    .bind(&user_id)
    .execute(&state.db)
    .await;

    println!("Lobby created: {lobby_id}");

    Json(json!({
        "success": true,
        "lobbyId": lobby_id,
        "name": name,
        "hostUserId": user_id,
        "maxPlayers": max_players,
        "players": 1
    }))
}

/* =========================================================
   LIST LOBBIES
   ========================================================= */

async fn list_lobbies_handler(
    State(state): State<AppState>,
) -> Json<Value> {
    let lobbies = sqlx::query_as::<_, (String, String, String, i32)>(
        r#"
        SELECT
            lobby_id,
            name,
            host_user_id,
            max_players
        FROM lobbies
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut result = Vec::new();

    for (lobby_id, name, host_user_id, max_players) in lobbies {
        let players: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM lobby_players
            WHERE lobby_id = ?
            "#,
        )
        .bind(&lobby_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

        result.push(json!({
            "lobbyId": lobby_id,
            "name": name,
            "hostUserId": host_user_id,
            "maxPlayers": max_players,
            "players": players
        }));
    }

    Json(json!({
        "success": true,
        "lobbies": result
    }))
}

/* =========================================================
   JOIN LOBBY
   ========================================================= */

async fn join_lobby_handler(
    State(state): State<AppState>,
    Path(lobby_id): Path<String>,
    Json(request): Json<LobbyJoinRequest>,
) -> Json<Value> {
    let user_id: Option<String> = sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE token = ?",
    )
    .bind(&request.token)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let Some(user_id) = user_id else {
        return Json(json!({
            "success": false,
            "error": "invalid_session"
        }));
    };

    let lobby = sqlx::query_as::<_, (String, i32)>(
        r#"
        SELECT name, max_players
        FROM lobbies
        WHERE lobby_id = ?
        "#,
    )
    .bind(&lobby_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some((name, max_players)) = lobby else {
        return Json(json!({
            "success": false,
            "error": "lobby_not_found"
        }));
    };

    let players: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM lobby_players WHERE lobby_id = ?",
    )
    .bind(&lobby_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    if players >= max_players as i64 {
        return Json(json!({
            "success": false,
            "error": "lobby_full"
        }));
    }

    let result = sqlx::query(
        r#"
        INSERT OR IGNORE INTO lobby_players (
            lobby_id,
            user_id
        )
        VALUES (?, ?)
        "#,
    )
    .bind(&lobby_id)
    .bind(&user_id)
    .execute(&state.db)
    .await;

    if result.is_err() {
        return Json(json!({
            "success": false,
            "error": "join_failed"
        }));
    }

    Json(json!({
        "success": true,
        "lobbyId": lobby_id,
        "name": name,
        "players": players + 1,
        "maxPlayers": max_players
    }))
}

/* =========================================================
   LEAVE LOBBY
   ========================================================= */

async fn leave_lobby_handler(
    State(state): State<AppState>,
    Path(lobby_id): Path<String>,
    Json(request): Json<LobbyJoinRequest>,
) -> Json<Value> {
    let user_id: Option<String> = sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE token = ?",
    )
    .bind(&request.token)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let Some(user_id) = user_id else {
        return Json(json!({
            "success": false,
            "error": "invalid_session"
        }));
    };

    let result = sqlx::query(
        r#"
        DELETE FROM lobby_players
        WHERE lobby_id = ? AND user_id = ?
        "#,
    )
    .bind(&lobby_id)
    .bind(&user_id)
    .execute(&state.db)
    .await;

    match result {
        Ok(result) => Json(json!({
            "success": true,
            "removed": result.rows_affected() > 0
        })),

        Err(error) => {
            eprintln!("Failed to leave lobby: {error}");

            Json(json!({
                "success": false,
                "error": "leave_failed"
            }))
        }
    }
}

/* =========================================================
   DATABASE CHECK
   ========================================================= */

async fn db_check_handler(
    State(state): State<AppState>,
) -> Json<Value> {
    let connected = sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .is_ok();

    Json(json!({
        "status": if connected {
            "online"
        } else {
            "degraded"
        },
        "database": if connected {
            "connected"
        } else {
            "disconnected"
        },
        "healthy": connected
    }))
}

/* =========================================================
   SERVER INFO
   ========================================================= */

async fn server_info_handler(
    State(state): State<AppState>,
) -> Json<Value> {
    Json(json!({
        "name": "KLAY Friday the 13th Private Server",
        "status": "online",
        "backend": "rust-axum",
        "database": "sqlite",
        "public_url": state.public_url,
        "version": "0.3.0",
        "features": [
            "sessions",
            "heartbeat",
            "session_validation",
            "lobbies",
            "lobby_join",
            "lobby_leave",
            "protocol"
        ]
    }))
}
