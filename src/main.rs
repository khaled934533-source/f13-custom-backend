use std::env;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header::SET_COOKIE};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::routing::post;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use sqlx::SqlitePool;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions,
};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

mod protocol;


const SERVER_VERSION: &str = "0.5.0";
const DEFAULT_MAX_PLAYERS: i32 = 8;

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

#[derive(Debug, Deserialize)]
struct OfflineMatchRequest {
    token: String,
    match_id: String,
    mode: String,
    completed: bool,
}

#[derive(Debug, Deserialize)]
struct OfflineStartRequest {
    token: String,
    match_id: String,
    mode: String,
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
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());

    let port = env::var("PORT").unwrap_or_else(|_| "8080".to_string());

    let public_url = env::var("PUBLIC_URL")
        .unwrap_or_else(|_| "https://f13-custom-backend-production.up.railway.app".to_string());

    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:///tmp/f13.db".to_string());

    println!("========================================");
    println!("KLAY Friday the 13th Private Server");
    println!("Version: {SERVER_VERSION}");
    println!("========================================");
    println!("Host: {host}");
    println!("Port: {port}");
    println!("Server URL: {public_url}");
    println!("Database: {database_url}");
    println!("========================================");

    let sqlite_options = SqliteConnectOptions::from_str(&database_url)
        .expect("Invalid DATABASE_URL")
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));

    if let Some(parent) = std::path::Path::new(sqlite_options.get_filename())
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .expect("Failed to create database directory");
    }

    let db = SqlitePoolOptions::new()
        .max_connections(10)
        .connect_with(sqlite_options)
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

    sqlx::query("CREATE TABLE IF NOT EXISTS epic_oauth_states (state TEXT PRIMARY KEY, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)")
        .execute(&db).await.expect("Failed to create Epic OAuth state table");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS lobbies (
            lobby_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            host_user_id TEXT NOT NULL,
            max_players INTEGER NOT NULL,
            started INTEGER NOT NULL DEFAULT 0,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&db)
    .await
    .expect("Failed to create lobbies table");

    let started_column: Option<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('lobbies') WHERE name = 'started'",
    )
    .fetch_optional(&db)
    .await
    .expect("Failed to inspect lobbies table");

    if started_column.is_none() {
        sqlx::query(
            "ALTER TABLE lobbies ADD COLUMN started INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&db)
        .await
        .expect("Failed to add started column to lobbies");
    }

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS lobby_players (
            lobby_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            joined_at TEXT DEFAULT CURRENT_TIMESTAMP,
            ready INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (lobby_id, user_id)
        )
        "#,
    )
    .execute(&db)
    .await
    .expect("Failed to create lobby_players table");

    let ready_column: Option<String> = sqlx::query_scalar(
        r#"
        SELECT name
        FROM pragma_table_info('lobby_players')
        WHERE name = 'ready'
        "#,
    )
    .fetch_optional(&db)
    .await
    .expect("Failed to inspect lobby_players table");

    if ready_column.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE lobby_players
            ADD COLUMN ready INTEGER NOT NULL DEFAULT 0
            "#,
        )
        .execute(&db)
        .await
        .expect("Failed to add ready column to lobby_players");

        println!("Lobby players ready column added");
    }

    // Enforce lobby capacity inside SQLite as well as in request handlers.
    // SQLite serializes writers, so this trigger closes the check-then-insert race
    // when multiple clients attempt to occupy the final slot simultaneously.
    sqlx::query(
        r#"
        CREATE TRIGGER IF NOT EXISTS enforce_lobby_capacity
        BEFORE INSERT ON lobby_players
        WHEN NOT EXISTS (
            SELECT 1
            FROM lobby_players
            WHERE lobby_id = NEW.lobby_id
              AND user_id = NEW.user_id
        )
        AND (
            SELECT COUNT(*)
            FROM lobby_players
            WHERE lobby_id = NEW.lobby_id
        ) >= COALESCE(
            (SELECT max_players FROM lobbies WHERE lobby_id = NEW.lobby_id),
            0
        )
        BEGIN
            SELECT RAISE(ABORT, 'lobby_full');
        END
        "#,
    )
    .execute(&db)
    .await
    .expect("Failed to create lobby capacity trigger");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS offline_matches (
            user_id TEXT NOT NULL,
            match_id TEXT NOT NULL,
            mode TEXT NOT NULL CHECK (mode IN ('counselor', 'jason')),
            completed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (user_id, match_id)
        )
        "#,
    )
    .execute(&db)
    .await
    .expect("Failed to create offline_matches table");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS klay_achievements (
            user_id TEXT NOT NULL,
            achievement_id TEXT NOT NULL,
            unlocked_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (user_id, achievement_id)
        )
        "#,
    )
    .execute(&db)
    .await
    .expect("Failed to create klay_achievements table");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS offline_match_starts (
            user_id TEXT NOT NULL,
            match_id TEXT NOT NULL,
            mode TEXT NOT NULL CHECK (mode IN ('counselor', 'jason')),
            started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (user_id, match_id)
        )
        "#,
    )
    .execute(&db)
    .await
    .expect("Failed to create offline_match_starts table");

    for statement in [
        "CREATE INDEX IF NOT EXISTS idx_lobby_players_lobby_joined ON lobby_players(lobby_id, joined_at, user_id)",
        "CREATE INDEX IF NOT EXISTS idx_lobby_players_user ON lobby_players(user_id)",
        "CREATE INDEX IF NOT EXISTS idx_lobbies_created_at ON lobbies(created_at)",
        "CREATE INDEX IF NOT EXISTS idx_offline_matches_user_mode ON offline_matches(user_id, mode)",
        "CREATE INDEX IF NOT EXISTS idx_klay_achievements_user_unlocked ON klay_achievements(user_id, unlocked_at)",
    ] {
        sqlx::query(statement)
            .execute(&db)
            .await
            .expect("Failed to create database index");
    }

    println!("Database connected");
    println!("Sessions table ready");
    println!("Lobbies table ready");
    println!("Lobby players table ready");
    println!("Offline matches table ready");
    println!("KLAY achievements table ready");

    let state = AppState {
        db: db.clone(),
        public_url: public_url.clone(),
    };

    let tcp_host = env::var("TCP_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());

    let tcp_port = env::var("TCP_PORT").unwrap_or_else(|_| "9000".to_string());

    let tcp_addr = format!("{tcp_host}:{tcp_port}");
    let tcp_db = db.clone();

    tokio::spawn(async move {
        if let Err(error) = protocol::tcp::run_tcp_server(tcp_addr, tcp_db).await {
            eprintln!("Protocol TCP server stopped: {error}");
        }
    });

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
        .route("/api/v1/protocol", post(protocol_unavailable_handler))
        .route("/api/v1/login", post(login_handler))
        .route("/api/v1/auth/psn", post(psn_auth_unavailable_handler))
        .route("/auth/epic/login", get(epic_login_handler))
        .route("/auth/epic/callback", get(epic_callback_handler))
        .route("/auth/epic/me", get(epic_me_handler))
        .route("/api/v1/session/heartbeat", post(heartbeat_handler))
        .route("/api/v1/session/validate", post(validate_session_handler))
        .route("/api/v1/profiles/me", get(profile_handler))
        .route(
            "/api/v1/trophies/offline-start",
            post(record_offline_start_handler),
        )
        .route(
            "/api/v1/trophies/offline-match",
            post(record_offline_match_handler),
        )
        .route("/api/v1/trophies/progress", post(trophy_progress_handler))
        .route("/api/v1/database/status", get(db_check_handler))
        .route("/api/v1/database_check", get(db_check_handler))
        .route("/api/v1/server/info", get(server_info_handler))
        .route("/api/v1/lobbies", get(list_lobbies_handler))
        .route("/api/v1/lobbies/create", post(create_lobby_handler))
        .route("/api/v1/lobbies/:lobby_id", get(get_lobby_handler))
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

    axum::serve(listener, app).await.expect("Server failed");
}


