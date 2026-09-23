use axum::{
    routing::{get, post},
    http::StatusCode,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::net::SocketAddr;
use std::env;

#[tokio::main]
async fn main() {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://f13.db".to_string());

    println!("Starting Friday the 13th Private Server...");
    println!("Database URL: {}", database_url);

    let db_pool = match SqlitePoolOptions::new()
        .max_connections(5)
        .connect_lazy(&database_url) 
    {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("Failed to connect to SQLite Database: {}", e);
            std::process::exit(1);
        }
    };

    let app = Router::new()
        .route("/", get(home_handler))
        .route("/api/login", post(login_handler))
        .route("/api/database_check", get(db_check_handler))
        .with_state(db_pool);

    let addr_str = format!("{}:{}", host, port);
    let addr: SocketAddr = addr_str.parse().expect("Invalid HOST or PORT configuration");
    
    println!("Server is running and listening on http://{}", addr);

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn home_handler() -> &'static str {
    "Welcome to KLAY Private Server Backend!"
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
        player_level: 150,
    })
}

async fn db_check_handler() -> StatusCode {
    StatusCode::OK
}
