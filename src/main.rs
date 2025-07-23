use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Json, middleware};
use clap::Parser;
use serde::Deserialize;
use std::sync::Arc;

pub struct AppState {
    pub tags_db: sled::Db,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = 10_000_000)]
    cache: u64,
}

#[tokio::main]
async fn main() {
    use middleware::from_fn as mw_fn;
    use tagme as this_fuck_pack;

    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let state = AppState {
        tags_db: sled::Config::new()
            .path("data/tags.sled")
            .cache_capacity(args.cache)
            .open()
            .unwrap(),
    };

    let app = this_fuck_pack::static_file_routes("root/")
        .route("/{name}", get(user_page).post(user_handler))
        .fallback(this_fuck_pack::not_found_handle)
        .layer(mw_fn(this_fuck_pack::remove_tail_middleware))
        // .layer(mw_fn(this_fuck_pack::error_pages_middleware))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(Arc::new(state));

    this_fuck_pack::serve(app, 3000).await;
}

async fn user_page(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Html<String>, impl IntoResponse> {
    let tags = state
        .tags_db
        .get(&*name)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .and_then(|t| rmp_serde::from_slice::<Vec<(String, u32)>>(&t).ok())
        .unwrap_or_default();

    #[derive(Template)]
    #[template(path = "page.html")]
    struct UserPageTemplate {
        name: String,
        tags: Vec<(String, u32)>,
    }

    UserPageTemplate { name, tags }
        .render()
        .map(|r| Html(r))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
struct TagPost {
    tag: String,
}

async fn user_handler(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(tp): Json<TagPost>,
) -> Result<Json<serde_json::Value>, impl IntoResponse> {
    let result = state.tags_db.transaction(|tx| {
        use rmp_serde::{from_slice as encode, to_vec as decode};
        use sled::transaction::ConflictableTransactionError as CTError;

        let mut tags = tx
            .get(&*name)?
            .map(|t| encode::<Vec<(String, u32)>>(&t))
            .transpose()
            .map_err(|_| CTError::Abort(()))?
            .unwrap_or_default();

        if let Some((_, count)) = tags.iter_mut().find(|(tag, _)| *tag == tp.tag) {
            *count += 1;
            tags.sort_unstable_by_key(|(_, count)| std::cmp::Reverse(*count));
        } else {
            tags.push((tp.tag.clone(), 1));
        }

        tx.insert(&*name, decode(&tags).map_err(|_| CTError::Abort(()))?)?;
        Ok(tags)
    });

    result
        .map(|tags| Json(serde_json::json!({ "tags": tags })))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
