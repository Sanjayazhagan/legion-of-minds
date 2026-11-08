//! Clans: create, join, leave, roles, chat, and troops lent to clanmates.

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::players::load_user;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use bag_protocol::{
    canonical_army, canonical_troop, clean_text, with_borrowed, AttackSubmit, ChatMessage,
    ChatPost, ClanMember, ClanRole, ClanSummary, ClanView, CreateClan, Lend, Loan, Placement,
    PublicPlayer, User, CHAT_MAX, CLAN_DESC_MAX, CLAN_MAX_MEMBERS, CLAN_NAME_MAX, CLAN_NAME_MIN,
    MAX_LOANS,
};
use chrono::{DateTime, Utc};
use godcore::classes::Role;
use godcore::scenario::max_troop_level;
use godcore::troop::Troop;
use serde::Deserialize;
use sqlx::{PgConnection, PgPool};
use std::collections::HashMap;
use uuid::Uuid;

fn not_in_clan() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_in_clan", "you're not in a clan")
}

fn forbidden(msg: &str) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "forbidden", msg)
}

/// The player's clan and role, if any.
pub async fn membership(db: &mut PgConnection, user: Uuid) -> ApiResult<Option<(Uuid, ClanRole)>> {
    let r: Option<(Uuid, String)> =
        sqlx::query_as("SELECT clan_id, role FROM clan_members WHERE user_id = $1")
            .bind(user)
            .fetch_optional(&mut *db)
            .await?;
    Ok(r.map(|(c, role)| (c, ClanRole::parse(&role))))
}

pub async fn require_clan(db: &PgPool, user: Uuid) -> ApiResult<(Uuid, ClanRole)> {
    membership(&mut *db.acquire().await?, user)
        .await?
        .ok_or_else(not_in_clan)
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    id: Uuid,
    name: String,
    description: String,
    open: bool,
    war_searching: bool,
    war_wins: i32,
    war_losses: i32,
    war_draws: i32,
    members: i64,
    trophies: i64,
}

const SUMMARY_SELECT: &str = "SELECT c.id, c.name, c.description, c.open, c.war_searching,
        c.war_wins, c.war_losses, c.war_draws,
        COUNT(m.user_id) AS members, COALESCE(SUM(u.trophies), 0)::BIGINT AS trophies
    FROM clans c
    LEFT JOIN clan_members m ON m.clan_id = c.id
    LEFT JOIN users u ON u.id = m.user_id";

impl SummaryRow {
    fn summary(&self) -> ClanSummary {
        ClanSummary {
            id: self.id,
            name: self.name.clone(),
            description: self.description.clone(),
            open: self.open,
            members: self.members as u32,
            trophies: self.trophies,
            war_wins: self.war_wins,
            war_losses: self.war_losses,
            war_draws: self.war_draws,
        }
    }
}

pub async fn clan_view(db: &mut PgConnection, clan: Uuid) -> ApiResult<ClanView> {
    let s: SummaryRow = sqlx::query_as(&format!("{SUMMARY_SELECT} WHERE c.id = $1 GROUP BY c.id"))
        .bind(clan)
        .fetch_optional(&mut *db)
        .await?
        .ok_or_else(|| ApiError::not_found("clan"))?;
    let rows: Vec<(Uuid, String, i32, Option<i32>, String, i64)> = sqlx::query_as(
        "SELECT u.id, u.display_name, u.trophies, s.town_hall, m.role,
                (SELECT COUNT(*) FROM troop_loans l WHERE l.user_id = u.id AND l.clan_id = m.clan_id)
         FROM clan_members m
         JOIN users u ON u.id = m.user_id
         LEFT JOIN snapshots s ON s.id = u.current_snapshot
         WHERE m.clan_id = $1
         ORDER BY CASE m.role WHEN 'leader' THEN 0 WHEN 'elder' THEN 1 ELSE 2 END,
                  u.trophies DESC, m.joined_at ASC",
    )
    .bind(clan)
    .fetch_all(&mut *db)
    .await?;
    Ok(ClanView {
        clan: s.summary(),
        war_searching: s.war_searching,
        members: rows
            .into_iter()
            .map(|(id, name, trophies, th, role, lending)| ClanMember {
                player: PublicPlayer {
                    id,
                    display_name: name,
                    trophies,
                    town_hall: th,
                },
                role: ClanRole::parse(&role),
                lending: lending as u32,
            })
            .collect(),
    })
}

