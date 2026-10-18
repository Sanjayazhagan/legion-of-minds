//! Profiles, renaming, the leaderboard and publishing a base.

use crate::auth::{clean_name, AuthUser, SameRules};
use crate::error::{ApiError, ApiResult};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use bag_protocol::{canonical_snapshot, SIM_VERSION, BaseSnapshot, PublicPlayer, Published, RenameRequest, User};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    display_name: String,
    trophies: i32,
    gold: i64,
    shield_until: Option<DateTime<Utc>>,
    town_hall: Option<i32>,
    has_base: bool,
}

const USER_SELECT: &str = "SELECT u.id, u.display_name, u.trophies, u.gold, u.shield_until,
        s.town_hall, (u.current_snapshot IS NOT NULL) AS has_base
    FROM users u LEFT JOIN snapshots s ON s.id = u.current_snapshot";

pub async fn load_user<'e, E: sqlx::PgExecutor<'e>>(db: E, id: Uuid) -> ApiResult<User> {
    let r: UserRow = sqlx::query_as(&format!("{USER_SELECT} WHERE u.id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| ApiError::not_found("player"))?;
    Ok(User {
        id: r.id,
        display_name: r.display_name,
        trophies: r.trophies,
        gold: r.gold,
        town_hall: r.town_hall,
        has_base: r.has_base,
        shield_until: r
            .shield_until
            .filter(|t| *t > Utc::now())
            .map(|t| t.to_rfc3339()),
    })
}

pub fn public(u: &User) -> PublicPlayer {
    PublicPlayer {
        id: u.id,
        display_name: u.display_name.clone(),
        trophies: u.trophies,
        town_hall: u.town_hall,
    }
}

pub async fn me(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<User>> {
    Ok(Json(load_user(&st.db, user.id).await?))
}

pub async fn rename(
    State(st): State<AppState>,
    user: AuthUser,
    Json(req): Json<RenameRequest>,
) -> ApiResult<Json<User>> {
    sqlx::query("UPDATE users SET display_name = $1 WHERE id = $2")
        .bind(clean_name(&req.display_name))
        .bind(user.id)
        .execute(&st.db)
        .await?;
    Ok(Json(load_user(&st.db, user.id).await?))
}

pub async fn player(
    State(st): State<AppState>,
    _user: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PublicPlayer>> {
    Ok(Json(public(&load_user(&st.db, id).await?)))
}

#[derive(Deserialize)]
pub struct Page {
    limit: Option<i64>,
}

pub async fn leaderboard(
    State(st): State<AppState>,
    Query(q): Query<Page>,
) -> ApiResult<Json<Vec<PublicPlayer>>> {
    let rows: Vec<UserRow> = sqlx::query_as(&format!(
        "{USER_SELECT} ORDER BY u.trophies DESC, u.created_at ASC LIMIT $1"
    ))
    .bind(q.limit.unwrap_or(100).clamp(1, 200))
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| PublicPlayer {
                id: r.id,
                display_name: r.display_name,
                trophies: r.trophies,
                town_hall: r.town_hall,
            })
            .collect(),
    ))
}

/// Publish the base other players will raid: layout plus defender brains.
/// Checked and stored as a new immutable snapshot.
pub async fn publish_base(
    State(st): State<AppState>,
    _v: SameRules,
    user: AuthUser,
    Json(snap): Json<BaseSnapshot>,
) -> ApiResult<Json<Published>> {
    let snap = canonical_snapshot(&snap)?;
    let id = Uuid::new_v4();
    let data = serde_json::to_string(&snap).map_err(ApiError::internal)?;
    let mut tx = st.db.begin().await?;
    sqlx::query(
        "INSERT INTO snapshots (id, user_id, town_hall, data, sim_version) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
        .bind(user.id)
        .bind(snap.base.town_hall as i32)
        .bind(&data)
        .bind(SIM_VERSION as i32)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE users SET current_snapshot = $1 WHERE id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(Published {
        snapshot_id: id,
        town_hall: snap.base.town_hall,
        defenders: snap.defenders.len(),
    }))
}

pub async fn my_base(
    State(st): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<BaseSnapshot>> {
    let data: Option<String> = sqlx::query_scalar(
        "SELECT s.data FROM users u JOIN snapshots s ON s.id = u.current_snapshot WHERE u.id = $1",
    )
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    let data = data.ok_or_else(|| ApiError::not_found("published base"))?;
    Ok(Json(serde_json::from_str(&data).map_err(ApiError::internal)?))
}
