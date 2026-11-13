//! The budget database: one SQLite file on the family's computer.

use std::path::Path;

use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{invalid, Error, Result};
use crate::models::*;

/// Schema migrations, applied in order. `PRAGMA user_version` records how many have run.
/// Never edit a migration that has shipped; add a new one instead.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE accounts (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    kind        TEXT NOT NULL,
    closed      INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE category_groups (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    is_income   INTEGER NOT NULL DEFAULT 0,
    sort_order  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE categories (
    id          INTEGER PRIMARY KEY,
    group_id    INTEGER NOT NULL REFERENCES category_groups(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    hidden      INTEGER NOT NULL DEFAULT 0,
    UNIQUE (group_id, name)
);

CREATE TABLE transactions (
    id           INTEGER PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    date         TEXT NOT NULL,              -- YYYY-MM-DD
    payee        TEXT NOT NULL DEFAULT '',
    memo         TEXT NOT NULL DEFAULT '',
    amount_cents INTEGER NOT NULL,           -- negative = outflow
    category_id  INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    source       TEXT NOT NULL DEFAULT 'manual',  -- manual | csv | plaid
    import_id    TEXT,                       -- stable id used to skip duplicates on re-import
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (account_id, import_id)
);
CREATE INDEX transactions_date ON transactions(date);
CREATE INDEX transactions_category ON transactions(category_id);
"#];

/// Starter categories, loosely shaped like a typical family's spending.
/// Everything here can be renamed or removed later.
const DEFAULT_CATEGORIES: &[(&str, bool, &[&str])] = &[
    ("Income", true, &["Paycheck", "Other Income"]),
    ("Housing", false, &["Rent / Mortgage", "Utilities", "Home Maintenance"]),
    ("Food", false, &["Groceries", "Restaurants", "Coffee"]),
    ("Transportation", false, &["Gas", "Car Maintenance", "Rideshare / Transit"]),
    ("Health", false, &["Medical", "Pharmacy"]),
    ("Entertainment", false, &["Subscriptions", "Fun Money"]),
    ("Travel", false, &["Flights", "Lodging"]),
    ("Pets", false, &["Pet Food & Supplies", "Vet"]),
    ("Giving", false, &["Gifts", "Charity"]),
    ("Savings & Investing", false, &["Emergency Fund", "Investments"]),
    ("Miscellaneous", false, &["Shopping", "Other"]),
];

pub struct Budget {
    pub(crate) conn: Connection,
}

impl Budget {
    /// Open (creating if needed) the budget file at `path` and bring its schema up to date.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// An in-memory budget, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let mut budget = Budget { conn };
        budget.migrate()?;
        Ok(budget)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let fresh = version == 0;
        let tx = self.conn.transaction()?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        }
        if fresh {
            seed_categories(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---------- accounts ----------

    pub fn create_account(&self, name: &str, kind: AccountKind) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            return invalid("account name is required");
        }
        self.conn
            .execute("INSERT INTO accounts (name, kind) VALUES (?1, ?2)", params![name, kind.as_str()])
            .map_err(|e| unique_msg(e, format!("an account named {name:?} already exists")))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.name, a.kind,
                    COALESCE(SUM(t.amount_cents), 0), COUNT(t.id)
             FROM accounts a LEFT JOIN transactions t ON t.account_id = a.id
             WHERE a.closed = 0
             GROUP BY a.id ORDER BY a.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind: String = r.get(2)?;
            Ok(Account {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: AccountKind::parse(&kind).unwrap_or(AccountKind::Checking),
                balance_cents: r.get(3)?,
                transaction_count: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn delete_account(&self, id: i64) -> Result<()> {
        let n = self.conn.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        if n == 0 {
            return Err(Error::NotFound(format!("account {id}")));
        }
        Ok(())
    }

    // ---------- categories ----------

    pub fn list_category_groups(&self) -> Result<Vec<CategoryGroup>> {
        let mut groups: Vec<CategoryGroup> = self
            .conn
            .prepare("SELECT id, name, is_income, sort_order FROM category_groups ORDER BY sort_order, id")?
            .query_map([], |r| {
                Ok(CategoryGroup {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    is_income: r.get(2)?,
                    sort_order: r.get(3)?,
                    categories: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT id, group_id, name, sort_order FROM categories WHERE hidden = 0 ORDER BY sort_order, id",
        )?;
        let cats = stmt.query_map([], |r| {
            Ok(Category { id: r.get(0)?, group_id: r.get(1)?, name: r.get(2)?, sort_order: r.get(3)? })
        })?;
        for cat in cats {
            let cat = cat?;
            if let Some(g) = groups.iter_mut().find(|g| g.id == cat.group_id) {
                g.categories.push(cat);
            }
        }
        Ok(groups)
    }

    pub fn create_category_group(&self, name: &str, is_income: bool) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            return invalid("group name is required");
        }
        self.conn
            .execute(
                "INSERT INTO category_groups (name, is_income, sort_order)
                 VALUES (?1, ?2, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM category_groups))",
                params![name, is_income],
            )
            .map_err(|e| unique_msg(e, format!("a group named {name:?} already exists")))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn create_category(&self, group_id: i64, name: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            return invalid("category name is required");
        }
        self.conn
            .execute(
                "INSERT INTO categories (group_id, name, sort_order)
                 VALUES (?1, ?2, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM categories WHERE group_id = ?1))",
                params![group_id, name],
            )
            .map_err(|e| unique_msg(e, format!("{name:?} already exists in that group")))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_category(&self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return invalid("category name is required");
        }
        let n = self
            .conn
            .execute("UPDATE categories SET name = ?2 WHERE id = ?1", params![id, name])
            .map_err(|e| unique_msg(e, format!("{name:?} already exists in that group")))?;
        if n == 0 {
            return Err(Error::NotFound(format!("category {id}")));
        }
        Ok(())
    }

    /// Hide a category. Its past transactions keep their category so reports stay correct.
    pub fn hide_category(&self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE categories SET hidden = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- transactions ----------

    pub fn add_transaction(&self, t: &NewTransaction) -> Result<i64> {
        validate_date(&t.date)?;
        self.conn.execute(
            "INSERT INTO transactions (account_id, date, payee, memo, amount_cents, category_id, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'manual')",
            params![t.account_id, t.date, t.payee.trim(), t.memo.trim(), t.amount_cents, t.category_id],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_transactions(&self, f: &TransactionFilter) -> Result<Vec<Transaction>> {
        let search = f.search.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| format!("%{s}%"));
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.account_id, a.name, t.date, t.payee, t.memo, t.amount_cents,
                    t.category_id, c.name, t.source
             FROM transactions t
             JOIN accounts a ON a.id = t.account_id
             LEFT JOIN categories c ON c.id = t.category_id
             WHERE (?1 IS NULL OR t.account_id = ?1)
               AND (?2 = 0 OR t.category_id IS NULL)
               AND (?3 IS NULL OR t.payee LIKE ?3 OR t.memo LIKE ?3)
             ORDER BY t.date DESC, t.id DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![f.account_id, f.uncategorized_only, search, f.limit.unwrap_or(500)],
            |r| {
                Ok(Transaction {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    account_name: r.get(2)?,
                    date: r.get(3)?,
                    payee: r.get(4)?,
                    memo: r.get(5)?,
                    amount_cents: r.get(6)?,
                    category_id: r.get(7)?,
                    category_name: r.get(8)?,
                    source: r.get(9)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_transaction_category(&self, id: i64, category_id: Option<i64>) -> Result<()> {
        let n = self
            .conn
            .execute("UPDATE transactions SET category_id = ?2 WHERE id = ?1", params![id, category_id])?;
        if n == 0 {
            return Err(Error::NotFound(format!("transaction {id}")));
        }
        Ok(())
    }

    pub fn delete_transaction(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM transactions WHERE id = ?1", [id])?;
        Ok(())
    }

    pub(crate) fn account_exists(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM accounts WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .is_some())
    }
}

fn seed_categories(conn: &Connection) -> Result<()> {
    for (gi, (group, is_income, cats)) in DEFAULT_CATEGORIES.iter().enumerate() {
        conn.execute(
            "INSERT INTO category_groups (name, is_income, sort_order) VALUES (?1, ?2, ?3)",
            params![group, is_income, gi as i64],
        )?;
        let gid = conn.last_insert_rowid();
        for (ci, cat) in cats.iter().enumerate() {
            conn.execute(
                "INSERT INTO categories (group_id, name, sort_order) VALUES (?1, ?2, ?3)",
                params![gid, cat, ci as i64],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn validate_date(date: &str) -> Result<()> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|_| ())
        .map_err(|_| Error::Invalid(format!("date must be YYYY-MM-DD, got {date:?}")))
}

fn unique_msg(e: rusqlite::Error, msg: String) -> Error {
    match &e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
            Error::Invalid(msg)
        }
        _ => Error::Db(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_budget_has_starter_categories() {
        let b = Budget::open_in_memory().unwrap();
        let groups = b.list_category_groups().unwrap();
        assert_eq!(groups.len(), DEFAULT_CATEGORIES.len());
        assert!(groups[0].is_income);
        assert!(groups.iter().any(|g| g.categories.iter().any(|c| c.name == "Groceries")));
    }

    #[test]
    fn reopening_keeps_data_and_does_not_reseed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget.sqlite");
        {
            let b = Budget::open(&path).unwrap();
            b.create_account("Checking", AccountKind::Checking).unwrap();
        }
        let b = Budget::open(&path).unwrap();
        assert_eq!(b.list_accounts().unwrap().len(), 1);
        assert_eq!(b.list_category_groups().unwrap().len(), DEFAULT_CATEGORIES.len());
    }

    #[test]
    fn balances_and_categorizing() {
        let b = Budget::open_in_memory().unwrap();
        let acct = b.create_account("Checking", AccountKind::Checking).unwrap();
        assert!(b.create_account("checking ", AccountKind::Savings).is_ok()); // different case is allowed
        assert!(matches!(b.create_account("Checking", AccountKind::Cash), Err(Error::Invalid(_))));

        let groceries = b.create_category(b.list_category_groups().unwrap()[2].id, "Farmers Market").unwrap();
        let t1 = b
            .add_transaction(&NewTransaction {
                account_id: acct,
                date: "2026-11-01".into(),
                payee: "Paycheck".into(),
                amount_cents: 250_000,
                ..Default::default()
            })
            .unwrap();
        let t2 = b
            .add_transaction(&NewTransaction {
                account_id: acct,
                date: "2026-11-02".into(),
                payee: "Market".into(),
                amount_cents: -4_250,
                ..Default::default()
            })
            .unwrap();
        b.set_transaction_category(t2, Some(groceries)).unwrap();

        let accounts = b.list_accounts().unwrap();
        let checking = accounts.iter().find(|a| a.id == acct).unwrap();
        assert_eq!(checking.balance_cents, 245_750);

        let uncategorized = b
            .list_transactions(&TransactionFilter { uncategorized_only: true, ..Default::default() })
            .unwrap();
        assert_eq!(uncategorized.iter().map(|t| t.id).collect::<Vec<_>>(), vec![t1]);

        let found = b
            .list_transactions(&TransactionFilter { search: Some("mark".into()), ..Default::default() })
            .unwrap();
        assert_eq!(found[0].category_name.as_deref(), Some("Farmers Market"));
    }

    #[test]
    fn rejects_bad_dates() {
        let b = Budget::open_in_memory().unwrap();
        let acct = b.create_account("Cash", AccountKind::Cash).unwrap();
        let bad = NewTransaction { account_id: acct, date: "11/02/2026".into(), ..Default::default() };
        assert!(b.add_transaction(&bad).is_err());
    }
}
