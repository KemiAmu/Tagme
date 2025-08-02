use axum::body::Body;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{Response, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, middleware};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

use tagme::models::{AppState, Top, Topic, UserData, UserInfo, UserStatus, with_transaction};
use tagme::token::{OptionalToken, Token};

#[tokio::main]
async fn main() {
    use middleware::from_fn as mw_fn;

    #[derive(Deserialize)]
    struct Config {
        cache: u64,
        compression: i32,
        github_oauth_client_id: String,
        github_oauth_client_secrets: String,
    }
    let config: Config = toml::from_str(&std::fs::read_to_string("config.toml").unwrap()).unwrap();

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .without_time() // systemd it
        .init();

    let state = Arc::new(AppState {
        db: sled::Config::new()
            .path("data.sled")
            .cache_capacity(config.cache)
            .compression_factor(config.compression)
            .open()
            .unwrap(),
        oauth_client_id: config.github_oauth_client_id,
        oauth_client_secrets: config.github_oauth_client_secrets,
    });

    let app = axum::Router::new()
        .route("/top", get(get_top))
        .route("/user", get(get_me))
        .route("/user/{*user}", get(get_user).delete(ban_user))
        .route(
            "/topic/{*topic}",
            get(get_topic).post(topic_handler).delete(del_topic),
        )
        .route("/add/tag/{*topic}", post(add_tags_handler))
        .route("/del/tag/{*topic}", post(del_tags_handler))
        .route("/oauth/callback", get(oauth_callback))
        .route("/private/admin", get(admin_handler))
        .route("/uwu", get(async || "Kemi Amu: uwu"))
        .fallback(async || StatusCode::BAD_REQUEST)
        //
        .layer(mw_fn(tagme::token::token_middleware))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    tagme::serve(app, 3000).await;
}

// top

