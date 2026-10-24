//! Embedded web console router .
//!
//! The SPA is embedded in the binary at compile time with `rust-embed`, and
//! every unknown path under the console prefix falls back to `index.html` so
//! client side routing works (, no Node on the execution
//! machine).
//!
//! The console is served under a configurable prefix (`/ui` by default), so
//! the SPA shell gets a `<base href>` element pointing at that prefix. Without
//! it the browser would resolve the relative asset URLs against the server
//! root and the console would load without styles or scripts.
//!
//! The routes are registered on the top level router (not nested) so that the
//! handlers can see the full request path and serve the right assets.

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use rust_embed::Embed;

use crate::AppState;

/// The built SPA, embedded at compile time.
#[derive(Embed)]
#[folder = "../../frontend/dist"]
struct Assets;

/// Builds the console routes for the given prefix (for example `/ui`).
pub fn router(prefix: &str, spa_fallback: &str) -> Router<AppState> {
    let prefix = prefix.trim_end_matches('/');
    let base = format!("{prefix}/");

    let fallback = spa_fallback
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("index.html")
        .to_owned();

    let index_route = prefix.to_string();
    let index_route_slash = format!("{prefix}/");
    let asset_route = format!("{prefix}/*path");

    let base_index = base.clone();
    let base_index_slash = base.clone();
    let base_asset = base;

    Router::new()
        .route(&index_route, get(move || serve_index(base_index.clone())))
        .route(&index_route_slash, get(move || serve_index(base_index_slash.clone())))
        .route(
            &asset_route,
            get(move |uri: Uri| serve_asset(uri, fallback.clone(), base_asset.clone())),
        )
}

/// Serves the SPA shell.
async fn serve_index(base: String) -> Response {
    serve_file("index.html", &base).await
}

/// Serves a static asset, falling back to the SPA entry point.
async fn serve_asset(uri: Uri, fallback: String, base: String) -> Response {
    // The route is registered as `{prefix}/*path`, so the request path still
    // contains the console prefix: it is stripped here to reach the bundle.
    let request_path = uri.path();
    let prefix = base.trim_end_matches('/');
    let relative =
        request_path.strip_prefix(prefix).unwrap_or(request_path).trim_start_matches('/');

    if relative.is_empty() || relative == "index.html" {
        return serve_file("index.html", &base).await;
    }
    if let Some(response) = try_serve(relative) {
        return response;
    }
    // A client side route (no file extension) is served by the SPA shell.
    if !relative.contains('.') {
        return serve_file(&fallback, &base).await;
    }
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8".to_owned())],
        "not found",
    )
        .into_response()
}

/// Serves an embedded file, injecting the SPA base URL into the shell.
async fn serve_file(path: &str, base: &str) -> Response {
    let Some(file) = Assets::get(path) else {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8".to_owned())],
            "console assets are not embedded in this build",
        )
            .into_response();
    };

    let mime = mime_of(path);
    let body = if path == "index.html" {
        inject_base(file.data.as_ref(), base)
    } else {
        file.data.into_owned()
    };

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime.to_owned()), (header::CACHE_CONTROL, "no-cache".to_owned())],
        body,
    )
        .into_response()
}

/// Serves an embedded file when it exists.
fn try_serve(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = mime_of(path);
    Some(
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime.to_owned()),
                (header::CACHE_CONTROL, "no-cache".to_owned()),
            ],
            file.data,
        )
            .into_response(),
    )
}

/// Inserts a `<base href>` element so relative asset URLs resolve inside the
/// console prefix.
fn inject_base(data: &[u8], base: &str) -> Vec<u8> {
    let Ok(html) = std::str::from_utf8(data) else {
        return data.to_vec();
    };
    let tag = format!("<base href=\"{base}\">");
    if html.contains("<base ") {
        return data.to_vec();
    }
    match html.find("<head>") {
        Some(index) => {
            let mut out = String::with_capacity(html.len() + tag.len());
            out.push_str(&html[..index + "<head>".len()]);
            out.push_str(&tag);
            out.push_str(&html[index + "<head>".len()..]);
            out.into_bytes()
        }
        None => data.to_vec(),
    }
}

/// Guesses the content type of an embedded asset.
fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    }
}
