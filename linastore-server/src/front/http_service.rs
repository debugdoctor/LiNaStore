use std::{io, path::Path, time::Duration};

use crate::{
    conveyer::{AdmissionError, ConveyQueue},
    dtos::{Behavior, OrderRequest},
    mapper,
    shutdown::Shutdown,
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::{Method, Request, Response, body::Frame, server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::sync::oneshot;
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
use tracing::{Level, event, instrument};
use uuid::Uuid;

type HttpBody = BoxBody<Bytes, io::Error>;

fn boxed_full(body: Bytes) -> HttpBody {
    Full::new(body).map_err(|never| match never {}).boxed()
}

fn stream_body(rx: tokio::sync::mpsc::Receiver<Bytes>) -> HttpBody {
    let stream = ReceiverStream::new(rx).map(|chunk| Ok::<_, io::Error>(Frame::data(chunk)));
    StreamBody::new(stream).boxed()
}

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

async fn resolve_with_mapper(bucket: &str, key: &str) -> Result<String, Response<HttpBody>> {
    match mapper::get_mapper() {
        Some(m) => match m.resolve(bucket, key).await {
            Ok(Some(internal)) => Ok(internal),
            _ => Err(Response::builder()
                .status(hyper::StatusCode::NOT_FOUND)
                .body(boxed_full(Bytes::from("Not Found")))
                .unwrap()),
        },
        None => Err(Response::builder()
            .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
            .body(boxed_full(Bytes::from("Mapper unavailable")))
            .unwrap()),
    }
}

#[instrument(skip_all)]
async fn handle_http(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<HttpBody>, hyper::http::Error> {
    if req.method() != &Method::GET {
        return Ok(Response::builder()
            .status(hyper::StatusCode::METHOD_NOT_ALLOWED)
            .body(boxed_full(Bytes::from("Method Not Allowed")))?);
    }

    let uri = req.uri().to_string();
    let path = uri.strip_prefix('/').unwrap_or(&uri);
    if path.is_empty() {
        return Ok(Response::builder()
            .status(hyper::StatusCode::OK)
            .body(boxed_full(Bytes::from("LiNastore is running")))?);
    }

    let path_vec: Vec<&str> = path.split('/').collect();
    let file_identifier: String = if path_vec.len() >= 2 {
        let bucket = path_vec[0];
        let key = path_vec[1..].join("/");
        match resolve_with_mapper(bucket, &key).await {
            Ok(id) => id,
            Err(resp) => return Ok(resp),
        }
    } else if path_vec.len() == 1 {
        let key = path_vec[0];
        match resolve_with_mapper(mapper::DEFAULT_BUCKET, key).await {
            Ok(id) => id,
            Err(resp) => return Ok(resp),
        }
    } else {
        return Ok(Response::builder()
            .status(hyper::StatusCode::BAD_REQUEST)
            .body(boxed_full(Bytes::from("Invalid URL")))?);
    };

    let log_id = Uuid::new_v4().to_string();

    let con_queue = ConveyQueue::get_instance();
    // Hold the in-flight permit for the whole request; it is released when this
    // function returns (success, timeout, or disconnect).
    let _permit = match con_queue.acquire_permit().await {
        Ok(permit) => permit,
        Err(AdmissionError::TimedOut) => {
            event!(Level::WARN, "Admission wait timed out");
            return Ok(Response::builder()
                .status(hyper::StatusCode::SERVICE_UNAVAILABLE)
                .body(boxed_full(Bytes::from("Server busy, retry later")))?);
        }
        Err(_) => {
            return Ok(Response::builder()
                .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
                .body(boxed_full(Bytes::from("Failed to register request")))?);
        }
    };

    // GET has no body: close the payload channel, only meta enters the queue.
    let (reply_tx, reply_rx) = oneshot::channel();
    let (data_tx, order) = OrderRequest::create(
        Behavior::GetFile,
        0,
        Bytes::copy_from_slice(file_identifier.as_bytes()),
        0,
        false,
        reply_tx,
    );
    drop(data_tx);

    if let Err(e) = con_queue.produce_order(order).await {
        event!(Level::ERROR, "Failed to produce order: {}", e);
        return Ok(Response::builder()
            .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
            .body(boxed_full(Bytes::from("Failed to process request")))?);
    }

    let timeout = Duration::from_secs(10);
    match tokio::time::timeout(timeout, reply_rx).await {
        Ok(Ok(res)) => {
            let content_type = get_mime_type(&String::from_utf8_lossy(&res.identifier).to_string());
            Ok(Response::builder()
                .status(hyper::StatusCode::OK)
                .header("X-Content-Type-Options", "nosniff")
                .header("X-Frame-Options", "DENY")
                .header("Content-Type", content_type)
                .header("Content-Length", res.data_len.to_string())
                .body(stream_body(res.data))?)
        }
        Ok(Err(_)) => {
            event!(
                Level::ERROR,
                "[waitress {}] Channel closed unexpectedly",
                &log_id
            );
            Ok(Response::builder()
                .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
                .body(boxed_full(Bytes::from("Channel closed unexpectedly")))?)
        }
        Err(_) => {
            event!(Level::ERROR, "[waitress {}] Timeout exceeded", &log_id);
            Ok(Response::builder()
                .status(hyper::StatusCode::REQUEST_TIMEOUT)
                .body(boxed_full(Bytes::from("Request timeout")))?)
        }
    }
}

#[instrument(skip_all)]
pub async fn run_http_server(addr: &str) {
    event!(Level::INFO, "Self service starting");

    let listener = match super::net::bind_listener(addr) {
        Ok(listener) => listener,
        Err(e) => {
            event!(Level::ERROR, "Failed to bind to address {}: {}", addr, e);
            return;
        }
    };

    let shutdown_status = Shutdown::get_instance();

    loop {
        tokio::select! {
            _ = shutdown_status.wait() => {
                break;
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(req) => req,
                    Err(_) => {
                        event!(Level::ERROR, "Failed to accept connection");
                        continue;
                    }
                };

                super::net::tune_stream(&stream);
                let io = TokioIo::new(stream);

                tokio::task::spawn(async move {
                    if let Err(err) = http1::Builder::new()
                        .timer(TokioTimer::new())
                        .header_read_timeout(super::net::HEADER_READ_TIMEOUT)
                        .serve_connection(io, service_fn(handle_http))
                        .await
                    {
                        event!(Level::ERROR, "Error serving connection: {:?}", err);
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_string_to_static_bytes_array() {
        let mut buf = [0u8; 256];
        let s = "Hello, world!";
        buf[..s.len()].copy_from_slice(s.as_bytes());
        let valid_data_end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        assert_eq!(
            String::from_utf8_lossy(&buf[..valid_data_end]),
            "Hello, world!".to_string()
        );
    }

    #[test]
    fn test_url_slice() {
        let url_raw = "/path";
        let path: Vec<&str> = url_raw
            .strip_prefix("/")
            .unwrap_or(url_raw)
            .split('/')
            .collect();
        println!("path_slice{:?}", path);
    }
}
