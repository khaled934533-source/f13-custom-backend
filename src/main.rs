use axum::{
    routing::{get, post},
    http::StatusCode,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::env;

#[tokio::main]
async fn main() {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT").unwrap_or_else(|_| "8080".to_string());

    println!("Starting Friday the 13th Private Server...");

    let app = Router::new()
        .route("/", get(home_handler))
        .route("/api/v1/login", post(login_handler))
        .route("/api/v1/auth/psn", post(login_handler))
        .route("/api/v1/profiles/me", get(profile_handler))
        .route("/api/v1/database/status", get(db_check_handler))
        .route("/api/v1/database_check", get(db_check_handler));

    let addr_str = format!("{}:{}", host, port);
    let addr: SocketAddr = addr_str.parse().expect("Invalid HOST or PORT configuration");
    
    println!("Server is running and listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn home_handler() -> &'static str {
    "Welcome to KLAY Private Server Backend! Discord: https://discord.gg"
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
        "unlocked_assets": ["savini_jason", "backers_clothing_pack", "counselor_clothing_dlc"],
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