async fn get_top(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<String>>, (StatusCode, &'static str)> {
    with_transaction(&state.db, |helper| {
        let top: Top = helper.get(&"")?.unwrap_or_default();
        Ok(Json(top.0))
    })
}

// user...

async fn get_me(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<Arc<Mutex<OptionalToken>>>,
) -> Result<Json<UserInfo>, (StatusCode, &'static str)> {
    let uid: u64 = token.lock().await.auth()?;
    with_transaction(&state.db, |helper| {
        let user_status: UserStatus = helper.get_or_not_found(&uid)?;
        Ok(Json(user_status.into_info(uid)))
    })
}

async fn get_user(
    State(state): State<Arc<AppState>>,
    Path(uid): Path<u64>,
) -> Result<Json<UserInfo>, (StatusCode, &'static str)> {
    with_transaction(&state.db, |helper| {
        let user_status: UserStatus = helper.get_or_not_found(&uid)?;
        Ok(Json(user_status.into_info(uid)))
    })
}

async fn ban_user(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<Arc<Mutex<OptionalToken>>>,
    Path(uid): Path<u64>,
) -> Result<StatusCode, (StatusCode, &'static str)> {
    let admin_uid: u64 = token.lock().await.auth()?;
    with_transaction(&state.db, |helper| {
        let admin_status: UserStatus = helper.get_or_not_found(&admin_uid)?;
        if !admin_status.is_admin() {
            return Err((StatusCode::FORBIDDEN, "No, Fuck You!"));
        }

        let mut user_status: UserStatus = helper.get_or_not_found(&uid)?;
        user_status = UserStatus::Banned(user_status.into_data());
        helper.insert(&uid, &user_status)?;
        Ok(StatusCode::OK)
    })
}

// topics...

#[derive(Serialize)]
struct TopicRes {
    description: String,
    author: String,
    tags: HashMap<String, u32>,
    pending_tags: HashSet<String>,
}

async fn get_topic(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<Arc<Mutex<OptionalToken>>>,
    Path(topic): Path<String>,
) -> Result<Response<Body>, (StatusCode, &'static str)> {
    let uid: Option<u64> = token.lock().await.get_sub();
    with_transaction(&state.db, |helper| {
        let topic_data: Topic = helper.get_or_not_found(&topic)?;

        let user_status: Option<UserStatus> =
            uid.map(|uid| helper.get_or_not_found(&uid)).transpose()?;
        let user_zipped: Option<(u64, &UserStatus)> = uid.zip(user_status.as_ref());
        let is_owner: bool = user_zipped.map_or(false, |(uid, s)| {
            s.verified_data(uid, topic_data.author).is_ok()
        });

        Ok(Json(TopicRes {
            description: topic_data.description,
            author: topic_data.author.to_string(),
            tags: topic_data.tags,
            pending_tags: is_owner
                .then(|| topic_data.pending_tags)
                .unwrap_or_default(),
        })
        .into_response())
    })
}

#[derive(Deserialize)]
struct TopicPost {
    description: String,
}

async fn topic_handler(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<Arc<Mutex<OptionalToken>>>,
    Path(topic): Path<String>,
    Json(post): Json<TopicPost>,
) -> Result<Json<TopicRes>, (StatusCode, &'static str)> {
    if topic.is_empty() || topic.len() > 128 {
        return Err((StatusCode::BAD_REQUEST, "Topic is invalid"));
    }
    let uid: u64 = token.lock().await.auth()?;

    with_transaction(&state.db, |helper| {
        let mut user_status: UserStatus = helper.get_or_not_found(&uid)?;

        if let Some(mut topic_data) = helper.get::<_, Topic>(&topic)? {
            user_status.verified_data(uid, topic_data.author)?;
            topic_data.description = post.description.clone();
            helper.insert(&topic, &topic_data)?;
            Ok(Json(TopicRes {
                description: topic_data.description,
                author: topic_data.author.to_string(),
                tags: topic_data.tags,
                pending_tags: topic_data.pending_tags,
            }))
        } else {
            let user: &mut UserData = user_status.active_data_mut()?;
            let topic_data = Topic {
                author: uid,
                description: post.description.clone(),
                tags: HashMap::new(),
                pending_tags: HashSet::new(),
            };
            helper.insert(&topic, &topic_data)?;

            let mut top: Top = helper.get(&"")?.unwrap_or_default();
            top.0.push(topic.clone());
            helper.insert(&"", &top)?;

            user.topics.push(topic.clone());
            helper.insert(&uid, &user_status)?;
            Ok(Json(TopicRes {
                description: topic_data.description,
                author: topic_data.author.to_string(),
                tags: topic_data.tags,
                pending_tags: topic_data.pending_tags,
            }))
        }
    })
}

async fn del_topic(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<Arc<Mutex<OptionalToken>>>,
    Path(topic): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, &'static str)> {
    let uid: u64 = token.lock().await.auth()?;
    with_transaction(&state.db, |helper| {
        let topic_data: Topic = helper.get_or_not_found(&topic)?;
        let mut user_status: UserStatus = helper.get_or_not_found(&uid)?;

        let user: &mut UserData = user_status.verified_data_mut(uid, topic_data.author)?;
        user.topics.retain(|t| t != &topic);
        helper.insert(&uid, &user_status)?;

        let mut top: Top = helper.get(&"")?.unwrap_or_default();
        top.0.retain(|t| t != &topic);
        helper.insert(&"", &top)?;

        helper.remove::<_, Topic>(&topic)?;
        Ok((StatusCode::SEE_OTHER, [("Location", "/")]))
    })
}

// tag...

#[derive(Deserialize)]
struct TagPost {
    tag: String,
}

async fn add_tags_handler(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<Arc<Mutex<OptionalToken>>>,
    Path(topic): Path<String>,
    Json(post): Json<TagPost>,
) -> Result<Json<TopicRes>, (StatusCode, &'static str)> {
    if post.tag.is_empty() || post.tag.len() > 64 {
        return Err((StatusCode::BAD_REQUEST, "Tag is invalid"));
    }
    let uid: Option<u64> = token.lock().await.get_sub();

    with_transaction(&state.db, |helper| {
        let mut topic_data: Topic = helper.get_or_not_found(&topic)?;
        let is_owner: bool = uid.map_or(false, |uid| {
            helper
                .get_or_not_found::<_, UserStatus>(&uid)
                .map_or(false, |s| s.verified_data(uid, topic_data.author).is_ok())
        });

        if let Some(count) = topic_data.tags.get_mut(&post.tag) {
            *count += 1;
        } else if is_owner {
            topic_data.tags.insert(post.tag.clone(), 1);
            topic_data.pending_tags.remove(&post.tag);
        } else {
            topic_data.pending_tags.insert(post.tag.clone());
        }

        helper.insert(&topic, &topic_data)?;
        Ok(Json(TopicRes {
            description: topic_data.description,
            author: topic_data.author.to_string(),
            tags: topic_data.tags,
            pending_tags: is_owner
                .then(|| topic_data.pending_tags)
                .unwrap_or_default(),
        }))
    })
}

async fn del_tags_handler(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<Arc<Mutex<OptionalToken>>>,
    Path(topic): Path<String>,
    Json(post): Json<TagPost>,
) -> Result<Json<serde_json::Value>, (StatusCode, &'static str)> {
    let uid: u64 = token.lock().await.auth()?;
    with_transaction(&state.db, |helper| {
        let mut topic_data: Topic = helper.get_or_not_found(&topic)?;
        helper
            .get_or_not_found::<_, UserStatus>(&uid)?
            .verified_data(uid, topic_data.author)?;

        topic_data.tags.remove(&post.tag);
        topic_data.pending_tags.remove(&post.tag);
        helper.insert(&topic, &topic_data)?;
        Ok(Json(json!({
            "description": topic_data.description,
            "tags": topic_data.tags,
            "pending_tags": topic_data.pending_tags,
        })))
    })
}

// oauth

async fn oauth_callback(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, (StatusCode, &'static str)> {
    let code = query
        .get("code")
        .ok_or((StatusCode::BAD_REQUEST, "No code"))?;
    let client = reqwest::Client::new();

    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .json(&json!({
            "client_id": state.oauth_client_id,
            "client_secret": state.oauth_client_secrets,
            "code": code,
        }))
        .send()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Failed to request token"))?;
    let token_data: serde_json::Value = resp
        .json()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Failed to parse token"))?;
    let access_token: String = token_data["access_token"]
        .as_str()
        .ok_or((StatusCode::BAD_REQUEST, "Invalid token response"))?
        .to_string();

    let user_resp = client
        .get("https://api.github.com/user")
        .bearer_auth(&access_token)
        .header("User-Agent", "KemiAmu:tagme")
        .send()
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to request user info",
            )
        })?;
    let user_data: serde_json::Value = user_resp.json().await.map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to parse user info",
        )
    })?;
    let github_id = user_data["id"]
        .as_u64()
        .ok_or((StatusCode::BAD_REQUEST, "Invalid user id"))?;
    let login = user_data["login"].as_str().unwrap_or("").to_string();
    let name = user_data["name"].as_str().unwrap_or("").to_string();
    let avatar_url = user_data["avatar_url"].as_str().unwrap_or("").to_string();
    let bio = user_data["bio"].as_str().unwrap_or("").to_string();
    let token = Token::new(github_id);

    with_transaction(&state.db, |helper| {
        let mut user_status: UserStatus = helper.get(&github_id)?.unwrap_or_default();
        let user: &mut UserData = user_status.data_mut();
        user.access_token = access_token.clone();
        user.login = login.clone();
        user.name = name.clone();
        user.avatar_url = avatar_url.clone();
        user.bio = bio.clone();
        helper.insert(&github_id, &user_status)?;

        Ok(Html(format!(
            r#"<!doctype html>
            <html><head><script>
            localStorage.setItem("Token", "{}");
            window.location.href = "/profile";
            </script></head></html>"#,
            token.to_string()
        )))
    })
}

// admin

async fn admin_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<(StatusCode, String), (StatusCode, &'static str)> {
    let uid: u64 = query
        .get("op")
        .or_else(|| query.get("deop"))
        .and_then(|s| s.parse().ok())
        .ok_or((StatusCode::BAD_REQUEST, "Invalid uid"))?;

    with_transaction(&state.db, |helper| {
        let mut user_status: UserStatus = helper.get_or_not_found(&uid)?;
        match query.contains_key("op") {
            true => {
                user_status = UserStatus::Admin(user_status.into_data());
                helper.insert(&uid, &user_status)?;
                Ok((
                    StatusCode::OK,
                    format!("User @{} promoted to admin\n", user_status.data().login),
                ))
            }
            false => {
                user_status = UserStatus::Normal(user_status.into_data());
                helper.insert(&uid, &user_status)?;
                Ok((
                    StatusCode::OK,
                    format!("User @{} demoted to normal\n", user_status.data().login),
                ))
            }
        }
    })
}
