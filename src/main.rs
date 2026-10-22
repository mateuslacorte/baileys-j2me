mod models;
mod formatting;
mod store;
mod session;
mod routes;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::session::DeviceRegistry;
use crate::routes::create_routes;

#[tokio::main]
async fn main() {
    println!("=====================================================");
    println!("   Baileys-J2ME Multi-Device Server & Bridge");
    println!("=====================================================");

    let registry: DeviceRegistry = Arc::new(Mutex::new(HashMap::new()));
    let routes = create_routes(registry);

    let port = std::env::var("PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(8085);
    println!("Starting server on 0.0.0.0:{}...", port);
    warp::serve(routes).run(([0, 0, 0, 0], port)).await;
}