#[derive(Debug, Deserialize)]
struct EpicCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EpicTokenResponse {
    access_token: String,
    account_id: String,
}

#[derive(Debug, Deserialize)]
struct EpicAccountProfile {
    #[serde(rename = "accountId")]
    account_id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

fn request_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(axum::http::header::COOKIE)?.to_str().ok()?
        .split(';').filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| if key == name { Some(value.to_string()) } else { None })
}

fn expire_oauth_cookie(response: &mut Response) {
    response.headers_mut().append(SET_COOKIE, HeaderValue::from_static(
        "klay_oauth_state=; Path=/auth/epic; Max-Age=0; HttpOnly; Secure; SameSite=Lax"
    ));
}

fn oauth_error(status: StatusCode, message: &'static str) -> Response {
    let mut response = (status, message).into_response();
    expire_oauth_cookie(&mut response);
    response
}

async fn epic_login_handler(State(state): State<AppState>) -> Response {
    let client_id = match env::var("EPIC_CLIENT_ID") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => return (StatusCode::SERVICE_UNAVAILABLE, "Missing EPIC_CLIENT_ID").into_response(),
    };
    let redirect_uri = match env::var("EPIC_REDIRECT_URI") {
        Ok(value) if value.starts_with("https://") => value,
        _ => return (StatusCode::SERVICE_UNAVAILABLE, "Set EPIC_REDIRECT_URI to the registered HTTPS callback").into_response(),
    };

    let state_value = Uuid::new_v4().simple().to_string();
    if let Err(error) = sqlx::query("INSERT INTO epic_oauth_states (state) VALUES (?)")
        .bind(&state_value).execute(&state.db).await {
        eprintln!("Epic OAuth state insert failed: {error}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Could not start Epic sign-in").into_response();
    }

    let mut url = match reqwest::Url::parse("https://www.epicgames.com/id/authorize") {
        Ok(url) => url,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid Epic authorization URL").into_response(),
    };
    url.query_pairs_mut()
        .append_pair("client_id", &client_id)
        .append_pair("response_type", "code")
        .append_pair("scope", "basic_profile")
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("state", &state_value);

    let mut response = Redirect::to(url.as_str()).into_response();
    let cookie = format!("klay_oauth_state={state_value}; Path=/auth/epic; Max-Age=600; HttpOnly; Secure; SameSite=Lax");
    match HeaderValue::from_str(&cookie) {
        Ok(value) => { response.headers_mut().insert(SET_COOKIE, value); response }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Could not set OAuth state cookie").into_response(),
    }
}

