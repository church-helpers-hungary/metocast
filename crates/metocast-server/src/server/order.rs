//! An event's Order of Service: its steps in the database, the REST pair that reads and replaces
//! them, and the live position the presenter is on (`order.*` WebSocket commands).

use axum::extract::ws::Message;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use metocast_core::best_effort::LogGone;
use metocast_core::events::{EventOrder, OrderItem, OrderItemKind, OrderPosition, SlideFolder};
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::database::settings;
use crate::models::event::fetch_event;
use crate::server::AppState;
use crate::server::presenter::{self, BibleReferenceType, BibleVerseContent, PresenterState};
use crate::server::{routes, websocket, welcome};

const MAX_ITEMS: usize = 200;
const MAX_TEXT: usize = 2_000;

pub async fn fetch(
    pool: impl sqlx::PgExecutor<'_>,
    event_id: Uuid,
) -> sqlx::Result<Vec<OrderItem>> {
    let rows = sqlx::query(
        "SELECT id, kind, title, deck_path, bible_type, reference, translation, notes FROM event_order_items \
         WHERE event_id = $1 ORDER BY position",
    )
    .bind(event_id)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(OrderItem {
                id: Some(row.try_get("id")?),
                // The column's CHECK constraint allows only the three kinds.
                kind: OrderItemKind::parse(row.try_get("kind")?).unwrap_or(OrderItemKind::Block),
                title: row.try_get("title")?,
                deck_path: row.try_get("deck_path")?,
                bible_type: row.try_get("bible_type")?,
                reference: row.try_get("reference")?,
                translation: row.try_get("translation")?,
                notes: row.try_get("notes")?,
            })
        })
        .collect()
}

/// Why a step can't be stored, in words for the operator.
fn invalid(items: &[OrderItem]) -> Option<String> {
    if items.len() > MAX_ITEMS {
        return Some(format!(
            "An Order of Service holds at most {MAX_ITEMS} steps"
        ));
    }
    for (index, item) in items.iter().enumerate() {
        let step = index + 1;
        if item.title.chars().count() > 200 || item.notes.chars().count() > MAX_TEXT {
            return Some(format!("Step {step} has too much text"));
        }
        match item.kind {
            // A song may have no deck yet, or need none (a closing verse sung from memory),
            // but then its title has to say which song it is.
            OrderItemKind::Song
                if item.title.trim().is_empty()
                    && item
                        .deck_path
                        .as_deref()
                        .is_none_or(|p| p.trim().is_empty()) =>
            {
                return Some(format!("Step {step} needs a title or a deck"));
            }
            OrderItemKind::Bible
                if !matches!(item.bible_type.as_deref(), Some("textus" | "leckio"))
                    && own_reading(item).is_none() =>
            {
                return Some(format!(
                    "Step {step} needs a Bible reference, or must be the Textus or the Lekció"
                ));
            }
            OrderItemKind::Block | OrderItemKind::Announcements | OrderItemKind::Text
                if item.title.trim().is_empty() =>
            {
                return Some(format!("Step {step} needs a title"));
            }
            _ => {}
        }
    }
    None
}

/// A Bible step's own reading (reference, translation), when it isn't the event's Textus or
/// Lekció. Without a translation it reads RUF (v2), like a new event's readings.
fn own_reading(item: &OrderItem) -> Option<(&str, &str)> {
    if item.kind != OrderItemKind::Bible || item.bible_type.is_some() {
        return None;
    }
    let reference = item
        .reference
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())?;
    let translation = item
        .translation
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("RUF_v2");
    Some((reference, translation))
}

