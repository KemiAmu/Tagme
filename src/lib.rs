use axum::Router;
use tracing::debug;

pub mod models;
pub mod token;

pub async fn serve(app: Router, port: u16) {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    debug!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

// pub async fn remove_tail_middleware(request: Request, next: Next) -> Response {
//     let path = request.uri().path();
//     if path.ends_with('/') && path != "/" {
//         Redirect::permanent(&path.trim_end_matches('/')).into_response()
//     } else {
//         next.run(request).await
//     }
// }

// pub fn static_file_routes<S>(root_path: &str) -> Router<S>
// where
//     S: Clone + Send + Sync + 'static,
// {
//     walkdir::WalkDir::new(root_path)
//         .into_iter()
//         .filter_map(Result::ok)
//         .filter(|e| e.file_type().is_file())
//         .filter_map(|e| {
//             let abs_path = e.path().to_str()?;
//             let rel_path = e.path().strip_prefix(root_path).ok()?;
//             Some((abs_path.to_string(), format!("/{}", rel_path.to_str()?)))
//         })
//         .fold(Router::new(), |router, (abs_path, rel_path)| {
//             debug!("static route: '{}' -> '{}'", rel_path, abs_path);
//             router.route(&rel_path, get_service(ServeFile::new(abs_path)))
//         })
// }