async fn epic_callback_handler(
    State(state): State<AppState>,
    Query(query): Query<EpicCallbackQuery>,
    headers: HeaderMap,
) -> Response {
    if query.error.is_some() {
        return oauth_error(StatusCode::UNAUTHORIZED, "Epic sign-in was cancelled or denied");
    }
    let code = match query.code.as_deref() {
        Some(value) if !value.trim().is_empty() => value,
        _ => return oauth_error(StatusCode::BAD_REQUEST, "Missing Epic authorization code"),
    };
    let returned_state = match query.state.as_deref() {
        Some(value) => value,
        None => return oauth_error(StatusCode::BAD_REQUEST, "Missing OAuth state"),
    };
    let browser_state = match request_cookie(&headers, "klay_oauth_state") {
        Some(value) => value,
        None => return oauth_error(StatusCode::BAD_REQUEST, "Missing browser state cookie"),
    };
    if returned_state != browser_state {
        return oauth_error(StatusCode::BAD_REQUEST, "OAuth state mismatch");
    }

    let consumed = sqlx::query("DELETE FROM epic_oauth_states WHERE state = ? AND created_at >= datetime('now', '-10 minutes')")
        .bind(returned_state).execute(&state.db).await;
    match consumed {
        Ok(result) if result.rows_affected() == 1 => {}
        Ok(_) => return oauth_error(StatusCode::BAD_REQUEST, "OAuth state expired or already used"),
        Err(error) => {
            eprintln!("Epic OAuth state validation failed: {error}");
            return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "Could not validate OAuth state");
        }
    }

    let client_id = match env::var("EPIC_CLIENT_ID") { Ok(v) if !v.is_empty() => v, _ => return oauth_error(StatusCode::SERVICE_UNAVAILABLE, "Missing EPIC_CLIENT_ID") };
    let client_secret = match env::var("EPIC_CLIENT_SECRET") { Ok(v) if !v.is_empty() => v, _ => return oauth_error(StatusCode::SERVICE_UNAVAILABLE, "Missing EPIC_CLIENT_SECRET") };
    let deployment_id = match env::var("EPIC_DEPLOYMENT_ID") { Ok(v) if !v.is_empty() => v, _ => return oauth_error(StatusCode::SERVICE_UNAVAILABLE, "Missing EPIC_DEPLOYMENT_ID") };
    let redirect_uri = match env::var("EPIC_REDIRECT_URI") { Ok(v) if v.starts_with("https://") => v, _ => return oauth_error(StatusCode::SERVICE_UNAVAILABLE, "Missing HTTPS callback URI") };

    let token_response = reqwest::Client::new()
        .post("https://api.epicgames.dev/epic/oauth/v2/token")
        .basic_auth(client_id, Some(client_secret))
        .form(&[
            ("grant_type", "authorization_code"),
            ("deployment_id", deployment_id.as_str()),
            ("scope", "basic_profile"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send().await;

    let token_response = match token_response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            eprintln!("Epic token exchange returned HTTP {}", response.status());
            return oauth_error(StatusCode::UNAUTHORIZED, "Epic rejected the authorization code");
        }
        Err(error) => {
            eprintln!("Epic token exchange request failed: {error}");
            return oauth_error(StatusCode::BAD_GATEWAY, "Could not reach Epic authentication");
        }
    };
    let token: EpicTokenResponse = match token_response.json().await {
        Ok(value) if !value.account_id.trim().is_empty() && !value.access_token.trim().is_empty() => value,
        _ => return oauth_error(StatusCode::BAD_GATEWAY, "Epic returned an invalid token response"),
    };

    let user_id = format!("epic:{}", token.account_id);
    let fallback_name = format!("Epic_{}", &token.account_id[..token.account_id.len().min(8)]);
    let display_name = match reqwest::Client::new()
        .get("https://api.epicgames.dev/epic/id/v2/accounts")
        .query(&[("accountId", token.account_id.as_str())])
        .bearer_auth(&token.access_token)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            match response.json::<Vec<EpicAccountProfile>>().await {
                Ok(profiles) => profiles.into_iter()
                    .find(|profile| profile.account_id == token.account_id)
                    .and_then(|profile| profile.display_name)
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(fallback_name.clone()),
                Err(_) => fallback_name,
            }
        }
        _ => fallback_name,
    };
    let session_token = Uuid::new_v4().to_string();
    let saved = sqlx::query(
        "INSERT INTO sessions (user_id, display_name, token) VALUES (?, ?, ?) ON CONFLICT(user_id) DO UPDATE SET display_name = excluded.display_name, token = excluded.token, last_seen = CURRENT_TIMESTAMP"
    ).bind(&user_id).bind(&display_name).bind(&session_token).execute(&state.db).await;
    if let Err(error) = saved {
        eprintln!("Epic session insert failed: {error}");
        return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "Could not create KLAY session");
    }

    let mut response = Redirect::to("/").into_response();
    let cookie = format!("klay_session={session_token}; Path=/; Max-Age=604800; HttpOnly; Secure; SameSite=Lax");
    match HeaderValue::from_str(&cookie) {
        Ok(value) => {
            response.headers_mut().insert(SET_COOKIE, value);
            expire_oauth_cookie(&mut response);
            response
        }
        Err(_) => oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "Could not create secure session cookie"),
    }
}

