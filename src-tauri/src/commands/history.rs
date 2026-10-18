use crate::store::{new_id, now_iso, AppState, FavoriteRecord, HistoryRecord};
use tauri::State;

// ---------- history.rs 语义（含收藏，原版无独立 favorites 模块） ----------

#[tauri::command]
pub fn add_history(
    state: State<'_, AppState>,
    kind: String,
    query: String,
) -> Result<HistoryRecord, String> {
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    let record = HistoryRecord {
        id: new_id(),
        kind,
        query,
        timestamp: now_iso(),
    };
    data.history.insert(0, record.clone());
    drop(data);
    state.commit()?;
    Ok(record)
}

#[tauri::command]
pub fn list_history(state: State<'_, AppState>) -> Result<Vec<HistoryRecord>, String> {
    let data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    Ok(data.history.clone())
}

#[tauri::command]
pub fn delete_history(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    data.history.retain(|r| r.id != id);
    drop(data);
    state.commit()
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    data.history.clear();
    drop(data);
    state.commit()
}

#[tauri::command]
pub fn add_favorite(
    state: State<'_, AppState>,
    kind: String,
    path: String,
) -> Result<FavoriteRecord, String> {
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    if let Some(existing) = data
        .favorites
        .iter()
        .find(|f| f.kind == kind && f.path == path)
    {
        return Ok(existing.clone());
    }
    let record = FavoriteRecord {
        id: new_id(),
        kind,
        path,
        created_at: now_iso(),
    };
    data.favorites.push(record.clone());
    drop(data);
    state.commit()?;
    Ok(record)
}

#[tauri::command]
pub fn list_favorites(state: State<'_, AppState>) -> Result<Vec<FavoriteRecord>, String> {
    let data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    Ok(data.favorites.clone())
}

#[tauri::command]
pub fn delete_favorite(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    data.favorites.retain(|f| f.id != id);
    drop(data);
    state.commit()
}