/// The verses stored for each step that reads a passage of its own, by step id, with the
/// reference and translation they were looked up for.
async fn stored_verses(
    pool: impl sqlx::PgExecutor<'_>,
    event_id: Uuid,
) -> sqlx::Result<std::collections::HashMap<Uuid, (String, String, serde_json::Value)>> {
    let rows = sqlx::query(
        "SELECT id, reference, translation, verses FROM event_order_items \
         WHERE event_id = $1 AND verses IS NOT NULL",
    )
    .bind(event_id)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok((
                row.try_get("id")?,
                (
                    row.try_get::<Option<String>, _>("reference")?
                        .unwrap_or_default(),
                    row.try_get::<Option<String>, _>("translation")?
                        .unwrap_or_default(),
                    row.try_get("verses")?,
                ),
            ))
        })
        .collect()
}

/// Looks up the verses of every step whose own reading is new or changed, so a saved list can
/// be shown without the network. Unchanged steps keep what they have.
async fn look_up_readings(
    state: &AppState,
    event_id: Uuid,
    items: &[OrderItem],
) -> Result<Vec<(usize, serde_json::Value)>, String> {
    let stored = stored_verses(&state.pool, event_id)
        .await
        .map_err(|e| e.to_string())?;
    let szentiras: crate::connectors::SzentirasConfig =
        settings::get_json(&state.pool, "szentiras_config").await;
    let api_key = Some(szentiras.api_key.as_str()).filter(|key| !key.is_empty());
    let mut found = Vec::new();
    for (position, item) in items.iter().enumerate() {
        let Some((reference, translation)) = own_reading(item) else {
            continue;
        };
        let kept = item
            .id
            .and_then(|id| stored.get(&id))
            .is_some_and(|(r, t, _)| r == reference && t == translation);
        if kept {
            continue;
        }
        let passage = crate::bible::fetch_passage(reference, translation, api_key)
            .await
            .map_err(|e| format!("Step {}: {e}", position + 1))?;
        if passage.verses.is_empty() {
            return Err(format!(
                "Step {}: no verses found for {reference}",
                position + 1
            ));
        }
        let verses = serde_json::to_value(&passage.verses).map_err(|e| e.to_string())?;
        found.push((position, verses));
    }
    Ok(found)
}

async fn event_exists(pool: &PgPool, id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM events WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
}

