//! Seeded content behind the platform's unchanged web Files renderer.
use crate::Shared;
use axum::{
    extract::{ws::Message, Query, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

const FILES: &[(&str, &str)] = &[
    ("index.md", "# Sample Brain\n\nA small knowledge base to explore on your iPhone. These are local example files, rendered by the same **Files** UI as the web app.\n\n## Start exploring\n\n- [[life/wiki/morning-rituals|Morning rituals]] — a linked Markdown note\n- [[life/wiki/reading|Reading notes]] — a table and a few ideas\n- [A plain text checklist](life/raw/checklist.txt)\n\nUse the **Files** button to browse folders, or search this brain by words. Your agents would normally keep these files up to date.\n"),
    ("life/index.md", "# A little room to think\n\nNotes about quieter mornings, good books, and making space for both.\n\n- [[wiki/morning-rituals|Morning rituals]]\n- [[wiki/reading|Reading notes]]\n"),
    ("life/wiki/morning-rituals.md", "# Morning rituals\n\nStart with **ten quiet minutes**, before your phone.\n\n1. Open a window.\n2. Make coffee or tea.\n3. Read something that makes you curious.\n\n> A good routine leaves room for the day to surprise you.\n\n## Related notes\n\nSee [[reading|Reading notes]] and the [original checklist](../raw/checklist.txt).\n"),
    ("life/wiki/reading.md", "# Reading notes\n\nA place for things worth coming back to.\n\n| Book | What stayed with me |\n| --- | --- |\n| A field guide | Notice the things closest to home. |\n| A book of essays | Leave a little room between ideas. |\n\n## A small experiment\n\nRead for ten minutes after coffee. Keep one sentence in your own words.\n\n[[morning-rituals|Back to morning rituals]]\n"),
    ("life/raw/checklist.txt", "A quieter morning\n\nOpen a window\nMake coffee\nRead for ten minutes\nWrite down one thought\n"),
    ("life/log.md", "# Activity log\n\n- Added a few example notes for the iPhone Files preview.\n- Linked the morning ritual and reading pages.\n\nThis is fixture content. No agent, model, or cloud account is connected.\n"),
];

pub const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Sample Brain</title><link rel="stylesheet" href="./__files.css"></head>
<body data-fragment="test-brain" data-title="Sample Brain">
<script type="module" src="./__files.js"></script></body></html>"#;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route(
            "/f/test-brain",
            get(|| async { Redirect::temporary("/f/test-brain/") }),
        )
        .route("/f/test-brain/", get(|| async { Html(PAGE) }))
        .route("/f/test-brain/__files", get(list))
        .route("/f/test-brain/__file", get(file))
        .route(
            "/f/test-brain/__files.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    include_str!("../../../cell/files.mjs"),
                )
            }),
        )
        .route(
            "/f/test-brain/__files.css",
            get(|| async {
                (
                    [("content-type", "text/css; charset=utf-8")],
                    include_str!("../../../cell/files.css"),
                )
            }),
        )
        .route(
            "/f/test-brain/__markdown.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    include_str!("../../../cell/markdown.mjs"),
                )
            }),
        )
        .route("/f/test-brain/__search", post(search))
        .route(
            "/f/test-brain/__op/index_status",
            post(|| async {
                Json(json!({"result":{"line":"Local preview · 6 files","busy":false}}))
            }),
        )
        .route("/f/test-brain/__watch", get(watch))
}

async fn list(headers: HeaderMap) -> Response {
    if !headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"))
    {
        return Html(PAGE).into_response();
    }
    Json(json!({
        "title":"Sample Brain", "home":"index.md", "searchable":true, "status":"index_status",
        "files":FILES.iter().map(|(path,text)| json!({"path":path,"size":text.len()})).collect::<Vec<_>>()
    })).into_response()
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

async fn file(Query(query): Query<FileQuery>) -> Response {
    match FILES.iter().find(|(path, _)| *path == query.path) {
        Some((_, text)) => ([("content-type", "text/plain; charset=utf-8")], *text).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct Search {
    q: String,
    limit: Option<usize>,
}

async fn search(Json(query): Json<Search>) -> Response {
    let q = query.q.trim().to_lowercase();
    if q.is_empty() || q.chars().count() > 256 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"message":"Search needs 1–256 characters."})),
        )
            .into_response();
    }
    let results = FILES.iter().filter(|(path, text)| {
        format!("{path}\n{text}").to_lowercase().contains(&q)
    }).take(query.limit.unwrap_or(20).clamp(1, 20)).map(|(path, text)| {
        json!({"source":path,"title":text.lines().next().unwrap_or(path).trim_start_matches("# "),"snippet":text.chars().take(240).collect::<String>()})
    }).collect::<Vec<_>>();
    Json(json!({"results":results})).into_response()
}

async fn watch(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(|mut socket| async move {
        if socket
            .send(Message::Text(json!({"type":"hello"}).to_string().into()))
            .await
            .is_err()
        {
            return;
        }
        while let Some(Ok(message)) = socket.recv().await {
            match message {
                Message::Ping(data) => {
                    if socket.send(Message::Pong(data)).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    })
}
