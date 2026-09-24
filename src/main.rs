use axum::{
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{env, net::SocketAddr};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let host = env::var("HOST")
        .unwrap_or_else(|_| "0.0.0.0".to_string());

    let port = env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string());

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("Invalid HOST or PORT configuration");

    let app = Router::new()
        .route("/", get(home_handler))
        .route("/health", get(health_handler))
        .route("/api/v1/login", post(login_handler))
        .route("/api/v1/auth/psn", post(login_handler))
        .route("/api/v1/profiles/me", get(profile_handler))
        .route("/api/v1/database/status", get(db_check_handler))
        .route("/api/v1/database_check", get(db_check_handler));

    println!("KLAY Friday the 13th Private Server Backend");
    println!("Discord: https://discord.gg/SYaM9whT");
    println!("Listening on http://{addr}");

    let listener = TcpListener::bind(addr)
        .await
        .expect("Failed to bind server");

    axum::serve(listener, app)
        .await
        .expect("Server failed");
}

async fn home_handler() -> &'static str {
    "Welcome to KLAY Private Server Backend! Discord: https://discord.gg/SYaM9whT"
}

async fn health_handler() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "f13-custom-backend",
        "discord": "https://discord.gg/SYaM9whT"
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

async fn db_check_handler() -> Json<Value> {
    Json(json!({
        "status": "online",
        "database": "connected",
        "healthy": true
    }))
}