async fn epic_me_handler(State(state): State<AppState>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    let token = match request_cookie(&headers, "klay_session") {
        Some(value) if !value.trim().is_empty() => value,
        _ => return (StatusCode::UNAUTHORIZED, Json(json!({"success": false, "error": "authentication_required"}))),
    };
    let session = sqlx::query_as::<_, (String, String)>("SELECT user_id, display_name FROM sessions WHERE token = ?")
        .bind(token).fetch_optional(&state.db).await;
    match session {
        Ok(Some((user_id, display_name))) if user_id.starts_with("epic:") =>
            (StatusCode::OK, Json(json!({"success": true, "userId": user_id, "displayName": display_name, "provider": "epic"}))),
        Ok(_) => (StatusCode::UNAUTHORIZED, Json(json!({"success": false, "error": "invalid_session"}))),
        Err(error) => {
            eprintln!("Epic session lookup failed: {error}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"success": false, "error": "database_error"})))
        }
    }
}

async fn home_handler() -> &'static str {
    "KLAY Friday the 13th Private Server v0.5.0"
}

async fn health_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<Value>) {
    let database = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();
    let status = if database {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(json!({
            "status": if database { "ok" } else { "degraded" },
            "service": "f13-custom-backend",
            "version": SERVER_VERSION,
            "database": if database { "connected" } else { "disconnected" },
            "public_url": state.public_url
        })),
    )
}

async fn protocol_unavailable_handler() -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "success": false,
            "error": "original_game_protocol_not_implemented",
            "message": "This HTTP endpoint is not a substitute for the original game's protocol."
        })),
    )
}

async fn psn_auth_unavailable_handler() -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "success": false,
            "error": "psn_authentication_not_implemented",
            "message": "Use /api/v1/login only for a KLAY test account. This service does not authenticate PlayStation Network accounts."
        })),
    )
}

async fn login_handler(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Json<Value> {
    let display_name = request
        .display_name
        .unwrap_or_else(|| "KLAY_Player".to_string())
        .trim()
        .to_string();

    if display_name.is_empty() || display_name.chars().count() > 32 {
        return Json(json!({
            "success": false,
            "status": "invalid_display_name",
            "message": "Display name must contain between 1 and 32 characters."
        }));
    }

    let user_id = Uuid::new_v4().to_string();
    let token = Uuid::new_v4().to_string();

    let result = sqlx::query(
        r#"
        INSERT INTO sessions (user_id, display_name, token)
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

    println!("New session: user={} name={}", user_id, display_name);

    Json(json!(LoginResponse {
        success: true,
        token,
        user_id,
        display_name,
        status: "success".to_string(),
    }))
}

async fn heartbeat_handler(
    State(state): State<AppState>,
    Json(request): Json<AuthRequest>,
) -> Json<Value> {
    let token = request.token.trim();
    if token.is_empty() {
        return Json(json!({"success": false, "error": "missing_token"}));
    }

    match sqlx::query(
        "UPDATE sessions SET last_seen = CURRENT_TIMESTAMP WHERE token = ?",
    )
    .bind(token)
    .execute(&state.db)
    .await
    {
        Ok(result) if result.rows_affected() > 0 => Json(json!({
            "success": true,
            "status": "active"
        })),
        Ok(_) => Json(json!({
            "success": false,
            "error": "invalid_session"
        })),
        Err(error) => {
            eprintln!("HTTP heartbeat update failed: {error}");
            Json(json!({"success": false, "error": "database_error"}))
        }
    }
}

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
        Ok(Some((user_id, display_name))) => Json(json!({
            "success": true,
            "valid": true,
            "userId": user_id,
            "displayName": display_name
        })),

        Ok(None) => Json(json!({
            "success": true,
            "valid": false
        })),
        Err(error) => {
            eprintln!("Session validation query failed: {error}");
            Json(json!({
                "success": false,
                "error": "database_error"
            }))
        },
    }
}