#[derive(Deserialize)]
pub struct ClanQuery {
    q: Option<String>,
    limit: Option<i64>,
}

/// Clans by total trophies, optionally filtered by name. Also the clan
/// leaderboard.
pub async fn list(
    State(st): State<AppState>,
    Query(q): Query<ClanQuery>,
) -> ApiResult<Json<Vec<ClanSummary>>> {
    let pattern = q
        .q
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")));
    let rows: Vec<SummaryRow> = sqlx::query_as(&format!(
        "{SUMMARY_SELECT} WHERE ($1::TEXT IS NULL OR c.name_key LIKE $1)
         GROUP BY c.id ORDER BY trophies DESC, c.created_at ASC LIMIT $2"
    ))
    .bind(pattern)
    .bind(q.limit.unwrap_or(50).clamp(1, 200))
    .fetch_all(&st.db)
    .await?;
    Ok(Json(rows.iter().map(SummaryRow::summary).collect()))
}

pub async fn get(
    State(st): State<AppState>,
    _user: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ClanView>> {
    Ok(Json(clan_view(&mut *st.db.acquire().await?, id).await?))
}

pub async fn mine(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<ClanView>> {
    let (clan, _) = require_clan(&st.db, user.id).await?;
    Ok(Json(clan_view(&mut *st.db.acquire().await?, clan).await?))
}

async fn say(db: &mut PgConnection, clan: Uuid, body: String) -> ApiResult<()> {
    sqlx::query("INSERT INTO clan_messages (clan_id, user_id, author, body) VALUES ($1, NULL, '', $2)")
        .bind(clan)
        .bind(body)
        .execute(&mut *db)
        .await?;
    Ok(())
}

async fn name_of(db: &mut PgConnection, user: Uuid) -> ApiResult<String> {
    Ok(sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&mut *db)
        .await?)
}

pub async fn create(
    State(st): State<AppState>,
    user: AuthUser,
    Json(req): Json<CreateClan>,
) -> ApiResult<Json<ClanView>> {
    let name = clean_text(&req.name, CLAN_NAME_MIN, CLAN_NAME_MAX, "clan name")?;
    let description = if req.description.trim().is_empty() {
        String::new()
    } else {
        clean_text(&req.description, 1, CLAN_DESC_MAX, "description")?
    };
    let mut tx = st.db.begin().await?;
    if membership(&mut tx, user.id).await?.is_some() {
        return Err(ApiError::conflict("in_clan", "leave your clan first"));
    }
    let id = Uuid::new_v4();
    let made = sqlx::query(
        "INSERT INTO clans (id, name, name_key, description, open) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (name_key) DO NOTHING",
    )
    .bind(id)
    .bind(&name)
    .bind(name.to_lowercase())
    .bind(&description)
    .bind(req.open)
    .execute(&mut *tx)
    .await?;
    if made.rows_affected() == 0 {
        return Err(ApiError::conflict("name_taken", "a clan with that name already exists"));
    }
    sqlx::query("INSERT INTO clan_members (user_id, clan_id, role) VALUES ($1, $2, 'leader')")
        .bind(user.id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let who = name_of(&mut tx, user.id).await?;
    say(&mut tx, id, format!("{who} founded {name}")).await?;
    let view = clan_view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(view))
}

pub async fn join(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ClanView>> {
    let mut tx = st.db.begin().await?;
    let open: Option<bool> = sqlx::query_scalar("SELECT open FROM clans WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let open = open.ok_or_else(|| ApiError::not_found("clan"))?;
    if membership(&mut tx, user.id).await?.is_some() {
        return Err(ApiError::conflict("in_clan", "leave your clan first"));
    }
    if !open {
        return Err(forbidden("this clan is closed"));
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clan_members WHERE clan_id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if n as usize >= CLAN_MAX_MEMBERS {
        return Err(ApiError::conflict("clan_full", "this clan is full"));
    }
    sqlx::query("INSERT INTO clan_members (user_id, clan_id, role) VALUES ($1, $2, 'member')")
        .bind(user.id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let who = name_of(&mut tx, user.id).await?;
    say(&mut tx, id, format!("{who} joined")).await?;
    let view = clan_view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(view))
}

/// Take `user` out of `clan`: their loans go, a leaving leader hands over
/// to the senior member, and an empty clan is deleted.
async fn remove(db: &mut PgConnection, clan: Uuid, user: Uuid) -> ApiResult<()> {
    let role: Option<String> =
        sqlx::query_scalar("DELETE FROM clan_members WHERE user_id = $1 AND clan_id = $2 RETURNING role")
            .bind(user)
            .bind(clan)
            .fetch_optional(&mut *db)
            .await?;
    sqlx::query("DELETE FROM troop_loans WHERE user_id = $1")
        .bind(user)
        .execute(&mut *db)
        .await?;
    if role.as_deref() == Some("leader") {
        let next: Option<Uuid> = sqlx::query_scalar(
            "SELECT user_id FROM clan_members WHERE clan_id = $1
             ORDER BY CASE role WHEN 'elder' THEN 0 ELSE 1 END, joined_at ASC LIMIT 1",
        )
        .bind(clan)
        .fetch_optional(&mut *db)
        .await?;
        match next {
            Some(n) => {
                sqlx::query("UPDATE clan_members SET role = 'leader' WHERE user_id = $1")
                    .bind(n)
                    .execute(&mut *db)
                    .await?;
                let who = name_of(db, n).await?;
                say(db, clan, format!("{who} now leads the clan")).await?;
            }
            None => {
                sqlx::query("DELETE FROM clans WHERE id = $1")
                    .bind(clan)
                    .execute(&mut *db)
                    .await?;
            }
        }
    }
    Ok(())
}

pub async fn leave(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<()>> {
    let mut tx = st.db.begin().await?;
    let (clan, _) = membership(&mut tx, user.id).await?.ok_or_else(not_in_clan)?;
    sqlx::query("SELECT id FROM clans WHERE id = $1 FOR UPDATE")
        .bind(clan)
        .execute(&mut *tx)
        .await?;
    let who = name_of(&mut tx, user.id).await?;
    // Said first: if this empties the clan, the message goes with it.
    say(&mut tx, clan, format!("{who} left")).await?;
    remove(&mut tx, clan, user.id).await?;
    tx.commit().await?;
    Ok(Json(()))
}

pub async fn kick(
    State(st): State<AppState>,
    user: AuthUser,
    Path(target): Path<Uuid>,
) -> ApiResult<Json<ClanView>> {
    let mut tx = st.db.begin().await?;
    let (clan, role) = membership(&mut tx, user.id).await?.ok_or_else(not_in_clan)?;
    let (their_clan, their_role) = membership(&mut tx, target)
        .await?
        .ok_or_else(|| ApiError::not_found("member"))?;
    if their_clan != clan || target == user.id {
        return Err(ApiError::not_found("member"));
    }
    let allowed = match role {
        ClanRole::Leader => true,
        ClanRole::Elder => their_role == ClanRole::Member,
        ClanRole::Member => false,
    };
    if !allowed {
        return Err(forbidden("you can't remove this member"));
    }
    let who = name_of(&mut tx, target).await?;
    remove(&mut tx, clan, target).await?;
    say(&mut tx, clan, format!("{who} was removed")).await?;
    let view = clan_view(&mut tx, clan).await?;
    tx.commit().await?;
    Ok(Json(view))
}

/// Leader only: a member becomes an elder; an elder becomes leader and the
/// old leader an elder.
pub async fn promote(
    State(st): State<AppState>,
    user: AuthUser,
    Path(target): Path<Uuid>,
) -> ApiResult<Json<ClanView>> {
    let mut tx = st.db.begin().await?;
    let (clan, role) = membership(&mut tx, user.id).await?.ok_or_else(not_in_clan)?;
    if role != ClanRole::Leader {
        return Err(forbidden("only the leader can promote"));
    }
    let (their_clan, their_role) = membership(&mut tx, target)
        .await?
        .ok_or_else(|| ApiError::not_found("member"))?;
    if their_clan != clan || target == user.id {
        return Err(ApiError::not_found("member"));
    }
    let who = name_of(&mut tx, target).await?;
    let (new, line) = match their_role {
        ClanRole::Member => ("elder", format!("{who} is now an elder")),
        _ => ("leader", format!("{who} now leads the clan")),
    };
    if new == "leader" {
        sqlx::query("UPDATE clan_members SET role = 'elder' WHERE user_id = $1")
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE clan_members SET role = $1 WHERE user_id = $2")
        .bind(new)
        .bind(target)
        .execute(&mut *tx)
        .await?;
    say(&mut tx, clan, line).await?;
    let view = clan_view(&mut tx, clan).await?;
    tx.commit().await?;
    Ok(Json(view))
}

// ---------------------------------------------------------------- chat

#[derive(Deserialize)]
pub struct ChatQuery {
    /// Only messages after this id.
    after: Option<i64>,
}

pub async fn chat(
    State(st): State<AppState>,
    user: AuthUser,
    Query(q): Query<ChatQuery>,
) -> ApiResult<Json<Vec<ChatMessage>>> {
    let (clan, _) = require_clan(&st.db, user.id).await?;
    let mut rows: Vec<(i64, Option<Uuid>, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, user_id, author, body, created_at FROM clan_messages
         WHERE clan_id = $1 AND id > $2 ORDER BY id DESC LIMIT 50",
    )
    .bind(clan)
    .bind(q.after.unwrap_or(0))
    .fetch_all(&st.db)
    .await?;
    rows.reverse();
    Ok(Json(
        rows.into_iter()
            .map(|(id, user_id, from, body, at)| ChatMessage {
                id,
                user_id,
                from,
                body,
                at: at.to_rfc3339(),
            })
            .collect(),
    ))
}

pub async fn post_chat(
    State(st): State<AppState>,
    user: AuthUser,
    Json(req): Json<ChatPost>,
) -> ApiResult<Json<ChatMessage>> {
    let body = clean_text(&req.body, 1, CHAT_MAX, "a message")?;
    let mut conn = st.db.acquire().await?;
    let (clan, _) = membership(&mut conn, user.id).await?.ok_or_else(not_in_clan)?;
    let from = name_of(&mut conn, user.id).await?;
    let (id, at): (i64, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO clan_messages (clan_id, user_id, author, body) VALUES ($1, $2, $3, $4)
         RETURNING id, created_at",
    )
    .bind(clan)
    .bind(user.id)
    .bind(&from)
    .bind(&body)
    .fetch_one(&mut *conn)
    .await?;
    Ok(Json(ChatMessage {
        id,
        user_id: Some(user.id),
        from,
        body,
        at: at.to_rfc3339(),
    }))
}

// ---------------------------------------------------------------- loans

fn parse_troop(s: &str) -> ApiResult<Troop> {
    serde_json::from_str(s).map_err(ApiError::internal)
}

async fn loans_where(db: &PgPool, clause: &str, clan: Uuid, user: Uuid) -> ApiResult<Vec<Loan>> {
    let rows: Vec<(Uuid, String, Uuid, String, i32, Option<i32>)> = sqlx::query_as(&format!(
        "SELECT l.id, l.data, u.id, u.display_name, u.trophies, s.town_hall
         FROM troop_loans l
         JOIN clan_members m ON m.user_id = l.user_id AND m.clan_id = l.clan_id
         JOIN users u ON u.id = l.user_id
         LEFT JOIN snapshots s ON s.id = u.current_snapshot
         WHERE l.clan_id = $1 AND {clause}
         ORDER BY u.trophies DESC, l.created_at ASC"
    ))
    .bind(clan)
    .bind(user)
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(|(id, data, uid, name, trophies, th)| {
            Ok(Loan {
                id,
                owner: PublicPlayer {
                    id: uid,
                    display_name: name,
                    trophies,
                    town_hall: th,
                },
                troop: parse_troop(&data)?,
            })
        })
        .collect()
}

/// Troops clanmates lend, which you can take on raids.
pub async fn loans(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<Loan>>> {
    let (clan, _) = require_clan(&st.db, user.id).await?;
    Ok(Json(loans_where(&st.db, "l.user_id <> $2", clan, user.id).await?))
}

/// Lend up to `MAX_LOANS` attackers to the clan, replacing earlier loans.
/// They're checked like raiding troops, against your own online upgrades.
pub async fn lend(
    State(st): State<AppState>,
    user: AuthUser,
    Json(req): Json<Lend>,
) -> ApiResult<Json<Vec<Loan>>> {
    if req.troops.len() > MAX_LOANS {
        return Err(bag_protocol::Invalid(format!("lend at most {MAX_LOANS} troops")).into());
    }
    let me = load_user(&st.db, user.id).await?;
    let (clan, _) = require_clan(&st.db, user.id).await?;
    let cap = max_troop_level(me.upgrades.town_hall);
    let mut checked = Vec::new();
    for t in &req.troops {
        let t = canonical_troop(t, Role::Attack, cap)?;
        if let Some(why) = me.upgrades.blocker(&t) {
            return Err(bag_protocol::Invalid(why).into());
        }
        if checked.iter().any(|c: &Troop| c.id == t.id) {
            return Err(bag_protocol::Invalid("lend each troop once".into()).into());
        }
        checked.push(t);
    }
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM troop_loans WHERE user_id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    for t in &checked {
        sqlx::query(
            "INSERT INTO troop_loans (id, user_id, clan_id, troop_id, data) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(user.id)
        .bind(clan)
        .bind(t.id as i64)
        .bind(serde_json::to_string(t).map_err(ApiError::internal)?)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Json(loans_where(&st.db, "l.user_id = $2", clan, user.id).await?))
}

/// Your own troops on loan to the clan.
pub async fn my_loans(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<Loan>>> {
    let (clan, _) = require_clan(&st.db, user.id).await?;
    Ok(Json(loans_where(&st.db, "l.user_id = $2", clan, user.id).await?))
}

/// The checked army for a raid or war attack: the player's own troops,
/// then any borrowed from clanmates.
pub async fn raid_army(db: &PgPool, me: &User, req: &AttackSubmit) -> ApiResult<Vec<Placement>> {
    let own = canonical_army(&req.army, me.upgrades.town_hall)?;
    me.upgrades.check(&own)?;
    if req.borrowed.is_empty() {
        return Ok(own);
    }
    let (clan, _) = require_clan(db, me.id).await?;
    let lent: HashMap<Uuid, Troop> = loans_where(db, "l.user_id <> $2", clan, me.id)
        .await?
        .into_iter()
        .map(|l| (l.id, l.troop))
        .collect();
    Ok(with_borrowed(own, &req.borrowed, |id| lent.get(&id).cloned())?)
}
