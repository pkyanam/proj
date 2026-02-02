//! HTTP reverse proxy - routes requests based on Host header

use anyhow::Result;
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

/// Routing table mapping project names to ports
pub type RoutingTable = Arc<RwLock<HashMap<String, u16>>>;

/// Create a new routing table
pub fn new_routing_table() -> RoutingTable {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Start the reverse proxy server
pub async fn start_proxy(port: u16, routing_table: RoutingTable) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await?;

    tracing::info!("Reverse proxy listening on http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let table = routing_table.clone();

        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let table = table.clone();
                async move { handle_request(req, table).await }
            });

            if let Err(e) = http1::Builder::new()
                .preserve_header_case(true)
                .serve_connection(io, service)
                .with_upgrades()
                .await
            {
                tracing::debug!("Connection error: {}", e);
            }
        });
    }
}

/// Handle an incoming HTTP request
async fn handle_request(
    req: Request<Incoming>,
    routing_table: RoutingTable,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    // Extract project name from Host header
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    // Parse project name from host (e.g., "my-app.localhost:8080" -> "my-app")
    let project_name = host
        .split('.')
        .next()
        .unwrap_or("")
        .to_string();

    if project_name.is_empty() || project_name == "localhost" {
        return Ok(not_found_response("No project specified. Use <project>.localhost:8080"));
    }

    // Look up the target port
    let target_port = {
        let table = routing_table.read().await;
        table.get(&project_name).copied()
    };

    let target_port = match target_port {
        Some(port) => port,
        None => {
            return Ok(not_found_response(&format!(
                "Project '{}' not found or has no running process",
                project_name
            )));
        }
    };

    // Forward the request to the target
    match forward_request(req, target_port).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            tracing::error!("Failed to forward request: {}", e);
            Ok(error_response(&format!("Failed to connect to backend: {}", e)))
        }
    }
}

/// Forward a request to the target port
async fn forward_request(
    req: Request<Incoming>,
    target_port: u16,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>> {
    let target_addr = format!("127.0.0.1:{}", target_port);

    // Connect to target
    let stream = TcpStream::connect(&target_addr).await?;
    let io = TokioIo::new(stream);

    // Create HTTP connection
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;

    // Spawn connection handler
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            tracing::debug!("Backend connection error: {}", e);
        }
    });

    // Forward the request
    let resp = sender.send_request(req).await?;

    // Convert the response body
    let (parts, body) = resp.into_parts();
    let body = body.map_err(|e| e).boxed();

    Ok(Response::from_parts(parts, body))
}

/// Create a 404 response
fn not_found_response(message: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = Full::new(Bytes::from(format!("Not Found: {}\n", message)))
        .map_err(|never| match never {})
        .boxed();

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("Content-Type", "text/plain")
        .body(body)
        .unwrap()
}

/// Create a 502 error response
fn error_response(message: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = Full::new(Bytes::from(format!("Bad Gateway: {}\n", message)))
        .map_err(|never| match never {})
        .boxed();

    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header("Content-Type", "text/plain")
        .body(body)
        .unwrap()
}

#[allow(dead_code)]
fn empty_body() -> BoxBody<Bytes, hyper::Error> {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}
