//! The public HTTP server. Routes the shell has ported are answered here;
//! everything else is streamed through to the Python backend unchanged -
//! request and response bodies are never buffered, so /api/events (SSE)
//! and big downloads flow as they arrive.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use futures_util::TryStreamExt;

pub struct Backend {
    pub base: String,
    pub secret: String,
    pub client: reqwest::Client,
}

// hop-by-hop headers belong to one connection, not the request
const HOP: &[&str] = &[
    "connection", "keep-alive", "proxy-authenticate", "proxy-authorization",
    "te", "trailer", "transfer-encoding", "upgrade",
];
// ours: a client must never be able to supply these itself
const OURS: &[&str] = &["x-acecm-peer", "x-acecm-proxy"];

fn copy_headers(from: &HeaderMap, drop_ours: bool) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in from {
        let n = k.as_str();
        if HOP.contains(&n) || (drop_ours && OURS.contains(&n)) {
            continue;
        }
        out.append(k.clone(), v.clone());
    }
    out
}

async fn pass(
    State(be): State<Arc<Backend>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let pq = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let url = format!("{}{}", be.base, pq);
    // ⚠ Host is passed through untouched: the backend's CSRF check compares
    // Origin against it, so rewriting it to the private port would refuse
    // every write from our own window.
    let mut headers = copy_headers(&parts.headers, true);
    let ip = peer.ip().to_canonical().to_string();
    headers.insert(HeaderName::from_static("x-acecm-peer"),
                   HeaderValue::from_str(&ip).unwrap());
    headers.insert(HeaderName::from_static("x-acecm-proxy"),
                   HeaderValue::from_str(&be.secret).unwrap());
    let stream = body.into_data_stream();
    let up = be
        .client
        .request(parts.method, url)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await;
    match up {
        Ok(r) => {
            let mut resp = Response::builder().status(r.status().as_u16());
            if let Some(h) = resp.headers_mut() {
                *h = copy_headers(r.headers(), false);
            }
            let body = Body::from_stream(r.bytes_stream().map_err(std::io::Error::other));
            resp.body(body).unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
        }
        Err(e) => (StatusCode::BAD_GATEWAY,
                   format!("ACECM backend unavailable: {e}")).into_response(),
    }
}

/// Ported first; the rest falls through to the backend.
async fn dispatch(
    state: State<Arc<Backend>>,
    peer: ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let path = req.uri().path();
    let get = req.method() == axum::http::Method::GET || req.method() == axum::http::Method::HEAD;
    if get && !path.starts_with("/api/") {
        return crate::web::serve(path);
    }
    pass(state, peer, req).await
}

pub fn router(be: Arc<Backend>) -> Router {
    Router::new().fallback(dispatch).with_state(be)
}

pub async fn serve(listener: tokio::net::TcpListener, be: Arc<Backend>) {
    let app = router(be).into_make_service_with_connect_info::<SocketAddr>();
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("http server stopped: {e}");
    }
}
