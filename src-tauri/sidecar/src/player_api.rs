use std::sync::{Arc, RwLock};

use axum::{
    extract::{rejection::JsonRejection, State},
    http::{header::CACHE_CONTROL, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerInfo {
    pub current_track: Option<Value>,
    pub progress: f64,
}

impl Default for PlayerInfo {
    fn default() -> Self {
        Self {
            current_track: None,
            progress: 0.0,
        }
    }
}

#[derive(Clone, Default)]
pub struct PlayerApiState {
    inner: Arc<RwLock<PlayerInfo>>,
}

impl PlayerApiState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> PlayerInfo {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn update(&self, payload: &Value) -> bool {
        let Some(player_info) = decode_player_info(payload) else {
            return false;
        };
        *self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = player_info;
        true
    }
}

pub fn update_router(state: PlayerApiState) -> Router {
    Router::new()
        .route("/native/player-info", post(update_player_info_handler))
        .with_state(state)
}

// Mount this same router on the renderer and legacy listeners.
pub fn player_router(state: PlayerApiState) -> Router {
    Router::new()
        .route("/player", get(get_player_info_handler))
        .with_state(state)
}

pub async fn update_player_info_handler(
    State(state): State<PlayerApiState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Response {
    let Json(payload) = match payload {
        Ok(payload) => payload,
        Err(_) => return invalid_player_info_response(),
    };
    if !state.update(&payload) {
        return invalid_player_info_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

pub async fn get_player_info_handler(State(state): State<PlayerApiState>) -> Response {
    let mut response = Json(state.snapshot()).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn invalid_player_info_response() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "message": "播放器状态无效" })),
    )
        .into_response()
}

fn decode_player_info(value: &Value) -> Option<PlayerInfo> {
    let player_info = value.as_object()?;
    let current_track = match player_info.get("currentTrack")? {
        Value::Null => None,
        Value::Object(track) => {
            let id = track.get("id")?.as_number()?.as_f64()?;
            if !id.is_finite() {
                return None;
            }
            Some(Value::Object(track.clone()))
        }
        _ => return None,
    };
    let progress = player_info.get("progress")?.as_number()?.as_f64()?;
    if !progress.is_finite() || progress < 0.0 {
        return None;
    }
    Some(PlayerInfo {
        current_track,
        progress,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handlers_share_latest_snapshot_and_disable_caching() {
        let state = PlayerApiState::new();
        let update = update_player_info_handler(
            State(state.clone()),
            Ok(Json(json!({
                "currentTrack": { "id": 42, "name": "Track" },
                "progress": 12.5
            }))),
        )
        .await;
        assert_eq!(update.status(), StatusCode::NO_CONTENT);

        let response = get_player_info_handler(State(state.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-store"))
        );

        let snapshot = state.snapshot();
        assert_eq!(snapshot.progress, 12.5);
        assert_eq!(snapshot.current_track.as_ref().unwrap()["id"], 42);
        assert_eq!(state.clone().snapshot(), snapshot);
    }

    #[tokio::test]
    async fn invalid_update_is_rejected_without_overwriting_snapshot() {
        let state = PlayerApiState::new();
        assert!(state.update(&json!({
            "currentTrack": null,
            "progress": 3
        })));

        let response = update_player_info_handler(
            State(state.clone()),
            Ok(Json(json!({
                "currentTrack": { "id": 42 },
                "progress": -1
            }))),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            state.snapshot(),
            PlayerInfo {
                current_track: None,
                progress: 3.0,
            }
        );
    }
}
