//! Bank syncing commands. Plaid keys and per-bank access tokens live in the operating
//! system's credential store (Windows Credential Manager), never in the budget file.

use budget_core::plaid::{LinkStatus, PlaidAccount, PlaidClient, PlaidCredentials, PlaidEnv, PlaidItem, SyncResult};
use budget_core::rules::{suggest_match_text, Rule};
use budget_core::Budget;
use serde::{Deserialize, Serialize};
use tauri::State;
use tauri_plugin_opener::OpenerExt;

use crate::{secrets, AppState};

const CREDS_USER: &str = "plaid-credentials";

type CmdResult<T> = Result<T, String>;

fn load_creds() -> CmdResult<Option<PlaidCredentials>> {
    match secrets::get(CREDS_USER)? {
        Some(s) => serde_json::from_str(&s).map(Some).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

fn client() -> CmdResult<PlaidClient> {
    let creds = load_creds()?.ok_or("Add your Plaid keys in Settings first.")?;
    PlaidClient::new(creds).map_err(|e| e.to_string())
}

fn token_user(item_id: &str) -> String {
    format!("plaid-token-{item_id}")
}

fn access_token(item_id: &str) -> CmdResult<String> {
    secrets::get(&token_user(item_id))?
        .ok_or_else(|| "This bank's sign-in is missing on this computer. Disconnect it and connect again.".to_string())
}

/// Run blocking work (network + database) off the UI thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

fn lock(state: &std::sync::Arc<std::sync::Mutex<Budget>>) -> CmdResult<std::sync::MutexGuard<'_, Budget>> {
    state.lock().map_err(|_| "the database is busy, try again".to_string())
}

// ---------- settings ----------

#[derive(Serialize)]
pub struct PlaidSettings {
    configured: bool,
    env: Option<PlaidEnv>,
    /// Last 4 characters of the client ID, so the person can tell which keys are saved.
    client_id_hint: Option<String>,
}

#[tauri::command]
pub fn plaid_get_settings() -> CmdResult<PlaidSettings> {
    Ok(match load_creds()? {
        Some(c) => PlaidSettings {
            configured: true,
            env: Some(c.env),
            client_id_hint: Some(c.client_id.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect()),
        },
        None => PlaidSettings { configured: false, env: None, client_id_hint: None },
    })
}

#[tauri::command]
pub fn plaid_save_settings(client_id: String, secret: String, env: PlaidEnv) -> CmdResult<()> {
    let creds = PlaidCredentials { client_id: client_id.trim().into(), secret: secret.trim().into(), env };
    if creds.client_id.is_empty() || creds.secret.is_empty() {
        return Err("Both the client ID and the secret are required.".into());
    }
    secrets::set(CREDS_USER, &serde_json::to_string(&creds).map_err(|e| e.to_string())?)
}

// ---------- connections ----------

#[tauri::command]
pub fn plaid_list_items(state: State<AppState>) -> CmdResult<Vec<PlaidItem>> {
    lock(&state.0)?.list_plaid_items().map_err(|e| e.to_string())
}

/// Open Plaid's sign-in page in the browser. Pass `item` to repair an existing connection.
#[tauri::command]
pub async fn plaid_connect_start(app: tauri::AppHandle, state: State<'_, AppState>, item: Option<i64>) -> CmdResult<String> {
    let budget = state.0.clone();
    let (token, url) = blocking(move || {
        let update_token = match item {
            Some(id) => Some(access_token(&lock(&budget)?.plaid_item_id(id).map_err(|e| e.to_string())?)?),
            None => None,
        };
        client()?.link_token_create(update_token.as_deref()).map_err(|e| e.to_string())
    })
    .await?;
    app.opener().open_url(url, None::<&str>).map_err(|e| format!("couldn't open the browser: {e}"))?;
    Ok(token)
}

#[derive(Serialize)]
pub struct NewConnection {
    item: i64,
    institution_name: String,
    /// Bank accounts the person should match to app accounts (or let us create).
    accounts: Vec<PlaidAccount>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PollResult {
    Waiting,
    Exited,
    Completed { connections: Vec<NewConnection> },
}

fn finish_connection(budget: &std::sync::Arc<std::sync::Mutex<Budget>>, plaid: &PlaidClient, public_token: &str, institution: &str) -> CmdResult<NewConnection> {
    use budget_core::plaid::PlaidApi;
    let (token, item_id) = plaid.exchange_public_token(public_token).map_err(|e| e.to_string())?;
    secrets::set(&token_user(&item_id), &token)?;
    let accounts = plaid.accounts(&token).map_err(|e| e.to_string())?;
    let b = lock(budget)?;
    let item = b.save_plaid_item(&item_id, institution).map_err(|e| e.to_string())?;
    let accounts = b.unlinked_plaid_accounts(&accounts).map_err(|e| e.to_string())?;
    Ok(NewConnection { item, institution_name: institution.into(), accounts })
}

#[tauri::command]
pub async fn plaid_connect_poll(state: State<'_, AppState>, link_token: String) -> CmdResult<PollResult> {
    let budget = state.0.clone();
    blocking(move || {
        let plaid = client()?;
        Ok(match plaid.link_token_get(&link_token).map_err(|e| e.to_string())? {
            LinkStatus::Waiting => PollResult::Waiting,
            LinkStatus::Exited => PollResult::Exited,
            LinkStatus::Completed { links } => {
                let mut connections = Vec::new();
                for l in links {
                    connections.push(finish_connection(&budget, &plaid, &l.public_token, &l.institution_name)?);
                }
                PollResult::Completed { connections }
            }
        })
    })
    .await
}

/// Sandbox only: connect Plaid's fake test bank without the sign-in page.
#[tauri::command]
pub async fn plaid_sandbox_connect(state: State<'_, AppState>) -> CmdResult<NewConnection> {
    let budget = state.0.clone();
    blocking(move || {
        if load_creds()?.map(|c| c.env) != Some(PlaidEnv::Sandbox) {
            return Err("The test bank is only available with Sandbox keys.".into());
        }
        let plaid = client()?;
        let public = plaid.sandbox_public_token().map_err(|e| e.to_string())?;
        finish_connection(&budget, &plaid, &public, "Platypus Test Bank")
    })
    .await
}

#[derive(Deserialize)]
pub struct AccountChoice {
    account: PlaidAccount,
    /// An existing app account to attach to, or none to create a new one.
    existing_account_id: Option<i64>,
}

#[tauri::command]
pub fn plaid_link_accounts(state: State<AppState>, item: i64, choices: Vec<AccountChoice>) -> CmdResult<()> {
    let b = lock(&state.0)?;
    for c in choices {
        b.link_plaid_account(item, &c.account, c.existing_account_id).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct ItemSync {
    item: i64,
    institution_name: String,
    result: Option<SyncResult>,
    error: Option<String>,
    /// True when the bank needs the person to sign in again (password changed, etc.).
    needs_reconnect: bool,
}

/// Sync one connection, or all of them when `item` is None.
#[tauri::command]
pub async fn plaid_sync(state: State<'_, AppState>, item: Option<i64>) -> CmdResult<Vec<ItemSync>> {
    let budget = state.0.clone();
    blocking(move || {
        let plaid = client()?;
        let items = lock(&budget)?.list_plaid_items().map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for it in items.into_iter().filter(|i| item.is_none_or(|id| id == i.id)) {
            let outcome = access_token(&it.item_id).and_then(|tok| {
                lock(&budget)?.sync_plaid_item(it.id, &tok, &plaid).map_err(|e| match e {
                    budget_core::Error::Plaid { code, message } if code == "ITEM_LOGIN_REQUIRED" => format!("RECONNECT:{message}"),
                    other => other.to_string(),
                })
            });
            let (result, error) = match outcome {
                Ok(r) => (Some(r), None),
                Err(e) => (None, Some(e)),
            };
            let needs_reconnect = error.as_deref().is_some_and(|e| e.starts_with("RECONNECT:"));
            let error = error.map(|e| e.trim_start_matches("RECONNECT:").to_string());
            lock(&budget)?.set_plaid_item_error(it.id, error.as_deref()).map_err(|e| e.to_string())?;
            out.push(ItemSync { item: it.id, institution_name: it.institution_name, result, error, needs_reconnect });
        }
        Ok(out)
    })
    .await
}

#[tauri::command]
pub async fn plaid_disconnect(state: State<'_, AppState>, item: i64) -> CmdResult<()> {
    let budget = state.0.clone();
    blocking(move || {
        let item_id = lock(&budget)?.plaid_item_id(item).map_err(|e| e.to_string())?;
        if let (Ok(tok), Ok(plaid)) = (access_token(&item_id), client()) {
            // Best effort: tell Plaid to stop. Local cleanup happens either way.
            let _ = plaid.item_remove(&tok);
        }
        let _ = secrets::delete(&token_user(&item_id));
        lock(&budget)?.delete_plaid_item(item).map_err(|e| e.to_string())
    })
    .await
}

// ---------- rules ----------

#[tauri::command]
pub fn list_rules(state: State<AppState>) -> CmdResult<Vec<Rule>> {
    lock(&state.0)?.list_rules().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_rule(state: State<AppState>, match_text: String, category_id: i64) -> CmdResult<usize> {
    lock(&state.0)?.add_rule(&match_text, category_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_rule(state: State<AppState>, id: i64) -> CmdResult<()> {
    lock(&state.0)?.delete_rule(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn suggest_rule_text(payee: String) -> String {
    suggest_match_text(&payee)
}
