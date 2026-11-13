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
"#, r#"
-- Plan: how much was assigned to each category in each month.
CREATE TABLE budget_assignments (
    month          TEXT NOT NULL,            -- YYYY-MM
    category_id    INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    assigned_cents INTEGER NOT NULL,
    PRIMARY KEY (month, category_id)
);
"#, r#"
-- Bank connections through Plaid. Access tokens are NOT stored here; the app keeps
-- them in the operating system's credential store, keyed by item_id.
CREATE TABLE plaid_items (
    id               INTEGER PRIMARY KEY,
    item_id          TEXT NOT NULL UNIQUE,
    institution_name TEXT NOT NULL DEFAULT '',
    cursor           TEXT,                     -- /transactions/sync position
    last_synced_at   TEXT,
    last_error       TEXT,
    created_at       TEXT NOT NULL DEFAULT (datetime('now'))
);
ALTER TABLE accounts ADD COLUMN plaid_account_id TEXT;
ALTER TABLE accounts ADD COLUMN plaid_item_id INTEGER REFERENCES plaid_items(id) ON DELETE SET NULL;
-- Only bank transactions after this date are imported, so linking an account that
-- already has CSV history doesn't create duplicates.
ALTER TABLE accounts ADD COLUMN plaid_since TEXT;
CREATE UNIQUE INDEX accounts_plaid_account ON accounts(plaid_account_id);

