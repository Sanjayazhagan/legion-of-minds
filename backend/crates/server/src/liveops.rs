//! Live operations seen from the game: running events, rewards the
//! operator tuned, and gifts waiting to be claimed. The operator's side is
//! in `admin`.
//!
//! Everything here changes payouts only (gold, shields, season length),
//! never battle rules, so the server's re-run of a raid still matches the
//! game's.

use crate::auth::AuthUser;
use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::players::load_user_in;
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use bag_protocol::{
    combined_gold_pct, combined_win_gold, scale_gold, Gift, GiftClaimed, League, LiveEvent,
    LiveView, WarResult, LEAGUES,
};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

/// A reward or timing the operator can change from the admin page.
pub struct Knob {
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub min: i64,
    pub max: i64,
}

const fn knob(
    key: &'static str,
    group: &'static str,
    label: &'static str,
    help: &'static str,
    min: i64,
    max: i64,
) -> Knob {
    Knob { key, group, label, help, min, max }
}

const GOLD_MAX: i64 = 1_000_000;

pub const KNOBS: &[Knob] = &[
    knob("season_days", "Seasons", "Season length (days)",
        "How long each new season lasts. To move the end of the running season, use Season above.", 1, 365),
    knob("season_reward_bronze", "Seasons", "End of season gold: Bronze", "Paid to everyone who finishes a season in Bronze.", 0, GOLD_MAX),
    knob("season_reward_silver", "Seasons", "End of season gold: Silver", "", 0, GOLD_MAX),
    knob("season_reward_gold", "Seasons", "End of season gold: Gold", "", 0, GOLD_MAX),
    knob("season_reward_crystal", "Seasons", "End of season gold: Crystal", "", 0, GOLD_MAX),
    knob("season_reward_master", "Seasons", "End of season gold: Master", "", 0, GOLD_MAX),
    knob("season_reward_champion", "Seasons", "End of season gold: Champion", "", 0, GOLD_MAX),
    knob("season_reward_legend", "Seasons", "End of season gold: Legend", "", 0, GOLD_MAX),
    knob("win_bonus_pct", "Raids", "League win bonus (%)",
        "Scales the extra gold per star that higher leagues pay. 100 is normal, 0 turns it off.", 0, 1000),
    knob("shield_minutes", "Raids", "Shield after being raided (minutes)",
        "How long a raided player can't be found again.", 0, 7 * 24 * 60),
    knob("war_reward_won", "Clan wars", "War won: gold per member", "", 0, GOLD_MAX),
    knob("war_reward_draw", "Clan wars", "War drawn: gold per member", "", 0, GOLD_MAX),
    knob("war_reward_lost", "Clan wars", "War lost: gold per member", "", 0, GOLD_MAX),
    knob("start_gold", "New players", "Starting online gold", "Online gold a brand new account gets.", 0, GOLD_MAX),
];

pub fn knob_of(key: &str) -> Option<&'static Knob> {
    KNOBS.iter().find(|k| k.key == key)
}

/// The built-in value of a setting: the server's environment settings, or
/// the game's constants.
pub fn default_of(cfg: &Config, key: &str) -> i64 {
    if let Some(l) = key.strip_prefix("season_reward_") {
        return LEAGUES
            .iter()
            .find(|x| crate::league::league_key(**x) == l)
            .map(|x| x.season_reward())
            .unwrap_or(0);
    }
    match key {
        "season_days" => (cfg.season.as_secs() / 86_400).max(1) as i64,
        "win_bonus_pct" => 100,
        "shield_minutes" => (cfg.shield.as_secs() / 60) as i64,
        "war_reward_won" => WarResult::Won.reward(),
        "war_reward_draw" => WarResult::Draw.reward(),
        "war_reward_lost" => WarResult::Lost.reward(),
        "start_gold" => cfg.start_gold,
        _ => 0,
    }
}

/// Settings and running events, loaded once per request that pays out.
pub struct Live {
    values: HashMap<&'static str, i64>,
    pub events: Vec<LiveEvent>,
}

impl Live {
    pub async fn load(db: &PgPool, cfg: &Config) -> Result<Live, sqlx::Error> {
        let mut values: HashMap<&'static str, i64> =
            KNOBS.iter().map(|k| (k.key, default_of(cfg, k.key))).collect();
        let rows: Vec<(String, i64)> = sqlx::query_as("SELECT key, value FROM settings")
            .fetch_all(db)
            .await?;
        for (k, v) in rows {
            if let Some(knob) = knob_of(&k) {
                values.insert(knob.key, v.clamp(knob.min, knob.max));
            }
        }
        Ok(Live {
            values,
            events: events(db, "ends_at > now() AND starts_at <= now()", 20).await?,
        })
    }

    pub fn get(&self, key: &str) -> i64 {
        self.values.get(key).copied().unwrap_or(0)
    }

