use core::panic;
use std::net::SocketAddr;
use http_body_util::Full;
use tracing::{event, Level};
use std::convert::Infallible;

use tokio::net::TcpListener;
use hyper::body::Bytes;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use hyper::server::conn::http1;
use hyper::service::service_fn;

// One waitress handles one incoming request
async fn waitress(_: Request<hyper::body::Incoming>) -> Result<Response<Full<Bytes>>, Infallible> { 
    Ok(Response::new(Full::new(Bytes::from("Hello, World!"))))
}

pub async fn start() {
    event!(Level::INFO ,"Starting job...");
    let addr = SocketAddr::from(([0, 0, 0, 0], 8096));

    let listener = match TcpListener::bind(addr).await{
        Ok(listener) => listener,
        Err(_) => {
            event!(Level::ERROR, "Failed to bind to address {}", addr);
            panic!("Failed to bind to address");
        }
    };

    loop {
        //  Accept the connection
        let (stream, addr ) = match listener.accept().await {
            Ok(req) => req,
            Err(_) => {
                event!(Level::ERROR, "Failed to accept connection");
                continue;
            }
        };

        let io = TokioIo::new(stream);
        event!(Level::INFO, "Accepted connection from {}", addr);

        tokio::task::spawn( async move {
            if let Err(e) = http1::Builder::new()
                .serve_connection(io, service_fn(waitress))
                .await {
                event!(Level::ERROR, "Error serving connection: {}", e);
            }
        });
    }
}