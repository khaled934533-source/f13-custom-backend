```rust
use axum::{
    extract::State,
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

#[derive(Clone)]
struct AppState {
    db: Option<SqlitePool>,
    public_url: String,
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    display_name: Option<String>,
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
        .unwrap_or_else(|_| "https://f13-custom-backend-production.up.railway.app".to_string());

    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:///tmp/f13.db".to_string());

    println!("========================================");
    println!("KLAY Friday the 13th Private Server");
    println!("========================================");
    println!("Host: {host}");
    println!("Port: {port}");
    println!("Server URL: {public_url}");
    println!("Database: {database_url}");
    println!("========================================");

    let db = match SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
    {
        Ok(pool) => {
            println!("Database connected");

            if let Err(error) = sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS sessions (
                    user_id TEXT PRIMARY KEY,
                    display_name TEXT NOT NULL,
                    token TEXT NOT NULL,
                    created_at TEXT DEFAULT CURRENT_TIMESTAMP
                )
                "#,
            )
            .execute(&pool)
            .await
            {
                eprintln!("Failed to create sessions table: {error}");
            }

            Some(pool)
        }

        Err(error) => {
            eprintln!("Database connection failed: {error}");
            None
        }
    };

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
        .route("/api/v1/login", post(login_handler))
        .route("/api/v1/auth/psn", post(login_handler))
        .route("/api/v1/profiles/me", get(profile_handler))
        .route("/api/v1/database/status", get(db_check_handler))
        .route("/api/v1/database_check", get(db_check_handler))
        .route("/api/v1/server/info", get(server_info_handler))
        .with_state(state)
        .layer(cors);

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("Invalid HOST or PORT");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind");

    println!("Server is listening on {addr}");

    axum::serve(listener, app)
        .await
        .expect("Server failed");
}

async fn home_handler() -> &'static str {
    "KLAY Friday the 13th Private Server"
}

async fn health_handler(
    State(state): State<AppState>,
) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "f13-custom-backend",
        "public_url": state.public_url
    }))
}

async fn login_handler(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Json<Value> {
    let display_name = request
        .display_name
        .unwrap_or_else(|| "KLAY_Player".to_string());

    let user_id = Uuid::new_v4().to_string();
    let token = Uuid::new_v4().to_string();

    if let Some(db) = &state.db {
        let _ = sqlx::query(
            r#"
            INSERT INTO sessions (user_id, display_name, token)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(&user_id)
        .bind(&display_name)
        .bind(&token)
        .execute(db)
        .await;
    }

    Json(json!(LoginResponse {
        success: true,
        token,
        user_id,
        display_name,
        status: "success".to_string(),
    }))
}

async fn profile_handler() -> Json<Value> {
    Json(json!({
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

async fn db_check_handler(
    State(state): State<AppState>,
) -> Json<Value> {
    let connected = match &state.db {
        Some(pool) => {
            sqlx::query("SELECT 1")
                .execute(pool)
                .await
                .is_ok()
        }
        None => false,
    };

    Json(json!({
        "status": if connected { "online" } else { "degraded" },
        "database": if connected { "connected" } else { "disconnected" },
        "healthy": connected
    }))
}

async fn server_info_handler(
    State(state): State<AppState>,
) -> Json<Value> {
    Json(json!({
        "name": "KLAY Friday the 13th Private Server",
        "status": "online",
        "backend": "rust-axum",
        "database": "sqlite",
        "public_url": state.public_url,
        "version": "0.2.0"
    }))
}
```
