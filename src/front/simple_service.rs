use std::{path::Path, time::Duration};

use http_body_util::Full;
use hyper::{body::Bytes, server::conn::http1, service::service_fn, Method, Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tracing::{event, instrument, Level};
use uuid::Uuid;
use crate::{conveyer::ConveyQueue, dtos::{self, Behavior, Package}, shutdown::Shutdown};

fn get_mime_type(filename: &str) -> &'static str {
    match Path::new(filename).extension().and_then(|e| e.to_str()) {
        Some("jpeg" | "jpg") => "image/jpeg",
        Some("png") => "image/png",
        Some("mp4") => "video/mp4",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("json") => "application/json",
        Some("html") => "text/html",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("xml") => "application/xml",
        _ => "application/octet-stream",
    }
}

#[instrument(skip_all)]
async fn handle_http(req: Request<hyper::body::Incoming>) -> Result<Response<Full<Bytes>>, hyper::http::Error> { 
     // Only handle GET requests
    if req.method() != &Method::GET {
        return Ok(Response::builder()
            .status(hyper::StatusCode::METHOD_NOT_ALLOWED)
            .body(Full::new(Bytes::from("Method Not Allowed")))?
        );
    }

    let log_id = Uuid::new_v4().to_string();
    event!(Level::INFO, "[waitress {}] Handling connection", &log_id);

    let uri = req.uri().to_string();
    let path_vec: Vec<&str> = uri.strip_prefix("/").unwrap_or(&uri).split('/').collect();
    if path_vec.len() != 1 {
        event!(Level::ERROR, "Invalid URL: {}", uri);
        return Ok(Response::builder()
            .status(hyper::StatusCode::BAD_REQUEST)
            .body(Full::new(Bytes::from("Invalid URL")))?
        );
    }

    // Create package for the queue
    let uuid = Uuid::new_v4();
    let uni_id = uuid.into_bytes();
    let mut package = Package::new_with_id(&uuid);
    package.behavior = Behavior::GetFile;

    let name_bytes = path_vec[0].as_bytes();
    if name_bytes.len() > dtos::NAME_SIZE {
        return Ok(Response::builder()
                    .status(hyper::StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from("File name too long: max 256 bytes")))?
                );
    }
    let mut name_buf = [0u8; dtos::NAME_SIZE];
    name_buf[..name_bytes.len()].copy_from_slice(name_bytes);
    package.content.name = name_buf;
    package.behavior = Behavior::GetFile;

    // Send to queue
    if let Err(e) = ConveyQueue::get_instance().produce_order(package) {
        event!(Level::ERROR, "Failed to produce order: {}", e);
        return Ok(Response::builder()
            .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
            .body(Full::new(Bytes::from("Failed to process request")))?
        );
    }

    // Time control
    let start_time = tokio::time::Instant::now();
    let overall_timeout = Duration::from_secs(10);

    // Wait for package from conveyer
    let con_queue = ConveyQueue::get_instance();
    loop {
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Check overall timeout
        if tokio::time::Instant::now() > start_time + overall_timeout {
            event!(tracing::Level::ERROR, "[waitress {}] Overall timeout exceeded", &log_id);
            return Ok(Response::builder()
                    .status(hyper::StatusCode::REQUEST_TIMEOUT)
                    .body(Full::new(Bytes::from("Overall timeout exceeded")))?
                )
        }

        let con_queue_clone = con_queue.clone();
        let uni_id_value = uni_id;

        match con_queue_clone.consume_service(uni_id_value) {
            Ok(Some(pkg)) => {
                let valid_data_end = pkg.content.name.iter()
                    .position(|&b| b == 0)
                    .unwrap_or(pkg.content.name.len());

                let content_type = get_mime_type(
                    &String::from_utf8_lossy(&pkg.content.name[..valid_data_end]).to_string()
                );
                return Ok(Response::builder()
                    .status(hyper::StatusCode::OK)
                    .header("X-Content-Type-Options", "nosniff")
                    .header("X-Frame-Options", "DENY")
                    .header("Content-Type", content_type)
                    .header("Content-Length", pkg.content.data.len().to_string())
                    .body(Full::new(Bytes::from(pkg.content.data)))?
                )
            },
            Ok(None) => {},
            Err(err) => {
                event!(tracing::Level::ERROR, "[waitress {}] {}", &log_id, err);
            }
        }
    }
}

#[instrument(skip_all)]
pub async fn run_http_server(addr: &str) {
    event!(Level::INFO ,"Self service starting");

    let listener = match TcpListener::bind(addr).await{
        Ok(listener) => listener,
        Err(_) => {
            event!(Level::ERROR, "Failed to bind to address {}", addr);
            panic!("Failed to bind to address");
        }
    };

    let shutdown_status = Shutdown::get_instance();

    loop {
        if shutdown_status.is_shutdown() {
            break;
        }

        let (stream, _ ) = match listener.accept().await {
            Ok(req) => req,
            Err(_) => {
                event!(Level::ERROR, "Failed to accept connection");
                continue;
            }
        };

        let io = TokioIo::new(stream);

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service_fn(handle_http))
                .await
            {
                event!(Level::ERROR, "Error serving connection: {:?}", err);
            }
        });
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_string_to_static_bytes_array() {
        let mut buf = [0u8; 256];
        let s = "Hello, world!";
        buf[..s.len()].copy_from_slice(s.as_bytes());
        let valid_data_end = buf.iter()
            .position(|&b| b == 0)
            .unwrap_or(buf.len());
        assert_eq!(String::from_utf8_lossy(&buf[..valid_data_end]), "Hello, world!".to_string());
    }

    #[test]
    fn test_url_slice() {
        let url_raw = "/path";
        let path: Vec<&str> = url_raw.strip_prefix("/").unwrap_or(url_raw).split('/').collect();
        println!("path_slice{:?}", path);
    }
}