-- Auto-categorize: a payee containing match_text goes to category_id.
CREATE TABLE category_rules (
    id          INTEGER PRIMARY KEY,
    match_text  TEXT NOT NULL UNIQUE COLLATE NOCASE,
    category_id INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
"#, r#"
-- Transfers between the family's own accounts aren't income or spending.
ALTER TABLE category_groups ADD COLUMN is_transfer INTEGER NOT NULL DEFAULT 0;
INSERT OR IGNORE INTO category_groups (name, is_income, sort_order) VALUES ('Transfers', 0, 1000);
UPDATE category_groups SET is_transfer = 1 WHERE name = 'Transfers';
INSERT OR IGNORE INTO categories (group_id, name, sort_order)
    SELECT g.id, v.name, v.ord FROM category_groups g,
        (SELECT 'Between my accounts' AS name, 0 AS ord
         UNION ALL SELECT 'Credit card payment', 1
         UNION ALL SELECT 'Loan payment', 2
         UNION ALL SELECT 'Paid through another account', 3) v
    WHERE g.name = 'Transfers';

-- The balance the bank reports, for synced accounts.
ALTER TABLE accounts ADD COLUMN bank_balance_cents INTEGER;
ALTER TABLE accounts ADD COLUMN bank_balance_at TEXT;

-- The other half of a transfer, or the purchase a PayPal-style payment paid for.
ALTER TABLE transactions ADD COLUMN linked_transaction_id INTEGER REFERENCES transactions(id) ON DELETE SET NULL;
-- Plaid's own category guess (personal_finance_category.detailed).
ALTER TABLE transactions ADD COLUMN plaid_category TEXT;
-- 1 when the app chose the category (rule, transfer match, Plaid's guess) rather than a person.
-- The app may re-categorize these; it never touches ones a person picked.
ALTER TABLE transactions ADD COLUMN category_auto INTEGER NOT NULL DEFAULT 0;
UPDATE transactions SET category_auto = 1 WHERE source = 'plaid' AND category_id IS NOT NULL;
-- Fetch every bank transaction once more so Plaid's categories get stored.
UPDATE plaid_items SET cursor = NULL;

-- Rules can be limited to an amount range ("TESLA" from $120 to $250 is insurance), so the
-- same payee text can appear in several rules. Amounts are positive cents, either direction.
CREATE TABLE category_rules_v2 (
    id          INTEGER PRIMARY KEY,
    match_text  TEXT NOT NULL COLLATE NOCASE,
    category_id INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    min_cents   INTEGER,
    max_cents   INTEGER,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO category_rules_v2 (id, match_text, category_id, created_at)
    SELECT id, match_text, category_id, created_at FROM category_rules;
DROP TABLE category_rules;
ALTER TABLE category_rules_v2 RENAME TO category_rules;

-- Insurance shows up in real budgets; older budgets get it too.
INSERT OR IGNORE INTO categories (group_id, name, sort_order)
    SELECT id, 'Car Insurance', 3 FROM category_groups WHERE name = 'Transportation';
"#];

/// Starter categories, loosely shaped like a typical family's spending.
/// Everything here can be renamed or removed later.
const DEFAULT_CATEGORIES: &[(&str, bool, &[&str])] = &[
    ("Income", true, &["Paycheck", "Other Income"]),
    ("Housing", false, &["Rent / Mortgage", "Utilities", "Home Maintenance"]),
    ("Food", false, &["Groceries", "Restaurants", "Coffee"]),
    ("Transportation", false, &["Gas", "Car Maintenance", "Rideshare / Transit", "Car Insurance"]),
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
        budget.detect_transfers()?;
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
        if version < 4 {
            tidy_bank_account_names(&tx)?;
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
                    COALESCE(a.bank_balance_cents, SUM(t.amount_cents), 0), COUNT(t.id),
                    a.bank_balance_cents IS NOT NULL, a.bank_balance_at
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
                balance_from_bank: r.get(5)?,
                balance_as_of: r.get(6)?,
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
            .prepare("SELECT id, name, is_income, is_transfer, sort_order FROM category_groups ORDER BY sort_order, id")?
            .query_map([], |r| {
                Ok(CategoryGroup {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    is_income: r.get(2)?,
                    is_transfer: r.get(3)?,
                    sort_order: r.get(4)?,
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
                    t.category_id, c.name, t.source, t.linked_transaction_id, l.payee, la.name
             FROM transactions t
             JOIN accounts a ON a.id = t.account_id
             LEFT JOIN categories c ON c.id = t.category_id
             LEFT JOIN transactions l ON l.id = t.linked_transaction_id
             LEFT JOIN accounts la ON la.id = l.account_id
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
                    linked_transaction_id: r.get(10)?,
                    linked_payee: r.get(11)?,
                    linked_account_name: r.get(12)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// A person picking a category. The app won't change it afterwards. Moving a matched
    /// transfer out of the Transfers group also undoes the match.
    pub fn set_transaction_category(&self, id: i64, category_id: Option<i64>) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE transactions SET category_id = ?2, category_auto = 0 WHERE id = ?1",
            params![id, category_id],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("transaction {id}")));
        }
        let still_transfer = match category_id {
            Some(c) => self.conn.query_row(
                "SELECT g.is_transfer FROM categories c JOIN category_groups g ON g.id = c.group_id WHERE c.id = ?1",
                [c],
                |r| r.get::<_, bool>(0),
            )?,
            None => false,
        };
        if !still_transfer {
            self.unlink_transaction(id)?;
        }
        Ok(())
    }

    /// Undo a transfer or wallet match on both sides.
    pub fn unlink_transaction(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE transactions SET linked_transaction_id = NULL
             WHERE linked_transaction_id = ?1
                OR id = ?1",
            [id],
        )?;
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

/// Earlier versions named bank accounts "{institution} {account name}", which doubled up
/// when the bank already starts its account names with its own name ("SoFi SoFi Savings").
fn tidy_bank_account_names(conn: &Connection) -> Result<()> {
    let rows: Vec<(i64, String, String)> = conn
        .prepare(
            "SELECT a.id, a.name, p.institution_name FROM accounts a JOIN plaid_items p ON p.id = a.plaid_item_id",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, name, institution) in rows {
        let inst = institution.trim();
        if inst.is_empty() {
            continue;
        }
        let doubled = format!("{inst} {inst}");
        if let Some(rest) = name.strip_prefix(&doubled) {
            let new_name = format!("{inst}{rest}");
            // Leave it alone if the tidy name is already taken.
            let taken = conn.query_row("SELECT 1 FROM accounts WHERE name = ?1", [&new_name], |_| Ok(())).optional()?.is_some();
            if !taken {
                conn.execute("UPDATE accounts SET name = ?2 WHERE id = ?1", params![id, new_name])?;
            }
        }
    }
    Ok(())
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
        assert_eq!(groups.len(), DEFAULT_CATEGORIES.len() + 1);
        assert!(groups[0].is_income);
        let transfers = groups.last().unwrap();
        assert!(transfers.is_transfer && transfers.name == "Transfers");
        assert_eq!(transfers.categories.len(), 4);
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
        assert_eq!(b.list_category_groups().unwrap().len(), DEFAULT_CATEGORIES.len() + 1);
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
