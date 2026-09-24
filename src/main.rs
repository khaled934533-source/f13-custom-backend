use axum::{
    extract::State,
    http::HeaderValue,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::{env, net::SocketAddr};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
struct AppState {
    db: Option<SqlitePool>,
    public_url: String,
}

#[tokio::main]
async fn main() {
    let host = env::var("HOST")
        .unwrap_or_else(|_| "0.0.0.0".to_string());

    let port = env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string());

    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://f13.db".to_string());

    let public_url = env::var("PUBLIC_URL")
        .unwrap_or_else(|_| {
            "https://f13-custom-backend-production.up.railway.app".to_string()
        });

    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .expect("Invalid HOST or PORT configuration");

    let db = match SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
    {
        Ok(pool) => {
            println!("Database connected: {}", database_url);
            Some(pool)
        }
        Err(error) => {
            eprintln!("Database connection failed: {error}");
            None
        }
    };

    let state = AppState {
        db,
        public_url,
    };

    let cors = CorsLayer::new()
        .allow_origin(
            state
                .public_url
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
        .layer(cors)
        .with_state(state);

    let listener = TcpListener::bind(addr)
        .await
        .expect("Failed to bind server");

    println!("========================================");
    println!("KLAY Friday the 13th Private Server");
    println!("========================================");
    println!("Host: {}", host);
    println!("Port: {}", port);
    println!("Server URL: {}", public_url);
    println!("========================================");
    println!("Server is listening on {}", addr);

    axum::serve(listener, app)
        .await
        .expect("Server failed");
}

async fn home_handler() -> &'static str {
    "Welcome to KLAY Private Server Backend!"
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

async fn login_handler() -> Json<Value> {
    Json(json!({
        "success": true,
        "token": "f13_secure_session_token_xyz_completed",
        "userId": "1337",
        "displayName": "KLAY_Player",
        "status": "success"
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
