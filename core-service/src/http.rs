use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;

#[derive(Debug, Serialize)]
struct Website {
    id: String,
    name: String,
    project_path: String,
    runtime: String,
    status: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct CreateWebsite {
    name: String,
    project_path: String,
    runtime: String,
}

#[derive(Debug)]
struct ApiError(StatusCode, &'static str, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({
            "error": { "code": self.1, "message": self.2 }
        }))).into_response()
    }
}

fn database_error(error: rusqlite::Error) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database_error", error.to_string())
}

async fn require_bearer(
    State(token): State<String>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let expected = format!("Bearer {token}");
    let supplied = request.headers().get(AUTHORIZATION).and_then(|v| v.to_str().ok());
    if supplied != Some(expected.as_str()) {
        return (StatusCode::UNAUTHORIZED, Json(json!({
            "error": { "code": "unauthorized", "message": "A valid bearer token is required." }
        }))).into_response();
    }
    next.run(request).await
}

pub fn router(state: AppState, token: String) -> Router {
    let protected = Router::new()
        .route("/api/v1/websites", get(list_websites).post(create_website))
        .route("/api/v1/websites/:id", get(get_website).delete(delete_website))
        .layer(middleware::from_fn_with_state(token, require_bearer));

    Router::new()
        .route("/api/v1/health", get(health))
        .merge(protected)
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": "webdynamite-core", "api_version": 1 }))
}

async fn list_websites(State(state): State<AppState>) -> Result<Json<Vec<Website>>, ApiError> {
    let db = state.db.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database_locked", "Database lock is unavailable.".into()))?;
    let mut statement = db.prepare(
        "SELECT id, name, project_path, runtime, status, created_at, updated_at FROM websites ORDER BY created_at DESC"
    ).map_err(database_error)?;
    let rows = statement.query_map([], |row| Ok(Website {
        id: row.get(0)?,
        name: row.get(1)?,
        project_path: row.get(2)?,
        runtime: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })).map_err(database_error)?;
    let websites = rows.collect::<Result<Vec<_>, _>>().map_err(database_error)?;
    Ok(Json(websites))
}

async fn create_website(
    State(state): State<AppState>,
    Json(input): Json<CreateWebsite>,
) -> Result<(StatusCode, Json<Website>), ApiError> {
    let name = input.name.trim();
    let project_path = input.project_path.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_name", "Website name must contain 1 to 80 characters.".into()));
    }
    if project_path.is_empty() || project_path.len() > 1024 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_project_path", "Project path must contain 1 to 1024 characters.".into()));
    }
    if !std::path::Path::new(project_path).is_absolute() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_project_path", "Project path must be absolute.".into()));
    }
    let runtime = input.runtime.to_ascii_lowercase();
    if !["static", "node", "php", "python"].contains(&runtime.as_str()) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "unsupported_runtime", "Runtime must be one of: static, node, php, python.".into()));
    }

    let id = Uuid::new_v4().to_string();
    let db = state.db.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database_locked", "Database lock is unavailable.".into()))?;
    db.execute(
        "INSERT INTO websites(id, name, project_path, runtime) VALUES (?1, ?2, ?3, ?4)",
        params![id, name, project_path, runtime],
    ).map_err(database_error)?;

    let website = db.query_row(
        "SELECT id, name, project_path, runtime, status, created_at, updated_at FROM websites WHERE id = ?1",
        params![id],
        |row| Ok(Website {
            id: row.get(0)?, name: row.get(1)?, project_path: row.get(2)?,
            runtime: row.get(3)?, status: row.get(4)?, created_at: row.get(5)?, updated_at: row.get(6)?,
        }),
    ).map_err(database_error)?;
    Ok((StatusCode::CREATED, Json(website)))
}

async fn get_website(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Website>, ApiError> {
    let db = state.db.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database_locked", "Database lock is unavailable.".into()))?;
    let result = db.query_row(
        "SELECT id, name, project_path, runtime, status, created_at, updated_at FROM websites WHERE id = ?1",
        params![id],
        |row| Ok(Website {
            id: row.get(0)?, name: row.get(1)?, project_path: row.get(2)?,
            runtime: row.get(3)?, status: row.get(4)?, created_at: row.get(5)?, updated_at: row.get(6)?,
        }),
    );
    match result {
        Ok(website) => Ok(Json(website)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(ApiError(StatusCode::NOT_FOUND, "website_not_found", "No website exists with that ID.".into())),
        Err(error) => Err(database_error(error)),
    }
}

async fn delete_website(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let db = state.db.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database_locked", "Database lock is unavailable.".into()))?;
    let changed = db.execute("DELETE FROM websites WHERE id = ?1", params![id]).map_err(database_error)?;
    if changed == 0 {
        return Err(ApiError(StatusCode::NOT_FOUND, "website_not_found", "No website exists with that ID.".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}