pub async fn get_order(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    match event_exists(&state.pool, id).await {
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("get_order: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Ok(true) => {}
    }
    match fetch(&state.pool, id).await {
        Ok(items) => Json(EventOrder { items }).into_response(),
        Err(e) => {
            tracing::error!("get_order: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Replaces the whole list. Steps that keep their `id` keep being the live step.
pub async fn put_order(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<EventOrder>,
) -> impl IntoResponse {
    if let Some(error) = invalid(&body.items) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    match event_exists(&state.pool, id).await {
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("put_order: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Ok(true) => {}
    }
    let looked_up = match look_up_readings(&state, id, &body.items).await {
        Ok(found) => found,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    let replaced = async {
        let mut tx = state.pool.begin().await?;
        write(&mut tx, id, &body.items).await?;
        for (position, verses) in &looked_up {
            sqlx::query(
                "UPDATE event_order_items SET verses = $1 WHERE event_id = $2 AND position = $3",
            )
            .bind(verses)
            .bind(id)
            .bind(*position as i32)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    };
    if let Err(e) = replaced.await {
        // A repeated step id is the only way a valid list fails here.
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    match announce(&state, id).await {
        Ok(items) => Json(EventOrder { items }).into_response(),
        Err(e) => {
            tracing::error!("put_order: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Tells every client that an event's list changed: `event.order.changed`, and `order.state`
/// when that event is on screen, so a renamed live step shows everywhere (Companion too).
pub async fn announce(state: &AppState, event_id: Uuid) -> sqlx::Result<Vec<OrderItem>> {
    let items = fetch(&state.pool, event_id).await?;
    websocket::broadcast_json(
        &state.ws_clients,
        &json!({ "type": "event.order.changed", "eventId": event_id, "items": &items }),
    )
    .await;
    let mut position = state.order_position.write().await;
    if position.is_some_and(|p| p.event_id == event_id) {
        // The live step was removed: nothing is current any more.
        if position.is_some_and(|p| !items.iter().any(|i| i.id == Some(p.item_id))) {
            *position = None;
        }
        drop(position);
        broadcast_position(state).await;
    }
    Ok(items)
}

/// The steps a new event starts with: placeholders the operator fills in. Songs have no deck
/// until one is picked from the library.
fn template() -> Vec<OrderItem> {
    let step = |kind, title: &str, bible_type: Option<&str>| OrderItem {
        id: None,
        kind,
        title: title.to_string(),
        deck_path: None,
        bible_type: bible_type.map(str::to_string),
        reference: None,
        translation: None,
        notes: String::new(),
    };
    use OrderItemKind::{Announcements, Bible, Block, Song, Text};
    vec![
        step(Text, "Welcome message", None),
        step(Song, "Song 1", None),
        step(Song, "Song 2", None),
        step(Bible, "Lekció", Some("leckio")),
        step(Song, "Song 3", None),
        step(Bible, "Textus", Some("textus")),
        step(Block, "Preaching", None),
        step(Song, "Song 4", None),
        step(Announcements, "Hirdetések", None),
        step(Block, "Áldás", None),
        step(Song, "Song 4 last part", None),
    ]
}

/// Stores the template for an event just created, inside its transaction.
pub async fn seed_template(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_id: Uuid,
) -> sqlx::Result<()> {
    let mut items = template();
    // The welcome step is named after the greeting set for the welcome screen.
    items[0].title = welcome::greeting(tx).await?;
    write(tx, event_id, &items).await
}

/// `items` with a step for each reading in `types` that has none, or `None` if all have one.
/// The Lekció goes before the Textus step and the Textus after the Lekció step, as the service
/// reads them; otherwise at the end.
fn with_readings(mut items: Vec<OrderItem>, types: &[String]) -> Option<Vec<OrderItem>> {
    let has = |items: &[OrderItem], t: &str| {
        items
            .iter()
            .any(|i| i.kind == OrderItemKind::Bible && i.bible_type.as_deref() == Some(t))
    };
    let mut changed = false;
    for (bible_type, title) in [("leckio", "Lekció"), ("textus", "Textus")] {
        if !types.iter().any(|t| t == bible_type) || has(&items, bible_type) {
            continue;
        }
        let other = if bible_type == "leckio" {
            "textus"
        } else {
            "leckio"
        };
        let at = items
            .iter()
            .position(|i| i.kind == OrderItemKind::Bible && i.bible_type.as_deref() == Some(other))
            .map_or(
                items.len(),
                |i| if bible_type == "leckio" { i } else { i + 1 },
            );
        items.insert(
            at,
            OrderItem {
                id: None,
                kind: OrderItemKind::Bible,
                title: title.to_string(),
                deck_path: None,
                bible_type: Some(bible_type.to_string()),
                reference: None,
                translation: None,
                notes: String::new(),
            },
        );
        changed = true;
    }
    changed.then_some(items)
}

/// Adds a step for each reading in `types` (the ones just set or changed) that has none. True if
/// the list changed, so the caller announces it once the transaction is committed.
pub async fn add_readings(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_id: Uuid,
    types: &[String],
) -> sqlx::Result<bool> {
    if types.is_empty() {
        return Ok(false);
    }
    let items = fetch(&mut **tx, event_id).await?;
    if items.len() >= MAX_ITEMS {
        return Ok(false);
    }
    match with_readings(items, types) {
        Some(items) => write(tx, event_id, &items).await.map(|()| true),
        None => Ok(false),
    }
}

/// Replaces the event's steps. A step that keeps its id and its own reading keeps the verses
/// stored for it; `put_order` stores the ones it looked up.
async fn write(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_id: Uuid,
    items: &[OrderItem],
) -> sqlx::Result<()> {
    let stored = stored_verses(&mut **tx, event_id).await?;
    sqlx::query("DELETE FROM event_order_items WHERE event_id = $1")
        .bind(event_id)
        .execute(&mut **tx)
        .await?;
    for (position, item) in items.iter().enumerate() {
        let own = own_reading(item);
        let kept = item
            .id
            .and_then(|id| stored.get(&id))
            .filter(|(r, t, _)| {
                own.is_some_and(|(reference, translation)| r == reference && t == translation)
            })
            .map(|(_, _, verses)| verses.clone());
        sqlx::query(
            "INSERT INTO event_order_items \
             (id, event_id, position, kind, title, deck_path, bible_type, notes, reference, translation, verses) \
             VALUES (COALESCE($1, gen_random_uuid()), $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(item.id)
        .bind(event_id)
        .bind(position as i32)
        .bind(item.kind.as_str())
        .bind(item.title.trim())
        .bind(item.deck_path.as_deref().filter(|_| item.kind == OrderItemKind::Song))
        .bind(item.bible_type.as_deref().filter(|_| item.kind == OrderItemKind::Bible))
        .bind(&item.notes)
        .bind(own.map(|(r, _)| r))
        .bind(own.map(|(_, t)| t))
        .bind(kept)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// `order.state`: the step on screen and its event's steps, so a read-only view (the shared
/// Order of Service page) needs nothing but the socket.
pub async fn position_message(state: &AppState) -> String {
    let position = *state.order_position.read().await;
    let items = match position {
        Some(p) => fetch(&state.pool, p.event_id).await.unwrap_or_default(),
        None => Vec::new(),
    };
    json!({ "type": "order.state", "position": position, "items": items }).to_string()
}

async fn broadcast_position(state: &AppState) {
    let message = position_message(state).await;
    for tx in state.ws_clients.read().await.values() {
        tx.send(Message::Text(message.clone().into())).log_gone();
    }
}

/// Shows one step: a song opens its deck, a Bible step loads the event's reading, the
/// announcements become slides of their bullets, a text step its own slide, and a block leaves the screen as it is. Then every client hears which step is current.
pub async fn go(
    state: &AppState,
    event_id: Uuid,
    item_id: Uuid,
    client_tx: &crate::server::ws_client::ClientTx,
) -> Result<(), String> {
    let items = fetch(&state.pool, event_id)
        .await
        .map_err(|e| e.to_string())?;
    let item = items
        .iter()
        .find(|item| item.id == Some(item_id))
        .ok_or("That step is not in the event's Order of Service")?;
    match item.kind {
        // A song with no deck leaves the screen as it is, like a block.
        OrderItemKind::Song => {
            if let Some(path) = item.deck_path.clone().filter(|p| !p.trim().is_empty()) {
                websocket::open_presentation(state, path, None, client_tx).await;
            }
        }
        OrderItemKind::Bible => match own_reading(item) {
            Some((reference, _)) => show_own_reading(state, event_id, item, reference).await?,
            None => {
                show_reading(
                    state,
                    event_id,
                    item.bible_type.as_deref().unwrap_or_default(),
                )
                .await?
            }
        },
        OrderItemKind::Block => {}
        // The web presenter gets the welcome design; Keynote a plain text slide.
        OrderItemKind::Text
            if state
                .use_web_presenter
                .load(std::sync::atomic::Ordering::Relaxed) =>
        {
            let event = fetch_event(event_id, &state.pool)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("Event not found")?;
            welcome::show(state, &item.title, &item.notes, event.date_time).await?
        }
        OrderItemKind::Text => {
            let deck = PresenterState::from_text(&item.title, &item.notes);
            show_deck(state, deck, "szoveg.pptx").await?
        }
        OrderItemKind::Announcements => {
            let deck = PresenterState::from_announcements(&item.title, &item.notes);
            show_deck(state, deck, "hirdetesek.pptx").await?
        }
    }
    *state.order_position.write().await = Some(OrderPosition { event_id, item_id });
    broadcast_position(state).await;
    Ok(())
}

/// Moves `by` steps from the current one. With nothing current, `event_id` names the event and
/// the first step is shown.
pub async fn step(
    state: &AppState,
    event_id: Option<Uuid>,
    by: isize,
    client_tx: &crate::server::ws_client::ClientTx,
) -> Result<(), String> {
    let current = *state.order_position.read().await;
    let event_id = event_id
        .or(current.map(|p| p.event_id))
        .ok_or("No Order of Service is running. Pick a step first.")?;
    let items = fetch(&state.pool, event_id)
        .await
        .map_err(|e| e.to_string())?;
    let index = match current.filter(|p| p.event_id == event_id) {
        Some(p) => {
            let at = items
                .iter()
                .position(|item| item.id == Some(p.item_id))
                .unwrap_or(0) as isize;
            at + by
        }
        None => 0,
    };
    let Some(item) = usize::try_from(index).ok().and_then(|i| items.get(i)) else {
        return Err(if items.is_empty() {
            "This event has no Order of Service".to_string()
        } else if by > 0 {
            "This is the last step".to_string()
        } else {
            "This is the first step".to_string()
        });
    };
    go(state, event_id, item.id.unwrap_or_default(), client_tx).await
}

/// Puts one of the event's readings on screen: into the web presenter, or as a deck in the Bible
/// slide folder that Keynote opens.
async fn show_reading(state: &AppState, event_id: Uuid, bible_type: &str) -> Result<(), String> {
    let event = fetch_event(event_id, &state.pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Event not found")?;
    let (kind, file_name) = match bible_type {
        "textus" => (BibleReferenceType::Textus, "textus.pptx"),
        _ => (BibleReferenceType::Leckio, "lekcio.pptx"),
    };
    let reference = event
        .bible_references
        .iter()
        .find(|r| r.r#type == kind.as_str())
        .ok_or("The event has no such reading. Add it to the event first.")?;
    let verses: Vec<BibleVerseContent> =
        serde_json::from_value(reference.verses.clone()).unwrap_or_default();
    if verses.is_empty() {
        return Err("The event's reading has no verses yet".to_string());
    }
    let deck =
        PresenterState::from_bible_reference(&event.title, kind, &reference.reference, verses);
    show_deck(state, deck, file_name).await
}

/// Puts a step's own reading on screen, from the verses stored when it was saved.
async fn show_own_reading(
    state: &AppState,
    event_id: Uuid,
    item: &OrderItem,
    reference: &str,
) -> Result<(), String> {
    let event = fetch_event(event_id, &state.pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Event not found")?;
    let verses = stored_verses(&state.pool, event_id)
        .await
        .map_err(|e| e.to_string())?
        .remove(&item.id.unwrap_or_default())
        .map(|(_, _, verses)| verses)
        .ok_or("This reading has no verses yet. Save it again to look them up.")?;
    let verses: Vec<BibleVerseContent> = serde_json::from_value(verses).unwrap_or_default();
    let heading = if item.title.trim().is_empty() {
        reference
    } else {
        item.title.trim()
    };
    let deck = PresenterState::from_reading(&event.title, heading, reference, verses);
    show_deck(state, deck, "ige.pptx").await
}

/// Puts a deck the server built on screen: into the web presenter, or written as `file_name` for
/// Keynote to open, into the Bible slide folder or, with none set, a temporary folder.
async fn show_deck(state: &AppState, deck: PresenterState, file_name: &str) -> Result<(), String> {
    if state
        .use_web_presenter
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        *state.presenter_state.write().await = deck;
        websocket::broadcast_presenter_state(
            &state.ws_clients,
            &*state.presenter_state.read().await,
        )
        .await;
        websocket::broadcast_presentation_status(&state.ws_clients, state).await;
        return Ok(());
    }

    let folder =
        settings::get_json::<SlideFolder>(&state.pool, routes::BIBLE_SLIDE_FOLDER_KEY).await;
    let mut dir = std::path::PathBuf::from(&folder.path);
    if folder.path.is_empty() || !dir.is_dir() {
        dir = std::env::temp_dir().join("metocast-slides");
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Couldn't make a folder for the slides: {e}"))?;
    }
    let path = dir.join(file_name);
    let theme = *state.presenter_theme.read().await;
    presenter::write_pptx(
        &path,
        &deck.slides,
        deck.slide_width_emu,
        deck.slide_height_emu,
        theme,
    )?;
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = state
            .keynote_connector
            .open_file(&path.to_string_lossy())
            .await
        {
            tracing::warn!(%error, "Keynote could not open the deck");
        }
    }
    websocket::broadcast_presentation_status(&state.ws_clients, state).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: OrderItemKind) -> OrderItem {
        OrderItem {
            id: None,
            kind,
            title: "Ének".to_string(),
            deck_path: None,
            bible_type: None,
            reference: None,
            translation: None,
            notes: String::new(),
        }
    }

    #[test]
    fn steps_need_what_they_show() {
        assert!(invalid(&[item(OrderItemKind::Block)]).is_none());
        // "13. Záró énekvers": a song with a title and no deck.
        assert!(invalid(&[item(OrderItemKind::Song)]).is_none());
        let mut nameless = item(OrderItemKind::Song);
        nameless.title = String::new();
        assert_eq!(
            invalid(&[item(OrderItemKind::Block), nameless]).as_deref(),
            Some("Step 2 needs a title or a deck")
        );
        let mut reading = item(OrderItemKind::Bible);
        assert!(invalid(std::slice::from_ref(&reading)).is_some());
        reading.bible_type = Some("textus".to_string());
        assert!(invalid(&[reading]).is_none());
        let mut untitled = item(OrderItemKind::Block);
        untitled.title = " ".to_string();
        assert!(invalid(&[untitled]).is_some());
    }

    fn readings(items: &[OrderItem]) -> Vec<(usize, &str)> {
        items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| item.bible_type.as_deref().map(|t| (i, t)))
            .collect()
    }

    #[test]
    fn a_bible_step_reads_the_event_or_a_reference_of_its_own() {
        let mut own = item(OrderItemKind::Bible);
        assert!(invalid(std::slice::from_ref(&own)).is_some());
        own.reference = Some(" Zsolt 23 ".to_string());
        assert!(invalid(std::slice::from_ref(&own)).is_none());
        // RUF (v2) unless it says otherwise.
        assert_eq!(own_reading(&own), Some(("Zsolt 23", "RUF_v2")));
        own.translation = Some("KG".to_string());
        assert_eq!(own_reading(&own), Some(("Zsolt 23", "KG")));
        // The event's Textus wins over a reference left on the step.
        own.bible_type = Some("textus".to_string());
        assert_eq!(own_reading(&own), None);
    }

    #[test]
    fn template_is_valid_and_has_both_readings() {
        let template = template();
        assert!(invalid(&template).is_none());
        assert_eq!(readings(&template), [(3, "leckio"), (5, "textus")]);
        assert!(with_readings(template, &["leckio".into(), "textus".into()]).is_none());
    }

    #[test]
    fn a_missing_reading_goes_next_to_the_other() {
        let types = ["leckio".to_string(), "textus".to_string()];
        let mut lekcio = item(OrderItemKind::Bible);
        lekcio.bible_type = Some("leckio".to_string());
        let list = vec![
            item(OrderItemKind::Block),
            lekcio.clone(),
            item(OrderItemKind::Song),
        ];
        let added = with_readings(list, &types[1..]).expect("textus added");
        assert_eq!(readings(&added), [(1, "leckio"), (2, "textus")]);

        let mut textus = lekcio;
        textus.bible_type = Some("textus".to_string());
        let added = with_readings(vec![item(OrderItemKind::Block), textus], &types[..1])
            .expect("lekció added");
        assert_eq!(readings(&added), [(1, "leckio"), (2, "textus")]);

        let added = with_readings(vec![item(OrderItemKind::Block)], &types).expect("both added");
        assert_eq!(readings(&added), [(1, "leckio"), (2, "textus")]);
        // A type that isn't a reading adds nothing.
        assert!(with_readings(Vec::new(), &["other".to_string()]).is_none());
    }
}
