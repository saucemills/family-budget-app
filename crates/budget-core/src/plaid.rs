//! Bank syncing through Plaid.
//!
//! The flow, with no server of our own:
//! 1. `link_token_create` with `hosted_link` gives a URL to Plaid's own sign-in page,
//!    which the app opens in the browser. Bank passwords only ever go to Plaid.
//! 2. The app polls `link_token_get` until the person finishes, then exchanges the
//!    public token for an access token (kept in the OS credential store by the app shell,
//!    never in the budget file).
//! 3. `sync_plaid_item` pulls new, changed and removed transactions with
//!    `/transactions/sync`, remembering its cursor so each sync only fetches what's new.

use std::collections::HashMap;
use std::time::Duration;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::db::Budget;
use crate::error::{invalid, Error, Result};
use crate::models::AccountKind;

// ---------- Plaid API types ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaidEnv {
    Sandbox,
    Production,
}

impl PlaidEnv {
    pub fn base_url(self) -> &'static str {
        match self {
            PlaidEnv::Sandbox => "https://sandbox.plaid.com",
            PlaidEnv::Production => "https://production.plaid.com",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaidCredentials {
    pub client_id: String,
    pub secret: String,
    pub env: PlaidEnv,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaidAccount {
    pub account_id: String,
    pub name: String,
    #[serde(default)]
    pub mask: Option<String>,
    #[serde(rename = "type", default)]
    pub account_type: String,
    #[serde(default)]
    pub subtype: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersonalFinanceCategory {
    #[serde(default)]
    pub primary: String,
    #[serde(default)]
    pub detailed: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaidTransaction {
    pub transaction_id: String,
    pub account_id: String,
    /// Plaid's sign: positive is money OUT of the account.
    pub amount: f64,
    /// YYYY-MM-DD
    pub date: String,
    pub name: String,
    #[serde(default)]
    pub merchant_name: Option<String>,
    #[serde(default)]
    pub pending: bool,
    #[serde(default)]
    pub personal_finance_category: Option<PersonalFinanceCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovedTransaction {
    pub transaction_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncPage {
    #[serde(default)]
    pub added: Vec<PlaidTransaction>,
    #[serde(default)]
    pub modified: Vec<PlaidTransaction>,
    #[serde(default)]
    pub removed: Vec<RemovedTransaction>,
    pub next_cursor: String,
    #[serde(default)]
    pub has_more: bool,
}

/// A bank login that finished in Plaid's hosted page and is ready to exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedLink {
    pub public_token: String,
    pub institution_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LinkStatus {
    /// The person hasn't finished yet.
    Waiting,
    /// They closed Plaid's page without connecting.
    Exited,
    Completed { links: Vec<CompletedLink> },
}

/// The calls sync needs. A trait so tests can use a fake Plaid.
pub trait PlaidApi {
    fn accounts(&self, access_token: &str) -> Result<Vec<PlaidAccount>>;
    fn transactions_sync(&self, access_token: &str, cursor: Option<&str>) -> Result<SyncPage>;
}

pub struct PlaidClient {
    creds: PlaidCredentials,
    http: reqwest::blocking::Client,
}

impl PlaidClient {
    pub fn new(creds: PlaidCredentials) -> Result<Self> {
        if creds.client_id.trim().is_empty() || creds.secret.trim().is_empty() {
            return invalid("add your Plaid client ID and secret in Settings first");
        }
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(PlaidClient { creds, http })
    }

    fn post(&self, path: &str, mut body: Value) -> Result<Value> {
        body["client_id"] = json!(self.creds.client_id.trim());
        body["secret"] = json!(self.creds.secret.trim());
        let resp = self
            .http
            .post(format!("{}{}", self.creds.env.base_url(), path))
            .json(&body)
            .send()
            .map_err(|e| Error::Http(e.to_string()))?;
        let ok = resp.status().is_success();
        let value: Value = resp.json().map_err(|e| Error::Http(e.to_string()))?;
        if ok {
            Ok(value)
        } else {
            let code = value["error_code"].as_str().unwrap_or("UNKNOWN").to_string();
            let message = value["display_message"]
                .as_str()
                .or(value["error_message"].as_str())
                .unwrap_or("Plaid returned an error")
                .to_string();
            Err(Error::Plaid { code, message })
        }
    }

    /// Start a connection. Returns (link_token, URL of Plaid's hosted sign-in page).
    pub fn link_token_create(&self, update_access_token: Option<&str>) -> Result<(String, String)> {
        let mut body = json!({
            "client_name": "Family Budget",
            "user": { "client_user_id": "family-budget-local-user" },
            "country_codes": ["US"],
            "language": "en",
            "hosted_link": { "url_lifetime_seconds": 1800 },
        });
        match update_access_token {
            // Update mode: fix a connection whose bank login changed.
            Some(token) => body["access_token"] = json!(token),
            None => {
                body["products"] = json!(["transactions"]);
                body["transactions"] = json!({ "days_requested": 730 });
            }
        }
        let v = self.post("/link/token/create", body)?;
        let token = v["link_token"].as_str().unwrap_or_default().to_string();
        let url = v["hosted_link_url"].as_str().unwrap_or_default().to_string();
        if token.is_empty() || url.is_empty() {
            return invalid("Plaid didn't return a sign-in link");
        }
        Ok((token, url))
    }

    pub fn link_token_get(&self, link_token: &str) -> Result<LinkStatus> {
        let v = self.post("/link/token/get", json!({ "link_token": link_token }))?;
        Ok(parse_link_status(&v))
    }

    /// Returns (access_token, item_id).
    pub fn exchange_public_token(&self, public_token: &str) -> Result<(String, String)> {
        let v = self.post("/item/public_token/exchange", json!({ "public_token": public_token }))?;
        Ok((
            v["access_token"].as_str().unwrap_or_default().to_string(),
            v["item_id"].as_str().unwrap_or_default().to_string(),
        ))
    }

    /// Disconnect a bank at Plaid. Note: on Plaid's Trial plan this doesn't free a slot.
    pub fn item_remove(&self, access_token: &str) -> Result<()> {
        self.post("/item/remove", json!({ "access_token": access_token }))?;
        Ok(())
    }

    /// Sandbox only: make a fake bank connection without the sign-in page (for testing).
    pub fn sandbox_public_token(&self) -> Result<String> {
        let v = self.post(
            "/sandbox/public_token/create",
            json!({ "institution_id": "ins_109508", "initial_products": ["transactions"] }),
        )?;
        Ok(v["public_token"].as_str().unwrap_or_default().to_string())
    }
}

impl PlaidApi for PlaidClient {
    fn accounts(&self, access_token: &str) -> Result<Vec<PlaidAccount>> {
        let v = self.post("/accounts/get", json!({ "access_token": access_token }))?;
        serde_json::from_value(v["accounts"].clone()).map_err(|e| Error::Http(format!("unexpected accounts reply: {e}")))
    }

    fn transactions_sync(&self, access_token: &str, cursor: Option<&str>) -> Result<SyncPage> {
        let mut body = json!({ "access_token": access_token, "count": 500 });
        if let Some(c) = cursor {
            body["cursor"] = json!(c);
        }
        let v = self.post("/transactions/sync", body)?;
        serde_json::from_value(v).map_err(|e| Error::Http(format!("unexpected sync reply: {e}")))
    }
}

pub fn parse_link_status(v: &Value) -> LinkStatus {
    let sessions = v["link_sessions"].as_array().cloned().unwrap_or_default();
    let mut links = Vec::new();
    let mut exited = false;
    for s in &sessions {
        for r in s["results"]["item_add_results"].as_array().into_iter().flatten() {
            if let Some(pt) = r["public_token"].as_str() {
                links.push(CompletedLink {
                    public_token: pt.to_string(),
                    institution_name: r["institution"]["name"].as_str().unwrap_or("Bank").to_string(),
                });
            }
        }
        if !s["exit"].is_null() || (!s["finished_at"].is_null() && s["results"]["item_add_results"].as_array().is_none_or(|a| a.is_empty())) {
            exited = true;
        }
    }
    if !links.is_empty() {
        LinkStatus::Completed { links }
    } else if exited {
        LinkStatus::Exited
    } else {
        LinkStatus::Waiting
    }
}

// ---------- local bookkeeping ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedAccount {
    pub id: i64,
    pub name: String,
    pub plaid_since: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaidItem {
    pub id: i64,
    pub item_id: String,
    pub institution_name: String,
    pub last_synced_at: Option<String>,
    pub last_error: Option<String>,
    pub accounts: Vec<LinkedAccount>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncResult {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// Transactions dated on or before the account's existing history, skipped to avoid duplicates.
    pub skipped_old: usize,
    pub new_accounts: Vec<String>,
    /// Newly added transactions that got a category from a rule or Plaid's guess.
    pub auto_categorized: usize,
}

/// Plaid's spending categories mapped to the starter categories, by name. Used only when
/// no rule matched, and only if a category with that name still exists.
fn pfc_category_name(pfc: &PersonalFinanceCategory) -> Option<&'static str> {
    let d = pfc.detailed.as_str();
    Some(match d {
        "FOOD_AND_DRINK_GROCERIES" => "Groceries",
        "FOOD_AND_DRINK_COFFEE" => "Coffee",
        "FOOD_AND_DRINK_RESTAURANT" | "FOOD_AND_DRINK_FAST_FOOD" => "Restaurants",
        "TRANSPORTATION_GAS" => "Gas",
        "TRANSPORTATION_TAXIS_AND_RIDE_SHARES" | "TRANSPORTATION_PUBLIC_TRANSIT" => "Rideshare / Transit",
        "RENT_AND_UTILITIES_RENT" => "Rent / Mortgage",
        "MEDICAL_PHARMACIES_AND_SUPPLEMENTS" => "Pharmacy",
        "MEDICAL_VETERINARY_SERVICES" => "Vet",
        "GENERAL_MERCHANDISE_PET_SUPPLIES" => "Pet Food & Supplies",
        "ENTERTAINMENT_TV_AND_MOVIES" | "ENTERTAINMENT_MUSIC_AND_AUDIO" => "Subscriptions",
        "TRAVEL_FLIGHTS" => "Flights",
        "TRAVEL_LODGING" => "Lodging",
        "INCOME_WAGES" => "Paycheck",
        _ => match pfc.primary.as_str() {
            "RENT_AND_UTILITIES" => "Utilities",
            "MEDICAL" => "Medical",
            "GENERAL_MERCHANDISE" => "Shopping",
            "INCOME" => "Other Income",
            _ => return None,
        },
    })
}

fn account_kind(a: &PlaidAccount) -> AccountKind {
    match (a.account_type.as_str(), a.subtype.as_deref()) {
        ("credit", _) => AccountKind::CreditCard,
        ("depository", Some("savings" | "money market" | "cd")) => AccountKind::Savings,
        _ => AccountKind::Checking,
    }
}

impl Budget {
    pub fn save_plaid_item(&self, item_id: &str, institution_name: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO plaid_items (item_id, institution_name) VALUES (?1, ?2)
             ON CONFLICT (item_id) DO UPDATE SET institution_name = excluded.institution_name",
            params![item_id, institution_name],
        )?;
        Ok(self.conn.query_row("SELECT id FROM plaid_items WHERE item_id = ?1", [item_id], |r| r.get(0))?)
    }

    pub fn list_plaid_items(&self) -> Result<Vec<PlaidItem>> {
        let mut items: Vec<PlaidItem> = self
            .conn
            .prepare("SELECT id, item_id, institution_name, last_synced_at, last_error FROM plaid_items ORDER BY id")?
            .query_map([], |r| {
                Ok(PlaidItem {
                    id: r.get(0)?,
                    item_id: r.get(1)?,
                    institution_name: r.get(2)?,
                    last_synced_at: r.get(3)?,
                    last_error: r.get(4)?,
                    accounts: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT id, name, plaid_since FROM accounts WHERE plaid_item_id = ?1 AND closed = 0 ORDER BY name",
        )?;
        for item in &mut items {
            item.accounts = stmt
                .query_map([item.id], |r| Ok(LinkedAccount { id: r.get(0)?, name: r.get(1)?, plaid_since: r.get(2)? }))?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(items)
    }

    /// Plaid accounts at this connection that aren't attached to an app account yet.
    pub fn unlinked_plaid_accounts(&self, accounts: &[PlaidAccount]) -> Result<Vec<PlaidAccount>> {
        let mut out = Vec::new();
        for a in accounts {
            let linked: Option<i64> = self
                .conn
                .query_row("SELECT id FROM accounts WHERE plaid_account_id = ?1", [&a.account_id], |r| r.get(0))
                .optional()?;
            if linked.is_none() {
                out.push(a.clone());
            }
        }
        Ok(out)
    }

    /// Attach a bank account to the app: either to an existing account (only bank
    /// transactions after its latest one are imported) or to a new account.
    pub fn link_plaid_account(&self, item_db_id: i64, a: &PlaidAccount, existing: Option<i64>) -> Result<i64> {
        match existing {
            Some(id) => {
                let since: Option<String> =
                    self.conn.query_row("SELECT MAX(date) FROM transactions WHERE account_id = ?1", [id], |r| r.get(0))?;
                let n = self.conn.execute(
                    "UPDATE accounts SET plaid_account_id = ?2, plaid_item_id = ?3, plaid_since = ?4 WHERE id = ?1",
                    params![id, a.account_id, item_db_id, since],
                )?;
                if n == 0 {
                    return Err(Error::NotFound(format!("account {id}")));
                }
                Ok(id)
            }
            None => {
                let institution: String =
                    self.conn.query_row("SELECT institution_name FROM plaid_items WHERE id = ?1", [item_db_id], |r| r.get(0))?;
                let base = match &a.mask {
                    Some(m) if !m.is_empty() => format!("{institution} {} ••{m}", a.name),
                    _ => format!("{institution} {}", a.name),
                };
                // Account names are unique; add a number if needed.
                let mut name = base.clone();
                let mut n = 2;
                while self.conn.query_row("SELECT 1 FROM accounts WHERE name = ?1", [&name], |_| Ok(())).optional()?.is_some() {
                    name = format!("{base} ({n})");
                    n += 1;
                }
                let id = self.create_account(&name, account_kind(a))?;
                self.conn.execute(
                    "UPDATE accounts SET plaid_account_id = ?2, plaid_item_id = ?3 WHERE id = ?1",
                    params![id, a.account_id, item_db_id],
                )?;
                Ok(id)
            }
        }
    }

    pub fn set_plaid_item_error(&self, item_db_id: i64, message: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE plaid_items SET last_error = ?2 WHERE id = ?1", params![item_db_id, message])?;
        Ok(())
    }

    /// Forget a bank connection. Its accounts and transactions stay; they just stop syncing.
    pub fn delete_plaid_item(&self, item_db_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET plaid_account_id = NULL, plaid_item_id = NULL, plaid_since = NULL WHERE plaid_item_id = ?1",
            [item_db_id],
        )?;
        self.conn.execute("DELETE FROM plaid_items WHERE id = ?1", [item_db_id])?;
        Ok(())
    }

    pub fn plaid_item_id(&self, item_db_id: i64) -> Result<String> {
        self.conn
            .query_row("SELECT item_id FROM plaid_items WHERE id = ?1", [item_db_id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("bank connection {item_db_id}")))
    }

    /// Pull everything new from the bank since the last sync.
    pub fn sync_plaid_item(&mut self, item_db_id: i64, access_token: &str, api: &dyn PlaidApi) -> Result<SyncResult> {
        let mut result = SyncResult::default();

        // New accounts opened at the bank show up as new app accounts.
        let accounts = api.accounts(access_token)?;
        for a in self.unlinked_plaid_accounts(&accounts)? {
            let id = self.link_plaid_account(item_db_id, &a, None)?;
            let name: String = self.conn.query_row("SELECT name FROM accounts WHERE id = ?1", [id], |r| r.get(0))?;
            result.new_accounts.push(name);
        }

        // Fetch all pages first; if Plaid's data changes mid-way, start over (Plaid's documented recovery).
        let start_cursor: Option<String> =
            self.conn.query_row("SELECT cursor FROM plaid_items WHERE id = ?1", [item_db_id], |r| r.get(0))?;
        let mut attempts = 0;
        let (pages, final_cursor) = loop {
            attempts += 1;
            let mut pages = Vec::new();
            let mut cursor = start_cursor.clone();
            let outcome: Result<()> = (|| {
                loop {
                    let page = api.transactions_sync(access_token, cursor.as_deref())?;
                    cursor = Some(page.next_cursor.clone());
                    let more = page.has_more;
                    pages.push(page);
                    if !more {
                        return Ok(());
                    }
                }
            })();
            match outcome {
                Ok(()) => break (pages, cursor),
                Err(Error::Plaid { code, .. }) if code == "TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION" && attempts < 3 => continue,
                Err(e) => return Err(e),
            }
        };

        // Which app account each bank account maps to, and its cut-off date.
        let mut map: HashMap<String, (i64, Option<String>)> = HashMap::new();
        {
            let mut stmt = self.conn.prepare("SELECT plaid_account_id, id, plaid_since FROM accounts WHERE plaid_item_id = ?1")?;
            for row in stmt.query_map([item_db_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?)))? {
                let (pid, id, since) = row?;
                map.insert(pid, (id, since));
            }
        }

        let mut pfc_guesses: Vec<(i64, &'static str)> = Vec::new();
        let tx = self.conn.transaction()?;
        {
            let mut find = tx.prepare("SELECT id FROM transactions WHERE account_id = ?1 AND import_id = ?2")?;
            let mut insert = tx.prepare(
                "INSERT INTO transactions (account_id, date, payee, amount_cents, source, import_id)
                 VALUES (?1, ?2, ?3, ?4, 'plaid', ?5)",
            )?;
            let mut update = tx.prepare("UPDATE transactions SET date = ?2, payee = ?3, amount_cents = ?4 WHERE id = ?1")?;
            let mut remove = tx.prepare("DELETE FROM transactions WHERE source = 'plaid' AND import_id = ?1")?;

            for page in &pages {
                for t in page.added.iter().chain(page.modified.iter()) {
                    // Pending charges change or vanish; take them once they post.
                    if t.pending {
                        continue;
                    }
                    let Some((account_id, since)) = map.get(&t.account_id) else { continue };
                    if since.as_deref().is_some_and(|s| t.date.as_str() <= s) {
                        result.skipped_old += 1;
                        continue;
                    }
                    let import_id = format!("plaid:{}", t.transaction_id);
                    let payee = t.merchant_name.clone().filter(|m| !m.is_empty()).unwrap_or_else(|| t.name.clone());
                    let cents = -(t.amount * 100.0).round() as i64;
                    let existing: Option<i64> = find.query_row(params![account_id, import_id], |r| r.get(0)).optional()?;
                    match existing {
                        Some(id) => {
                            update.execute(params![id, t.date, payee, cents])?;
                            result.updated += 1;
                        }
                        None => {
                            insert.execute(params![account_id, t.date, payee, cents, import_id])?;
                            result.added += 1;
                            if let Some(name) = t.personal_finance_category.as_ref().and_then(pfc_category_name) {
                                pfc_guesses.push((tx.last_insert_rowid(), name));
                            }
                        }
                    }
                }
                for r in &page.removed {
                    result.removed += remove.execute([format!("plaid:{}", r.transaction_id)])?;
                }
            }
            tx.execute(
                "UPDATE plaid_items SET cursor = ?2, last_synced_at = datetime('now'), last_error = NULL WHERE id = ?1",
                params![item_db_id, final_cursor],
            )?;
        }
        tx.commit()?;

        // Rules first; Plaid's guess only fills in what rules didn't.
        result.auto_categorized += self.apply_rules()?;
        for (id, name) in pfc_guesses {
            result.auto_categorized += self.conn.execute(
                "UPDATE transactions SET category_id = (SELECT id FROM categories WHERE name = ?2 AND hidden = 0 LIMIT 1)
                 WHERE id = ?1 AND category_id IS NULL AND EXISTS (SELECT 1 FROM categories WHERE name = ?2 AND hidden = 0)",
                params![id, name],
            )?;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::models::{NewTransaction, TransactionFilter};

    /// A pretend Plaid that serves pages keyed by the cursor it's asked for.
    struct FakePlaid {
        accounts: Vec<PlaidAccount>,
        pages: HashMap<Option<String>, SyncPage>,
        fail_once_with: RefCell<Option<String>>,
    }

    impl PlaidApi for FakePlaid {
        fn accounts(&self, _: &str) -> Result<Vec<PlaidAccount>> {
            Ok(self.accounts.clone())
        }
        fn transactions_sync(&self, _: &str, cursor: Option<&str>) -> Result<SyncPage> {
            if cursor.is_some() {
                if let Some(code) = self.fail_once_with.borrow_mut().take() {
                    return Err(Error::Plaid { code, message: "changed".into() });
                }
            }
            Ok(self.pages.get(&cursor.map(String::from)).cloned().unwrap_or(SyncPage {
                next_cursor: cursor.unwrap_or_default().to_string(),
                ..Default::default()
            }))
        }
    }

    fn acct(id: &str, name: &str, ty: &str, sub: &str, mask: &str) -> PlaidAccount {
        PlaidAccount {
            account_id: id.into(),
            name: name.into(),
            mask: Some(mask.into()),
            account_type: ty.into(),
            subtype: Some(sub.into()),
        }
    }

    fn txn(id: &str, acct: &str, amount: f64, date: &str, name: &str, pfc: Option<&str>) -> PlaidTransaction {
        PlaidTransaction {
            transaction_id: id.into(),
            account_id: acct.into(),
            amount,
            date: date.into(),
            name: name.into(),
            merchant_name: None,
            pending: false,
            personal_finance_category: pfc.map(|d| PersonalFinanceCategory {
                primary: d.split('_').next().unwrap().into(),
                detailed: d.into(),
            }),
        }
    }

    #[test]
    fn first_sync_then_incremental_changes() {
        let mut b = Budget::open_in_memory().unwrap();
        let item = b.save_plaid_item("item-1", "Chase").unwrap();
        let mut pages = HashMap::new();
        pages.insert(
            None,
            SyncPage {
                added: vec![
                    txn("t1", "chk", 12.5, "2026-11-01", "STARBUCKS #12", Some("FOOD_AND_DRINK_COFFEE")),
                    txn("t2", "chk", -3000.0, "2026-11-01", "ACME PAYROLL", Some("INCOME_WAGES")),
                ],
                next_cursor: "c1".into(),
                has_more: true,
                ..Default::default()
            },
        );
        pages.insert(
            Some("c1".into()),
            SyncPage {
                added: vec![
                    txn("t3", "card", 40.0, "2026-11-02", "SHELL OIL 123", Some("TRANSPORTATION_GAS")),
                    PlaidTransaction { pending: true, ..txn("t4", "card", 9.0, "2026-11-03", "PENDING THING", None) },
                ],
                next_cursor: "c2".into(),
                ..Default::default()
            },
        );
        pages.insert(
            Some("c2".into()),
            SyncPage {
                modified: vec![txn("t3", "card", 41.25, "2026-11-02", "SHELL OIL 123", None)],
                removed: vec![RemovedTransaction { transaction_id: "t1".into() }],
                next_cursor: "c3".into(),
                ..Default::default()
            },
        );
        let fake = FakePlaid {
            accounts: vec![acct("chk", "Checking", "depository", "checking", "1111"), acct("card", "Sapphire", "credit", "credit card", "9999")],
            pages,
            fail_once_with: RefCell::new(None),
        };

        let r = b.sync_plaid_item(item, "tok", &fake).unwrap();
        assert_eq!((r.added, r.updated, r.removed), (3, 0, 0));
        assert_eq!(r.new_accounts, vec!["Chase Checking ••1111", "Chase Sapphire ••9999"]);
        assert_eq!(r.auto_categorized, 3);
        let accounts = b.list_accounts().unwrap();
        let card = accounts.iter().find(|a| a.name.contains("Sapphire")).unwrap();
        assert_eq!(card.kind, AccountKind::CreditCard);
        assert_eq!(card.balance_cents, -4000);
        let txns = b.list_transactions(&TransactionFilter::default()).unwrap();
        assert_eq!(txns.iter().find(|t| t.payee == "ACME PAYROLL").unwrap().category_name.as_deref(), Some("Paycheck"));

        // Second sync continues from c2: one change and one removal.
        let r = b.sync_plaid_item(item, "tok", &fake).unwrap();
        assert_eq!((r.added, r.updated, r.removed), (0, 1, 1));
        let txns = b.list_transactions(&TransactionFilter::default()).unwrap();
        assert_eq!(txns.len(), 2);
        let gas = txns.iter().find(|t| t.payee == "SHELL OIL 123").unwrap();
        assert_eq!((gas.amount_cents, gas.category_name.as_deref()), (-4125, Some("Gas"))); // category kept on update

        // Nothing new: no changes, no new accounts.
        let r = b.sync_plaid_item(item, "tok", &fake).unwrap();
        assert_eq!((r.added, r.updated, r.removed, r.new_accounts.len()), (0, 0, 0, 0));
        assert!(b.list_plaid_items().unwrap()[0].last_synced_at.is_some());
    }

    #[test]
    fn linking_existing_account_skips_old_history_and_rules_beat_plaid() {
        let mut b = Budget::open_in_memory().unwrap();
        let existing = b.create_account("Joint Checking", AccountKind::Checking).unwrap();
        b.add_transaction(&NewTransaction {
            account_id: existing,
            date: "2026-11-05".into(),
            payee: "from csv".into(),
            amount_cents: -100,
            ..Default::default()
        })
        .unwrap();
        let restaurants = b.list_category_groups().unwrap().into_iter().flat_map(|g| g.categories).find(|c| c.name == "Restaurants").unwrap().id;
        b.add_rule("STARBUCKS", restaurants).unwrap();

        let item = b.save_plaid_item("item-2", "Wells Fargo").unwrap();
        let chk = acct("chk", "Everyday Checking", "depository", "checking", "2222");
        b.link_plaid_account(item, &chk, Some(existing)).unwrap();

        let mut pages = HashMap::new();
        pages.insert(
            None,
            SyncPage {
                added: vec![
                    txn("old", "chk", 1.0, "2026-11-05", "ALREADY IN CSV", None),
                    txn("new", "chk", 5.0, "2026-11-06", "STARBUCKS #1", Some("FOOD_AND_DRINK_COFFEE")),
                ],
                next_cursor: "c1".into(),
                ..Default::default()
            },
        );
        let fake = FakePlaid { accounts: vec![chk], pages, fail_once_with: RefCell::new(None) };
        let r = b.sync_plaid_item(item, "tok", &fake).unwrap();
        assert_eq!((r.added, r.skipped_old, r.new_accounts.len()), (1, 1, 0));
        let t = b.list_transactions(&TransactionFilter { search: Some("STARBUCKS".into()), ..Default::default() }).unwrap();
        assert_eq!(t[0].category_name.as_deref(), Some("Restaurants"));
        assert_eq!(b.list_plaid_items().unwrap()[0].accounts[0].plaid_since.as_deref(), Some("2026-11-05"));
    }

    #[test]
    fn restarts_when_plaid_data_changes_mid_sync() {
        let mut b = Budget::open_in_memory().unwrap();
        let item = b.save_plaid_item("item-3", "Amex").unwrap();
        let mut pages = HashMap::new();
        pages.insert(None, SyncPage { added: vec![txn("a", "x", 1.0, "2026-11-01", "A", None)], next_cursor: "c1".into(), has_more: true, ..Default::default() });
        pages.insert(Some("c1".into()), SyncPage { added: vec![txn("b", "x", 2.0, "2026-11-02", "B", None)], next_cursor: "c2".into(), ..Default::default() });
        let fake = FakePlaid {
            accounts: vec![acct("x", "Card", "credit", "credit card", "0001")],
            pages,
            fail_once_with: RefCell::new(Some("TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION".into())),
        };
        let r = b.sync_plaid_item(item, "tok", &fake).unwrap();
        assert_eq!(r.added, 2); // not 3: the first page wasn't applied twice
    }

    #[test]
    fn disconnect_keeps_history() {
        let mut b = Budget::open_in_memory().unwrap();
        let item = b.save_plaid_item("item-4", "Citi").unwrap();
        let mut pages = HashMap::new();
        pages.insert(None, SyncPage { added: vec![txn("a", "x", 1.0, "2026-11-01", "A", None)], next_cursor: "c1".into(), ..Default::default() });
        let fake = FakePlaid { accounts: vec![acct("x", "Card", "credit", "credit card", "0001")], pages, fail_once_with: RefCell::new(None) };
        b.sync_plaid_item(item, "tok", &fake).unwrap();
        b.delete_plaid_item(item).unwrap();
        assert!(b.list_plaid_items().unwrap().is_empty());
        assert_eq!(b.list_accounts().unwrap()[0].transaction_count, 1);
    }

    #[test]
    fn parses_hosted_link_results() {
        let waiting = json!({ "link_sessions": [] });
        assert!(matches!(parse_link_status(&waiting), LinkStatus::Waiting));
        let done = json!({ "link_sessions": [{ "finished_at": "x", "results": { "item_add_results": [
            { "public_token": "public-sandbox-1", "institution": { "name": "First Platypus Bank" } } ] } }] });
        match parse_link_status(&done) {
            LinkStatus::Completed { links } => assert_eq!(links[0].institution_name, "First Platypus Bank"),
            other => panic!("{other:?}"),
        }
        let exited = json!({ "link_sessions": [{ "finished_at": "x", "exit": { "status": "requires_credentials" }, "results": {} }] });
        assert!(matches!(parse_link_status(&exited), LinkStatus::Exited));
    }
}
