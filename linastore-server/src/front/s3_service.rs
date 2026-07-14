use std::time::Duration;

use crate::{
    conveyer::ConveyQueue,
    dtos::{Behavior, Package, Status},
    mapper,
    shutdown::Shutdown,
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request, Response, StatusCode, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tracing::{Level, event, instrument};
use uuid::Uuid;

const S3_XML_NAMESPACE: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

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

fn build_response(status: StatusCode, body: String, content_type: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Content-Length", body.len().to_string())
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

fn build_empty_response(status: StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .unwrap()
}

async fn process_through_queue(behavior: Behavior, identifier: &str, data: Bytes) -> Result<Package, Status> {
    let uuid = Uuid::new_v4();
    let uni_id = uuid.into_bytes();
    let mut package = Package::new_with_id(&uuid);
    package.behavior = behavior;
    package.content.identifier = Bytes::copy_from_slice(identifier.as_bytes());
    package.content.data = data;

    let con_queue = ConveyQueue::get_instance();
    let receiver = match con_queue.register_waiter(uni_id) {
        Some(rx) => rx,
        None => return Err(Status::InternalError),
    };

    if let Err(e) = con_queue.produce_order(package) {
        event!(Level::ERROR, "Failed to produce order: {}", e);
        con_queue.unregister_waiter(uni_id);
        return Err(Status::InternalError);
    }

    match tokio::time::timeout(Duration::from_secs(10), receiver).await {
        Ok(Ok(pkg)) => {
            if pkg.status == Status::Success {
                Ok(pkg)
            } else {
                Err(pkg.status)
            }
        }
        Ok(Err(_)) => {
            con_queue.unregister_waiter(uni_id);
            con_queue.remove_order(uni_id);
            Err(Status::InternalError)
        }
        Err(_) => {
            event!(Level::ERROR, "S3 request timeout");
            con_queue.unregister_waiter(uni_id);
            con_queue.remove_order(uni_id);
            Err(Status::InternalError)
        }
    }
}

async fn handle_s3(req: Request<hyper::body::Incoming>) -> Result<Response<Full<Bytes>>, hyper::http::Error> {
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
                            match process_through_queue(Behavior::GetFile, &name, Bytes::new()).await {
                                Ok(pkg) => {
                                    let content_type = get_mime_type(key);
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .header("Content-Type", content_type)
                                        .header("Content-Length", pkg.content.data.len().to_string())
                                        .header("ETag", format!("\"{}\"", Uuid::new_v4().simple()))
                                        .body(Full::new(Bytes::from(pkg.content.data)))
                                        .unwrap()
                                }
                                Err(Status::FileNotFound) => {
                                    build_response(StatusCode::NOT_FOUND, s3_error_xml("NoSuchKey", "The specified key does not exist.", key), "application/xml")
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
                            match process_through_queue(Behavior::GetFile, &name, Bytes::new()).await {
                                Ok(pkg) => {
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .header("Content-Type", get_mime_type(k))
                                        .header("Content-Length", pkg.content.data.len().to_string())
                                        .header("ETag", format!("\"{}\"", Uuid::new_v4().simple()))
                                        .body(Full::new(Bytes::new()))
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
            let body_bytes = match req.into_body().collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(_) => return Ok(build_response(StatusCode::BAD_REQUEST, s3_error_xml("BadRequest", "Failed to read request body", key), "application/xml")),
            };

            let internal_name = Uuid::new_v4().to_string();
            if let Some(m) = &some_mapper {
                let _ = m.register(bucket, key, &internal_name).await;
            }

            match process_through_queue(Behavior::PutFile, &internal_name, body_bytes).await {
                Ok(_) => {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("ETag", format!("\"{}\"", Uuid::new_v4().simple()))
                        .body(Full::new(Bytes::new()))
                        .unwrap()
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
                            let _ = process_through_queue(Behavior::DeleteFile, &name, Bytes::new()).await;
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

    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(_) => {
            event!(Level::ERROR, "Failed to bind to address {}", addr);
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

                let io = TokioIo::new(stream);
                tokio::task::spawn(async move {
                    if let Err(err) = http1::Builder::new()
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
