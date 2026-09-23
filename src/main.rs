use axum::{
    routing::{get, post},
    http::StatusCode,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::env;

#[tokio::main]
async fn main() {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT").unwrap_or_else(|_| "8080".to_string());

    println!("Starting Friday the 13th Private Server...");

    let app = Router::new()
        .route("/", get(home_handler))
        .route("/api/login", post(login_handler))
        .route("/api/database_check", get(db_check_handler));

    let addr_str = format!("{}:{}", host, port);
    let addr: SocketAddr = addr_str.parse().expect("Invalid HOST or PORT configuration");
    
    println!("Server is running and listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn home_handler() -> &'static str {
    "Welcome to KLAY Private Server Backend! Discord: https://discord.gg/SYaM9whT"
}

#[derive(Serialize, Deserialize)]
struct LoginResponse {
    success: bool,
    token: String,
    player_level: u32,
}

async fn login_handler() -> Json<LoginResponse> {
    Json(LoginResponse {
        success: true,
        token: "f13_secure_session_token_xyz".to_string(),
        player_level: 150, // All features unlocked automatically
    })
}

async fn db_check_handler() -> StatusCode {
    StatusCode::OK
}