async fn profile_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Json<Value> {
    let token = match headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    {
        Some(token) if !token.trim().is_empty() => token.trim(),
        _ => {
            return Json(json!({
                "success": false,
                "error": "authentication_required"
            }));
        }
    };

    let session = sqlx::query_as::<_, (String, String)>(
        "SELECT user_id, display_name FROM sessions WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await;

    let (user_id, display_name) = match session {
        Ok(Some(session)) => session,
        Ok(None) => {
            return Json(json!({
                "success": false,
                "error": "invalid_session"
            }));
        }
        Err(_) => {
            return Json(json!({
                "success": false,
                "error": "database_error"
            }));
        }
    };

    Json(json!({
        "success": true,
        "profile": {
            "user_id": user_id,
            "display_name": display_name
        },
        "custom_catalog": {
            "assets": [
                "savini_jason",
                "backers_clothing_pack",
                "counselor_clothing_dlc"
            ],
            "customization": {
                "perkSlots": 3,
                "badgeSlots": 3
            }
        },
        "note": "Custom catalog only. Official game and platform entitlements are unchanged."
    }))
}
async fn record_offline_start_handler(
    State(state): State<AppState>,
    Json(request): Json<OfflineStartRequest>,
) -> Json<Value> {
    let token = request.token.trim();
    let match_id = request.match_id.trim();

    if token.is_empty() || match_id.is_empty() {
        return Json(json!({"success": false, "error": "missing_token_or_match_id"}));
    }
    if match_id.len() > 128 {
        return Json(json!({"success": false, "error": "match_id_too_long"}));
    }
    if request.mode != "counselor" && request.mode != "jason" {
        return Json(json!({
            "success": false,
            "error": "invalid_mode",
            "allowed_modes": ["counselor", "jason"]
        }));
    }

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            eprintln!("Failed to begin offline match start transaction: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let user_id: String = match sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(Some(user_id)) => user_id,
        Ok(None) => return Json(json!({"success": false, "error": "invalid_session"})),
        Err(error) => {
            eprintln!("Failed to validate offline match start session: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let inserted = match sqlx::query(
        "INSERT OR IGNORE INTO offline_match_starts (user_id, match_id, mode) VALUES (?, ?, ?)",
    )
    .bind(&user_id)
    .bind(match_id)
    .bind(&request.mode)
    .execute(&mut *tx)
    .await
    {
        Ok(result) => result.rows_affected() > 0,
        Err(error) => {
            eprintln!("Failed to record match start: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    if let Err(error) = sqlx::query(
        r#"
        WITH RECURSIVE seq(x) AS (
            SELECT 1
            UNION ALL
            SELECT x + 1 FROM seq WHERE x < 999
        )
        INSERT OR IGNORE INTO klay_achievements (user_id, achievement_id)
        SELECT ?, 'offline_trophy_' || printf('%03d', x)
        FROM seq
        "#,
    )
    .bind(&user_id)
    .execute(&mut *tx)
    .await
    {
        eprintln!("Failed to save KLAY achievements: {error}");
        return Json(json!({"success": false, "error": "achievement_save_failed"}));
    }

    let unlocked: i64 = match sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM klay_achievements
        WHERE user_id = ? AND achievement_id LIKE 'offline_trophy_%'
        "#,
    )
    .bind(&user_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(unlocked) => unlocked,
        Err(error) => {
            eprintln!("Failed to count KLAY achievements: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    if let Err(error) = tx.commit().await {
        eprintln!("Failed to commit offline match start: {error}");
        return Json(json!({"success": false, "error": "database_error"}));
    }

    Json(json!({
        "success": true,
        "event": "offline_match_started",
        "recorded": inserted,
        "duplicate": !inserted,
        "match_id": match_id,
        "mode": request.mode,
        "custom_achievements_unlocked": unlocked,
        "custom_achievements_total": 999,
        "note": "Custom KLAY backend records only, not official PSN trophies"
    }))
}
async fn record_offline_match_handler(
    State(state): State<AppState>,
    Json(request): Json<OfflineMatchRequest>,
) -> Json<Value> {
    let token = request.token.trim();
    let match_id = request.match_id.trim();

    if token.is_empty() || match_id.is_empty() {
        return Json(json!({"success": false, "error": "missing_token_or_match_id"}));
    }
    if match_id.len() > 128 {
        return Json(json!({"success": false, "error": "match_id_too_long"}));
    }
    if !request.completed {
        return Json(json!({"success": false, "error": "match_not_completed"}));
    }
    if request.mode != "counselor" && request.mode != "jason" {
        return Json(json!({
            "success": false,
            "error": "invalid_mode",
            "allowed_modes": ["counselor", "jason"]
        }));
    }

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            eprintln!("Failed to begin offline match transaction: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let user_id: String = match sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(Some(user_id)) => user_id,
        Ok(None) => return Json(json!({"success": false, "error": "invalid_session"})),
        Err(error) => {
            eprintln!("Failed to validate offline match session: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let inserted = match sqlx::query(
        "INSERT OR IGNORE INTO offline_matches (user_id, match_id, mode) VALUES (?, ?, ?)",
    )
    .bind(&user_id)
    .bind(match_id)
    .bind(&request.mode)
    .execute(&mut *tx)
    .await
    {
        Ok(result) => result.rows_affected() > 0,
        Err(error) => {
            eprintln!("Failed to record offline match: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let achievement_id = match request.mode.as_str() {
        "counselor" => "offline_counselor_first_match",
        "jason" => "offline_jason_first_match",
        _ => unreachable!(),
    };

    if inserted
        && let Err(error) = sqlx::query(
            "INSERT OR IGNORE INTO klay_achievements (user_id, achievement_id) VALUES (?, ?)",
        )
        .bind(&user_id)
        .bind(achievement_id)
        .execute(&mut *tx)
        .await
    {
        eprintln!("Failed to record KLAY achievement: {error}");
        return Json(json!({"success": false, "error": "achievement_save_failed"}));
    }

    let (counselor_matches, jason_matches): (i64, i64) = match sqlx::query_as(
        r#"
        SELECT
            COALESCE(SUM(CASE WHEN mode = 'counselor' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN mode = 'jason' THEN 1 ELSE 0 END), 0)
        FROM offline_matches
        WHERE user_id = ?
        "#,
    )
    .bind(&user_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(counts) => counts,
        Err(error) => {
            eprintln!("Failed to load offline match progress: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    if let Err(error) = tx.commit().await {
        eprintln!("Failed to commit offline match: {error}");
        return Json(json!({"success": false, "error": "database_error"}));
    }

    Json(json!({
        "success": true,
        "recorded": inserted,
        "duplicate": !inserted,
        "mode": request.mode,
        "match_id": match_id,
        "achievement": achievement_id,
        "progress": {
            "offline_counselor_matches": counselor_matches,
            "offline_jason_matches": jason_matches,
            "total_offline_matches": counselor_matches + jason_matches
        },
        "note": "Backend tracking only; not official PSN trophies"
    }))
}
async fn trophy_progress_handler(
    State(state): State<AppState>,
    Json(request): Json<AuthRequest>,
) -> Json<Value> {
    let token = request.token.trim();
    if token.is_empty() {
        return Json(json!({"success": false, "error": "missing_token"}));
    }

    let user_id: String = match sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(user_id)) => user_id,
        Ok(None) => return Json(json!({"success": false, "error": "invalid_session"})),
        Err(error) => {
            eprintln!("Failed to validate trophy progress session: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let (counselor_matches, jason_matches): (i64, i64) = match sqlx::query_as(
        r#"
        SELECT
            COALESCE(SUM(CASE WHEN mode = 'counselor' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN mode = 'jason' THEN 1 ELSE 0 END), 0)
        FROM offline_matches
        WHERE user_id = ?
        "#,
    )
    .bind(&user_id)
    .fetch_one(&state.db)
    .await
    {
        Ok(counts) => counts,
        Err(error) => {
            eprintln!("Failed to load trophy match progress: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let achievements: Vec<(String, String)> = match sqlx::query_as(
        r#"
        SELECT achievement_id, unlocked_at
        FROM klay_achievements
        WHERE user_id = ?
        ORDER BY unlocked_at ASC, achievement_id ASC
        "#,
    )
    .bind(&user_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(achievements) => achievements,
        Err(error) => {
            eprintln!("Failed to load KLAY achievements: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let achievement_list: Vec<Value> = achievements
        .into_iter()
        .map(|(id, unlocked_at)| json!({"id": id, "unlocked_at": unlocked_at}))
        .collect();

    Json(json!({
        "success": true,
        "user_id": user_id,
        "progress": {
            "offline_counselor_matches": counselor_matches,
            "offline_jason_matches": jason_matches,
            "total_offline_matches": counselor_matches + jason_matches
        },
        "achievements": achievement_list,
        "note": "Backend tracking only; not official PSN trophies"
    }))
}
async fn create_lobby_handler(
    State(state): State<AppState>,
    Json(request): Json<LobbyCreateRequest>,
) -> Json<Value> {
    let token = request.token.trim();
    if token.is_empty() {
        return Json(json!({"success": false, "error": "missing_token"}));
    }

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            eprintln!("Failed to begin lobby creation: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let session = sqlx::query_as::<_, (String, String)>(
        "SELECT user_id, display_name FROM sessions WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(&mut *tx)
    .await;

    let (user_id, display_name) = match session {
        Ok(Some(session)) => session,
        Ok(None) => return Json(json!({"success": false, "error": "invalid_session"})),
        Err(error) => {
            eprintln!("Failed to validate lobby creation session: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let name = request.name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{display_name}'s Lobby"));

    if name.chars().count() > 48 {
        return Json(json!({"success": false, "error": "lobby_name_too_long"}));
    }

    let max_players = request.max_players.unwrap_or(DEFAULT_MAX_PLAYERS)
        .clamp(1, DEFAULT_MAX_PLAYERS);
    let lobby_id = Uuid::new_v4().to_string();

    if let Err(error) = sqlx::query(
        "INSERT INTO lobbies (lobby_id, name, host_user_id, max_players) VALUES (?, ?, ?, ?)",
    )
    .bind(&lobby_id)
    .bind(&name)
    .bind(&user_id)
    .bind(max_players)
    .execute(&mut *tx)
    .await
    {
        eprintln!("Failed to create lobby: {error}");
        return Json(json!({"success": false, "error": "lobby_create_failed"}));
    }

    if let Err(error) = sqlx::query(
        "INSERT INTO lobby_players (lobby_id, user_id, ready) VALUES (?, ?, 0)",
    )
    .bind(&lobby_id)
    .bind(&user_id)
    .execute(&mut *tx)
    .await
    {
        eprintln!("Failed to add lobby host: {error}");
        return Json(json!({"success": false, "error": "lobby_host_join_failed"}));
    }

    if let Err(error) = tx.commit().await {
        eprintln!("Failed to commit lobby creation: {error}");
        return Json(json!({"success": false, "error": "database_error"}));
    }

    println!("Lobby created: {} host={}", lobby_id, user_id);

    Json(json!({
        "success": true,
        "lobbyId": lobby_id,
        "name": name,
        "hostUserId": user_id,
        "maxPlayers": max_players,
        "players": 1,
        "status": "waiting"
    }))
}
async fn list_lobbies_handler(State(state): State<AppState>) -> Json<Value> {
    let lobbies = match sqlx::query_as::<_, (String, String, String, i32, i64, i64)>(
        r#"
        SELECT
            l.lobby_id,
            l.name,
            l.host_user_id,
            l.max_players,
            l.started,
            COUNT(lp.user_id) AS player_count
        FROM lobbies l
        LEFT JOIN lobby_players lp ON lp.lobby_id = l.lobby_id
        GROUP BY l.lobby_id, l.name, l.host_user_id, l.max_players, l.started, l.created_at
        ORDER BY l.created_at DESC
        "#,
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(lobbies) => lobbies,
        Err(error) => {
            eprintln!("Failed to list lobbies: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let result: Vec<Value> = lobbies
        .into_iter()
        .map(|(lobby_id, name, host_user_id, max_players, started, players)| {
            json!({
                "lobbyId": lobby_id,
                "name": name,
                "hostUserId": host_user_id,
                "maxPlayers": max_players,
                "players": players,
                "started": started != 0,
                "status": if started != 0 { "started" } else if players >= max_players as i64 { "full" } else { "waiting" }
            })
        })
        .collect();

    Json(json!({
        "success": true,
        "lobbies": result
    }))
}
async fn get_lobby_handler(
    State(state): State<AppState>,
    Path(lobby_id): Path<String>,
) -> Json<Value> {
    let lobby = match sqlx::query_as::<_, (String, String, String, i32, i64)>(
        r#"
        SELECT lobby_id, name, host_user_id, max_players, started
        FROM lobbies
        WHERE lobby_id = ?
        "#,
    )
    .bind(&lobby_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(lobby)) => lobby,
        Ok(None) => return Json(json!({"success": false, "error": "lobby_not_found"})),
        Err(error) => {
            eprintln!("Failed to load lobby details: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let (lobby_id, name, host_user_id, max_players, started) = lobby;
    let players = match sqlx::query_as::<_, (String, String, String, i64)>(
        r#"
        SELECT s.user_id, s.display_name, lp.joined_at, lp.ready
        FROM lobby_players lp
        INNER JOIN sessions s ON s.user_id = lp.user_id
        WHERE lp.lobby_id = ?
        ORDER BY lp.joined_at ASC, lp.user_id ASC
        "#,
    )
    .bind(&lobby_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(players) => players,
        Err(error) => {
            eprintln!("Failed to load lobby members: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let player_list: Vec<Value> = players
        .iter()
        .map(|(user_id, display_name, joined_at, ready)| {
            json!({
                "userId": user_id,
                "displayName": display_name,
                "joinedAt": joined_at,
                "ready": *ready != 0
            })
        })
        .collect();

    let player_count = player_list.len() as i64;
    Json(json!({
        "success": true,
        "lobby": {
            "lobbyId": lobby_id,
            "name": name,
            "hostUserId": host_user_id,
            "maxPlayers": max_players,
            "players": player_count,
            "status": if started != 0 { "started" } else if player_count >= max_players as i64 { "full" } else { "waiting" },
            "started": started != 0,
            "members": player_list
        }
    }))
}
async fn join_lobby_handler(
    State(state): State<AppState>,
    Path(lobby_id): Path<String>,
    Json(request): Json<LobbyJoinRequest>,
) -> Json<Value> {
    let token = request.token.trim();
    if token.is_empty() {
        return Json(json!({"success": false, "error": "missing_token"}));
    }

    let user_id: String = match sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(user_id)) => user_id,
        Ok(None) => return Json(json!({"success": false, "error": "invalid_session"})),
        Err(error) => {
            eprintln!("Failed to validate lobby join session: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            eprintln!("Failed to begin lobby join: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    let lobby = sqlx::query_as::<_, (String, String, i32, i64)>(
        "SELECT name, host_user_id, max_players, started FROM lobbies WHERE lobby_id = ?",
    )
    .bind(&lobby_id)
    .fetch_optional(&mut *tx)
    .await;

    let (name, host_user_id, max_players, started) = match lobby {
        Ok(Some(lobby)) => lobby,
        Ok(None) => return Json(json!({"success": false, "error": "lobby_not_found"})),
        Err(error) => {
            eprintln!("Failed to load lobby for join: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    if started != 0 {
        return Json(json!({"success": false, "error": "lobby_started"}));
    }

    let inserted = match sqlx::query(
        "INSERT OR IGNORE INTO lobby_players (lobby_id, user_id, ready) VALUES (?, ?, 0)",
    )
    .bind(&lobby_id)
    .bind(&user_id)
    .execute(&mut *tx)
    .await
    {
        Ok(result) => result.rows_affected() > 0,
        Err(error) => {
            let message = error.to_string();
            eprintln!("Failed to join lobby {lobby_id}: {error}");
            if message.contains("lobby_full") {
                return Json(json!({
                    "success": false,
                    "error": "lobby_full",
                    "maxPlayers": max_players
                }));
            }
            return Json(json!({"success": false, "error": "join_failed"}));
        }
    };

    let players: i64 = match sqlx::query_scalar(
        "SELECT COUNT(*) FROM lobby_players WHERE lobby_id = ?",
    )
    .bind(&lobby_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(count) => count,
        Err(error) => {
            eprintln!("Failed to count lobby players: {error}");
            return Json(json!({"success": false, "error": "database_error"}));
        }
    };

    if let Err(error) = tx.commit().await {
        eprintln!("Failed to commit lobby join: {error}");
        return Json(json!({"success": false, "error": "database_error"}));
    }

    println!("Lobby join: lobby={} user={}", lobby_id, user_id);

    Json(json!({
        "success": true,
        "alreadyJoined": !inserted,
        "lobbyId": lobby_id,
        "name": name,
        "hostUserId": host_user_id,
        "players": players,
        "maxPlayers": max_players,
        "status": if players >= max_players as i64 { "full" } else { "waiting" }
    }))
}
async fn leave_lobby_handler(
    State(state): State<AppState>,
    Path(lobby_id): Path<String>,
    Json(request): Json<LobbyJoinRequest>,
) -> Json<Value> {
    let user_id: String = match sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE token = ?",
    )
    .bind(request.token.trim())
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(user_id)) => user_id,
        Ok(None) => {
            return Json(json!({
                "success": false,
                "error": "invalid_session"
            }));
        }
        Err(error) => {
            eprintln!("Failed to validate lobby leave session: {error}");
            return Json(json!({
                "success": false,
                "error": "database_error"
            }));
        }
    };

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            eprintln!("Failed to begin lobby leave transaction: {error}");
            return Json(json!({
                "success": false,
                "error": "database_error"
            }));
        }
    };

    let host_user_id: Option<String> = match sqlx::query_scalar(
        "SELECT host_user_id FROM lobbies WHERE lobby_id = ?",
    )
    .bind(&lobby_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(host_user_id) => host_user_id,
        Err(error) => {
            eprintln!("Failed to load lobby before leave: {error}");
            return Json(json!({
                "success": false,
                "error": "database_error"
            }));
        }
    };

    let Some(host_user_id) = host_user_id else {
        return Json(json!({
            "success": false,
            "error": "lobby_not_found"
        }));
    };

    let removed = match sqlx::query(
        "DELETE FROM lobby_players WHERE lobby_id = ? AND user_id = ?",
    )
    .bind(&lobby_id)
    .bind(&user_id)
    .execute(&mut *tx)
    .await
    {
        Ok(result) => result.rows_affected() > 0,
        Err(error) => {
            eprintln!("Failed to remove lobby member: {error}");
            return Json(json!({
                "success": false,
                "error": "leave_failed"
            }));
        }
    };

    let remaining: i64 = match sqlx::query_scalar(
        "SELECT COUNT(*) FROM lobby_players WHERE lobby_id = ?",
    )
    .bind(&lobby_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(count) => count,
        Err(error) => {
            eprintln!("Failed to count remaining lobby members: {error}");
            return Json(json!({
                "success": false,
                "error": "database_error"
            }));
        }
    };

    let new_host = if remaining == 0 {
        if let Err(error) = sqlx::query("DELETE FROM lobbies WHERE lobby_id = ?")
            .bind(&lobby_id)
            .execute(&mut *tx)
            .await
        {
            eprintln!("Failed to delete empty lobby: {error}");
            return Json(json!({
                "success": false,
                "error": "lobby_cleanup_failed"
            }));
        }
        None
    } else if host_user_id == user_id {
        let next_host: Option<String> = match sqlx::query_scalar(
            r#"
            SELECT user_id
            FROM lobby_players
            WHERE lobby_id = ?
            ORDER BY joined_at ASC, user_id ASC
            LIMIT 1
            "#,
        )
        .bind(&lobby_id)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(next_host) => next_host,
            Err(error) => {
                eprintln!("Failed to select replacement lobby host: {error}");
                return Json(json!({
                    "success": false,
                    "error": "host_transfer_failed"
                }));
            }
        };

        if let Some(next_host) = next_host.as_deref()
            && let Err(error) = sqlx::query(
                "UPDATE lobbies SET host_user_id = ? WHERE lobby_id = ?",
            )
            .bind(next_host)
            .bind(&lobby_id)
            .execute(&mut *tx)
            .await
        {
            eprintln!("Failed to transfer lobby host: {error}");
            return Json(json!({
                "success": false,
                "error": "host_transfer_failed"
            }));
        }
        next_host
    } else {
        Some(host_user_id)
    };

    if let Err(error) = tx.commit().await {
        eprintln!("Failed to commit lobby leave transaction: {error}");
        return Json(json!({
            "success": false,
            "error": "leave_commit_failed"
        }));
    }

    Json(json!({
        "success": true,
        "removed": removed,
        "lobbyId": lobby_id,
        "players": remaining,
        "hostUserId": new_host
    }))
}

async fn db_check_handler(State(state): State<AppState>) -> Json<Value> {
    let connected = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();

    Json(json!({
        "status": if connected { "online" } else { "degraded" },
        "database": if connected { "connected" } else { "disconnected" },
        "healthy": connected
    }))
}

async fn server_info_handler(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "name": "KLAY Friday the 13th Private Server",
        "status": "online",
        "backend": "rust-axum",
        "database": "sqlite",
        "public_url": state.public_url,
        "version": SERVER_VERSION,
        "features": [
            "sessions",
            "heartbeat",
            "session_validation",
            "lobbies",
            "lobby_details",
            "lobby_join",
            "lobby_leave",
            "protocol",
            "custom_tcp_protocol",
            "ready_state",
            "session_start",
            "offline_match_tracking",
            "klay_achievements"
        ]
    }))
}


#[cfg(test)]
mod lobby_capacity_tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_trigger_rejects_joining_a_full_lobby() {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            "CREATE TABLE lobbies (lobby_id TEXT PRIMARY KEY, max_players INTEGER NOT NULL)",
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE lobby_players (lobby_id TEXT NOT NULL, user_id TEXT NOT NULL, PRIMARY KEY (lobby_id, user_id))",
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TRIGGER enforce_lobby_capacity
            BEFORE INSERT ON lobby_players
            WHEN NOT EXISTS (
                SELECT 1 FROM lobby_players
                WHERE lobby_id = NEW.lobby_id AND user_id = NEW.user_id
            )
            AND (
                SELECT COUNT(*) FROM lobby_players WHERE lobby_id = NEW.lobby_id
            ) >= COALESCE(
                (SELECT max_players FROM lobbies WHERE lobby_id = NEW.lobby_id),
                0
            )
            BEGIN
                SELECT RAISE(ABORT, 'lobby_full');
            END
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query("INSERT INTO lobbies (lobby_id, max_players) VALUES ('lobby-a', 1)")
            .execute(&db)
            .await
            .unwrap();

        sqlx::query("INSERT INTO lobby_players (lobby_id, user_id) VALUES ('lobby-a', 'player-1')")
            .execute(&db)
            .await
            .unwrap();

        let rejected = sqlx::query(
            "INSERT INTO lobby_players (lobby_id, user_id) VALUES ('lobby-a', 'player-2')",
        )
        .execute(&db)
        .await;

        assert!(rejected.is_err(), "a full lobby must reject additional players");
    }
}
