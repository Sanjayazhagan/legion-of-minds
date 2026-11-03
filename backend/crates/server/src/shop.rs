//! Spending online gold. The Town Hall, weapons and troop levels a player
//! uses online are the ones bought here; anything bought offline only
//! counts offline.

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::players::{load_upgrades, load_user_in, weapon_key};
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use bag_protocol::{BuyWeapon, LevelUp, Upgrades, User};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// Lock the player's row for the purchase and read what they have.
async fn begin(st: &AppState, id: Uuid) -> ApiResult<(Transaction<'static, Postgres>, i64, Upgrades)> {
    let mut tx = st.db.begin().await?;
    let (gold, th): (i64, i32) =
        sqlx::query_as("SELECT gold, town_hall_level FROM users WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::not_found("player"))?;
    let up = load_upgrades(&mut tx, id, th).await?;
    Ok((tx, gold, up))
}

async fn pay(tx: &mut Transaction<'static, Postgres>, id: Uuid, gold: i64, price: i64) -> ApiResult<()> {
    if gold < price {
        return Err(ApiError::conflict(
            "not_enough_gold",
            format!("that costs {price} gold; you have {gold} online"),
        ));
    }
    sqlx::query("UPDATE users SET gold = gold - $1 WHERE id = $2")
        .bind(price)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn finish(mut tx: Transaction<'static, Postgres>, id: Uuid) -> ApiResult<Json<User>> {
    let u = load_user_in(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(u))
}

pub async fn upgrade_town_hall(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<User>> {
    let (mut tx, gold, up) = begin(&st, user.id).await?;
    pay(&mut tx, user.id, gold, up.town_hall_price()?).await?;
    sqlx::query("UPDATE users SET town_hall_level = town_hall_level + 1 WHERE id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    finish(tx, user.id).await
}

pub async fn buy_weapon(
    State(st): State<AppState>,
    user: AuthUser,
    Json(req): Json<BuyWeapon>,
) -> ApiResult<Json<User>> {
    let (mut tx, gold, up) = begin(&st, user.id).await?;
    pay(&mut tx, user.id, gold, up.weapon_price(req.weapon)?).await?;
    sqlx::query("INSERT INTO user_weapons (user_id, weapon) VALUES ($1, $2)")
        .bind(user.id)
        .bind(weapon_key(req.weapon))
        .execute(&mut *tx)
        .await?;
    finish(tx, user.id).await
}

pub async fn level_up(
    State(st): State<AppState>,
    user: AuthUser,
    Path(troop): Path<u64>,
    Json(req): Json<LevelUp>,
) -> ApiResult<Json<User>> {
    let id = i64::try_from(troop).map_err(|_| ApiError::not_found("troop"))?;
    let (mut tx, gold, up) = begin(&st, user.id).await?;
    pay(&mut tx, user.id, gold, up.level_price(troop, req.from)?).await?;
    sqlx::query(
        "INSERT INTO troop_levels (user_id, troop_id, level) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, troop_id) DO UPDATE SET level = EXCLUDED.level",
    )
    .bind(user.id)
    .bind(id)
    .bind(req.from as i32 + 1)
    .execute(&mut *tx)
    .await?;
    finish(tx, user.id).await
}
