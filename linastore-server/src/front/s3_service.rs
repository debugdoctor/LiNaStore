use std::{io, time::Duration};

use crate::{
    conveyer::{AdmissionError, ConveyQueue},
    dtos::{Behavior, OrderRequest, ResponseStream, PAYLOAD_UNKNOWN_LEN, Status},
    mapper,
    shutdown::Shutdown,
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::{Method, Request, Response, StatusCode, body::Frame, server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::sync::oneshot;
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
use tracing::{Level, event, instrument};
use uuid::Uuid;

const S3_XML_NAMESPACE: &str = "http://s3.amazonaws.com/doc/2006-03-01/";
// Per-frame body read timeout while streaming an upload to the worker.
const BODY_CHUNK_TIMEOUT: Duration = Duration::from_secs(30);

type S3Body = BoxBody<Bytes, io::Error>;

fn boxed_full(body: Bytes) -> S3Body {
    Full::new(body).map_err(|never| match never {}).boxed()
}

fn s3_error_xml(code: &str, message: &str, resource: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Error><Code>{}</Code><Message>{}</Message><Resource>{}</Resource></Error>"#,
        code, message, resource
    )
}

fn list_buckets_xml(buckets: &[String]) -> String {
    let mut inner = String::new();
    for b in buckets {
        inner.push_str(&format!("<Bucket><Name>{}</Name></Bucket>", escape_xml(b)));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ListAllMyBucketsResult xmlns="{}"><Buckets>{}</Buckets></ListAllMyBucketsResult>"#,
        S3_XML_NAMESPACE, inner
    )
}

fn list_objects_xml(files: &[(String, String)], bucket: &str, prefix: &str, max_keys: u32, is_truncated: bool) -> String {
    let mut contents = String::new();
    for (key, _) in files {
        contents.push_str(&format!(
            "<Contents><Key>{}</Key><Size>0</Size><StorageClass>STANDARD</StorageClass></Contents>",
            escape_xml(key)
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="{}"><IsTruncated>{}</IsTruncated><Name>{}</Name><Prefix>{}</Prefix><MaxKeys>{}</MaxKeys><KeyCount>{}</KeyCount>{}</ListBucketResult>"#,
        S3_XML_NAMESPACE,
        if is_truncated { "true" } else { "false" },
        escape_xml(bucket),
        escape_xml(prefix),
        max_keys,
        files.len(),
        contents,
    )
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

fn get_mime_type(filename: &str) -> &'static str {
    match std::path::Path::new(filename).extension().and_then(|e| e.to_str()) {
        Some("jpeg" | "jpg") => "image/jpeg",
        Some("png") => "image/png",
        Some("mp4") => "video/mp4",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("json") => "application/json",
        Some("html") => "text/html",
        _ => "application/octet-stream",
    }
}

fn parse_s3_path(path: &str) -> (Option<&str>, Option<&str>) {
    let path = path.strip_prefix('/').unwrap_or(path);
    if path.is_empty() {
        return (None, None);
    }
    let mut parts = path.splitn(2, '/');
    let bucket = parts.next();
    let key = parts.next();
    (bucket, key)
}

fn build_response(status: StatusCode, body: String, content_type: &str) -> Response<S3Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Content-Length", body.len().to_string())
        .body(boxed_full(Bytes::from(body)))
        .unwrap()
}

fn build_empty_response(status: StatusCode) -> Response<S3Body> {
    Response::builder()
        .status(status)
        .body(boxed_full(Bytes::new()))
        .unwrap()
}

/// Build a streaming body that drains an `mpsc::Receiver<Bytes>`.
fn stream_body(rx: tokio::sync::mpsc::Receiver<Bytes>) -> S3Body {
    let stream = ReceiverStream::new(rx).map(|chunk| Ok::<_, io::Error>(Frame::data(chunk)));
    StreamBody::new(stream).boxed()
}

/// Enqueue a body-less order (GET/HEAD/DELETE) and wait for the response.
async fn process_through_queue(behavior: Behavior, identifier: &str) -> Result<ResponseStream, Status> {
    let con_queue = ConveyQueue::get_instance();
    // Hold the in-flight permit for the whole request; released on return.
    let _permit = match con_queue.acquire_permit().await {
        Ok(permit) => permit,
        Err(AdmissionError::TimedOut) => return Err(Status::Overloaded),
        Err(_) => return Err(Status::InternalError),
    };

    let (reply_tx, reply_rx) = oneshot::channel();
    let (data_tx, order) = OrderRequest::create(
        behavior,
        0,
        Bytes::copy_from_slice(identifier.as_bytes()),
        0,
        false,
        reply_tx,
    );
    drop(data_tx);

    if let Err(e) = con_queue.produce_order(order).await {
        event!(Level::ERROR, "Failed to produce order: {}", e);
        return Err(Status::InternalError);
    }

    match tokio::time::timeout(Duration::from_secs(10), reply_rx).await {
        Ok(Ok(res)) => {
            if res.status == Status::Success {
                Ok(res)
            } else {
                Err(res.status)
            }
        }
        Ok(Err(_)) => Err(Status::InternalError),
        Err(_) => {
            event!(Level::ERROR, "S3 request timeout");
            Err(Status::InternalError)
        }
    }
}

/// Streaming upload: only the meta enters the queue; the body is forwarded
/// chunk-by-chunk into the order's bounded channel while the worker processes.
async fn put_through_queue(
    identifier: &str,
    data_len: u64,
    body: hyper::body::Incoming,
) -> Result<ResponseStream, Status> {
    let con_queue = ConveyQueue::get_instance();
    // Admission happens before any body byte is read: a full pool pauses the
    // client via TCP backpressure rather than rejecting a started upload.
    let _permit = match con_queue.acquire_permit().await {
        Ok(permit) => permit,
        Err(AdmissionError::TimedOut) => return Err(Status::Overloaded),
        Err(_) => return Err(Status::InternalError),
    };

    let (reply_tx, reply_rx) = oneshot::channel();
    let (data_tx, order) = OrderRequest::create(
        Behavior::PutFile,
        0,
        Bytes::from(identifier.to_string()),
        data_len,
        false,
        reply_tx,
    );

    if let Err(e) = con_queue.produce_order(order).await {
        event!(Level::ERROR, "Failed to produce order: {}", e);
        return Err(Status::InternalError);
    }

    // The bounded channel throttles the socket to the worker's write rate.
    let mut body = body;
    let mut stream_failed = false;
    loop {
        match tokio::time::timeout(BODY_CHUNK_TIMEOUT, body.frame()).await {
            Ok(Some(Ok(frame))) => {
                if let Ok(data) = frame.into_data() {
                    if data_tx.send(data).await.is_err() {
                        stream_failed = true;
                        break;
                    }
                }
            }
            Ok(Some(Err(_))) | Err(_) => {
                stream_failed = true;
                break;
            }
            Ok(None) => break,
        }
    }
    drop(data_tx);

    if stream_failed {
        return Err(Status::BadRequest);
    }

    match tokio::time::timeout(Duration::from_secs(10), reply_rx).await {
        Ok(Ok(res)) => {
            if res.status == Status::Success {
                Ok(res)
            } else {
                Err(res.status)
            }
        }
        Ok(Err(_)) => Err(Status::InternalError),
        Err(_) => {
            event!(Level::ERROR, "S3 request timeout");
            Err(Status::InternalError)
        }
    }
}

async fn handle_s3(req: Request<hyper::body::Incoming>) -> Result<Response<S3Body>, hyper::http::Error> {
    let method = req.method().clone();
    let uri = req.uri().to_string();
    let path = uri.split('?').next().unwrap_or(&uri);
    let query = uri.split('?').nth(1).unwrap_or("");

    let some_mapper = mapper::get_mapper();

    let resp = match method {
        Method::GET => {
            let (bucket, key) = parse_s3_path(path);
            if bucket.is_none() {
                let buckets = match &some_mapper {
                    Some(m) => m.list_buckets().await.unwrap_or_else(|_| vec!["linastore".to_string()]),
                    None => vec!["linastore".to_string()],
                };
                build_response(StatusCode::OK, list_buckets_xml(&buckets), "application/xml")
            } else {
                let bucket = bucket.unwrap();
                if key.is_none() {
                    let prefix = query.split('&')
                        .find_map(|p| p.strip_prefix("prefix="))
                        .unwrap_or("");
                    let max_keys: u32 = query.split('&')
                        .find_map(|p| p.strip_prefix("max-keys=").and_then(|v| v.parse().ok()))
                        .unwrap_or(1000);

                    let files = match &some_mapper {
                        Some(m) => m.list_bucket(bucket, prefix).await.unwrap_or_default(),
                        None => vec![],
                    };
                    let total = files.len() as u32;
                    let is_truncated = total > max_keys;
                    let shown: Vec<_> = files.into_iter().take(max_keys as usize).collect();
                    build_response(StatusCode::OK, list_objects_xml(&shown, bucket, prefix, max_keys, is_truncated), "application/xml")
                } else {
                    let key = key.unwrap();
                    let internal_name: Option<String> = match &some_mapper {
                        Some(m) => m.resolve(bucket, key).await.unwrap_or(None),
                        None => None,
                    };
                    match internal_name {
                        Some(ref name) => {
                            match process_through_queue(Behavior::GetFile, &name).await {
                                Ok(res) => {
                                    let content_type = get_mime_type(key);
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .header("Content-Type", content_type)
                                        .header("Content-Length", res.data_len.to_string())
                                        .header("ETag", format!("\"{}\"", Uuid::new_v4().simple()))
                                        .body(stream_body(res.data))
                                        .unwrap()
                                }
                                Err(Status::FileNotFound) => {
                                    build_response(StatusCode::NOT_FOUND, s3_error_xml("NoSuchKey", "The specified key does not exist.", key), "application/xml")
                                }
                                Err(Status::Overloaded) => {
                                    build_response(StatusCode::SERVICE_UNAVAILABLE, s3_error_xml("SlowDown", "Please reduce your request rate.", key), "application/xml")
                                }
                                Err(_) => {
                                    build_response(StatusCode::INTERNAL_SERVER_ERROR, s3_error_xml("InternalError", "Internal server error", key), "application/xml")
                                }
                            }
                        }
                        None => build_response(StatusCode::NOT_FOUND, s3_error_xml("NoSuchKey", "The specified key does not exist.", key), "application/xml"),
                    }
                }
            }
        }
        Method::HEAD => {
            let (_, key) = parse_s3_path(path);
            match key {
                Some(k) => {
                    let (bucket, _) = parse_s3_path(path);
                    let internal_name = match (bucket, &some_mapper) {
                        (Some(b), Some(m)) => m.resolve(b, k).await.unwrap_or(None),
                        _ => None,
                    };
                    match internal_name {
                        Some(name) => {
                            match process_through_queue(Behavior::GetFile, &name).await {
                                Ok(res) => {
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .header("Content-Type", get_mime_type(k))
                                        .header("Content-Length", res.data_len.to_string())
                                        .header("ETag", format!("\"{}\"", Uuid::new_v4().simple()))
                                        .body(boxed_full(Bytes::new()))
                                        .unwrap()
                                }
                                Err(_) => build_empty_response(StatusCode::NOT_FOUND),
                            }
                        }
                        None => build_empty_response(StatusCode::NOT_FOUND),
                    }
                }
                None => build_empty_response(StatusCode::BAD_REQUEST),
            }
        }
        Method::PUT => {
            let (_, key) = parse_s3_path(path);
            let key = match key {
                Some(k) => k,
                None => return Ok(build_response(StatusCode::OK, String::new(), "application/xml")),
            };
            let (bucket, _) = parse_s3_path(path);
            let bucket = match bucket {
                Some(b) => b,
                None => return Ok(build_response(StatusCode::BAD_REQUEST, s3_error_xml("BadRequest", "Bucket name required", ""), "application/xml")),
            };
            let content_length: Option<u64> = req
                .headers()
                .get(hyper::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok());

            let internal_name = match &some_mapper {
                Some(m) => {
                    let suggested = Uuid::new_v4().to_string();
                    m.register(bucket, key, &suggested).await.unwrap_or(suggested)
                }
                None => Uuid::new_v4().to_string(),
            };

            match put_through_queue(
                &internal_name,
                content_length.unwrap_or(PAYLOAD_UNKNOWN_LEN),
                req.into_body(),
            )
            .await
            {
                Ok(_) => {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("ETag", format!("\"{}\"", Uuid::new_v4().simple()))
                        .body(boxed_full(Bytes::new()))
                        .unwrap()
                }
                Err(Status::BadRequest) => {
                    build_response(
                        StatusCode::BAD_REQUEST,
                        s3_error_xml("IncompleteBody", "The request body was aborted before completion.", key),
                        "application/xml",
                    )
                }
                Err(Status::Overloaded) => {
                    build_response(StatusCode::SERVICE_UNAVAILABLE, s3_error_xml("SlowDown", "Please reduce your request rate.", key), "application/xml")
                }
                Err(_) => {
                    build_response(StatusCode::INTERNAL_SERVER_ERROR, s3_error_xml("InternalError", "Failed to store object", key), "application/xml")
                }
            }
        }
        Method::DELETE => {
            let (bucket, key) = parse_s3_path(path);
            match (bucket, key) {
                (Some(b), Some(k)) => {
                    if let Some(m) = &some_mapper {
                        let internal_name = m.resolve(b, k).await.unwrap_or(None);
                        if let Some(name) = internal_name {
                            let _ = m.delete(b, k).await;
                            let _ = process_through_queue(Behavior::DeleteFile, &name).await;
                        }
                    }
                    build_empty_response(StatusCode::NO_CONTENT)
                }
                _ => build_empty_response(StatusCode::NO_CONTENT),
            }
        }
        _ => build_response(
            StatusCode::METHOD_NOT_ALLOWED,
            s3_error_xml("MethodNotAllowed", "The specified method is not allowed against this resource.", path),
            "application/xml",
        ),
    };
    Ok(resp)
}

#[instrument(skip_all)]
pub async fn run_s3_server(addr: &str) {
    event!(Level::INFO, "S3-compatible service starting on {}", addr);

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
            _ = shutdown_status.wait() => break,
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(req) => req,
                    Err(_) => continue,
                };

                super::net::tune_stream(&stream);
                let io = TokioIo::new(stream);
                tokio::task::spawn(async move {
                    if let Err(err) = http1::Builder::new()
                        .timer(TokioTimer::new())
                        .header_read_timeout(super::net::HEADER_READ_TIMEOUT)
                        .serve_connection(io, service_fn(handle_s3))
                        .await
                    {
                        event!(Level::ERROR, "Error serving S3 connection: {:?}", err);
                    }
                });
            }
        }
    }
}