    /// Gold multiplier from running events, in percent.
    pub fn gold_pct(&self) -> i32 {
        combined_gold_pct(&self.events)
    }

    /// Gold earned now, with running events applied.
    pub fn gold(&self, base: i64) -> i64 {
        scale_gold(base, self.gold_pct())
    }

    /// Flat extra gold for a won raid from running events.
    pub fn win_gold(&self) -> i64 {
        combined_win_gold(&self.events)
    }

    pub fn season_length(&self) -> Duration {
        Duration::from_secs(self.get("season_days").max(1) as u64 * 86_400)
    }

    pub fn shield(&self) -> Duration {
        Duration::from_secs(self.get("shield_minutes").max(0) as u64 * 60)
    }

    pub fn season_reward(&self, l: League) -> i64 {
        self.get(&format!("season_reward_{}", crate::league::league_key(l)))
    }

    /// The league's win bonus scaled by `win_bonus_pct`, before events.
    pub fn win_bonus(&self, l: League, stars: u32) -> i64 {
        scale_gold(l.win_bonus(stars), self.get("win_bonus_pct") as i32)
    }

    /// Gold each war member gets, before events.
    pub fn war_reward(&self, r: WarResult) -> i64 {
        self.get(match r {
            WarResult::Won => "war_reward_won",
            WarResult::Draw => "war_reward_draw",
            WarResult::Lost => "war_reward_lost",
        })
    }
}

type EventRow = (Uuid, String, String, DateTime<Utc>, DateTime<Utc>, i32, i64);

/// Events matching a fixed SQL condition, soonest ending first.
pub async fn events(db: &PgPool, filter: &str, limit: i64) -> Result<Vec<LiveEvent>, sqlx::Error> {
    let rows: Vec<EventRow> = sqlx::query_as(&format!(
        "SELECT id, title, message, starts_at, ends_at, gold_pct, win_gold FROM events
         WHERE {filter} ORDER BY starts_at, ends_at LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(db)
    .await?;
    let now = Utc::now();
    Ok(rows
        .into_iter()
        .map(|(id, title, message, s, e, gold_pct, win_gold)| LiveEvent {
            id,
            title,
            message,
            starts_at: s.to_rfc3339(),
            ends_at: e.to_rfc3339(),
            gold_pct,
            win_gold,
            active: s <= now && now < e,
        })
        .collect())
}

/// Gifts the player can still claim.
async fn open_gifts(db: &PgPool, user: Uuid) -> Result<Vec<Gift>, sqlx::Error> {
    let rows: Vec<(Uuid, i64, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT g.id, g.gold, g.message, g.expires_at FROM gifts g
         JOIN users u ON u.id = $1
         WHERE (g.user_id IS NULL OR g.user_id = $1) AND g.expires_at > now()
           AND NOT EXISTS (SELECT 1 FROM gift_claims c WHERE c.gift_id = g.id AND c.user_id = $1)
           -- A gift for everyone reaches players who joined up to its end.
           AND u.created_at < g.expires_at
         ORDER BY g.created_at LIMIT 20",
    )
    .bind(user)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, gold, message, e)| Gift {
            id,
            gold,
            message,
            expires_at: e.to_rfc3339(),
        })
        .collect())
}

/// `GET /v1/live`: running and upcoming events, the season and the
/// player's unclaimed gifts.
pub async fn live(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<LiveView>> {
    let ev = events(
        &st.db,
        "ends_at > now() AND starts_at < now() + interval '7 days'",
        20,
    )
    .await?;
    let season = crate::league::current(&st.db, Live::load(&st.db, &st.cfg).await?.season_length())
        .await
        .ok();
    Ok(Json(LiveView {
        gold_pct: combined_gold_pct(&ev),
        win_gold: combined_win_gold(&ev),
        events: ev,
        gifts: open_gifts(&st.db, user.id).await?,
        season,
    }))
}

/// `POST /v1/me/gifts/{id}/claim`: add a gift's gold to the player.
pub async fn claim(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<GiftClaimed>> {
    let mut tx = st.db.begin().await?;
    let gold: Option<i64> = sqlx::query_scalar(
        "SELECT g.gold FROM gifts g JOIN users u ON u.id = $2
         WHERE g.id = $1 AND (g.user_id IS NULL OR g.user_id = $2)
           AND g.expires_at > now() AND u.created_at < g.expires_at",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?;
    let gold = gold.ok_or_else(|| ApiError::not_found("gift"))?;
    let fresh = sqlx::query(
        "INSERT INTO gift_claims (gift_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(user.id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if fresh == 0 {
        return Err(ApiError::conflict("already_claimed", "you already claimed this gift"));
    }
    sqlx::query("UPDATE users SET gold = gold + $1 WHERE id = $2")
        .bind(gold)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let me = load_user_in(&mut tx, user.id).await?;
    tx.commit().await?;
    Ok(Json(GiftClaimed { gold, user: me }))
}
