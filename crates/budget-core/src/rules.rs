//! Auto-categorize rules: "a payee containing STARBUCKS goes to Coffee".
//! Rules run on every import (CSV and bank sync) for transactions that don't have a
//! category yet. When several rules match, the longest (most specific) one wins.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Budget;
use crate::error::{invalid, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: i64,
    pub match_text: String,
    pub category_id: i64,
    pub category_name: String,
    /// Only amounts at least this big (positive cents, money in or out).
    pub min_cents: Option<i64>,
    /// Only amounts at most this big.
    pub max_cents: Option<i64>,
}

/// SQL: does rule `r` match `transactions`? Ranged rules count the size of the amount.
const RULE_MATCHES: &str = "instr(upper(transactions.payee), upper(r.match_text)) > 0
    AND (r.min_cents IS NULL OR abs(transactions.amount_cents) >= r.min_cents)
    AND (r.max_cents IS NULL OR abs(transactions.amount_cents) <= r.max_cents)";

/// Turn a bank payee like `STARBUCKS #1234` or `SHELL OIL 57444` into the stable part
/// worth matching on (`STARBUCKS`, `SHELL OIL`), dropping store numbers and ids.
pub fn suggest_match_text(payee: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for word in payee.split_whitespace() {
        // Card processors prefix the merchant: "SQ *BLUE BOTTLE", "TST* JOE'S". Keep what follows.
        if word.ends_with('*') {
            kept.clear();
            continue;
        }
        let word = match word.strip_prefix('*') {
            Some(rest) => {
                kept.clear();
                rest
            }
            None => word,
        };
        if word.contains('#') || word.chars().any(|c| c.is_ascii_digit()) || word == "*" {
            break;
        }
        kept.push(word);
        if kept.len() == 4 {
            break;
        }
    }
    let text = kept.join(" ");
    let text = text.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '&').trim();
    if text.chars().filter(|c| c.is_alphanumeric()).count() >= 3 { text.to_string() } else { payee.trim().to_string() }
}

impl Budget {
    pub fn list_rules(&self) -> Result<Vec<Rule>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.match_text, r.category_id, c.name, r.min_cents, r.max_cents
             FROM category_rules r JOIN categories c ON c.id = r.category_id
             ORDER BY r.match_text COLLATE NOCASE, r.min_cents, r.max_cents",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Rule {
                id: r.get(0)?,
                match_text: r.get(1)?,
                category_id: r.get(2)?,
                category_name: r.get(3)?,
                min_cents: r.get(4)?,
                max_cents: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Add a rule for any amount (or change the category of an existing one with the same
    /// text), then apply it to uncategorized transactions. Returns how many it categorized.
    pub fn add_rule(&self, match_text: &str, category_id: i64) -> Result<usize> {
        self.add_ranged_rule(match_text, category_id, None, None)
    }

    /// Like `add_rule`, limited to amounts between `min_cents` and `max_cents` (positive
    /// cents, inclusive; either may be open). A rule with the same text and range is updated.
    pub fn add_ranged_rule(&self, match_text: &str, category_id: i64, min_cents: Option<i64>, max_cents: Option<i64>) -> Result<usize> {
        let text = match_text.trim();
        if text.len() < 2 {
            return invalid("rule text needs at least 2 characters");
        }
        let min_cents = min_cents.map(i64::abs);
        let max_cents = max_cents.map(i64::abs);
        if let (Some(lo), Some(hi)) = (min_cents, max_cents) {
            if lo > hi {
                return invalid("the smallest amount is bigger than the largest");
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        let updated = tx.execute(
            "UPDATE category_rules SET category_id = ?2
             WHERE match_text = ?1 AND min_cents IS ?3 AND max_cents IS ?4",
            params![text, category_id, min_cents, max_cents],
        )?;
        if updated == 0 {
            tx.execute(
                "INSERT INTO category_rules (match_text, category_id, min_cents, max_cents) VALUES (?1, ?2, ?3, ?4)",
                params![text, category_id, min_cents, max_cents],
            )?;
        }
        let rule_id: i64 = tx.query_row(
            "SELECT id FROM category_rules WHERE match_text = ?1 AND min_cents IS ?2 AND max_cents IS ?3",
            params![text, min_cents, max_cents],
            |r| r.get(0),
        )?;
        // Reach back into history: past transactions the app categorized (a rule or Plaid's
        // guess) are re-decided now that there's a new rule. Ones a person picked, and matched
        // transfers, are left alone.
        let before: Vec<(i64, Option<i64>)> = tx
            .prepare(&format!(
                "SELECT t.id, t.category_id FROM transactions t
                 JOIN category_rules r ON r.id = ?1 AND {}
                 WHERE (t.category_id IS NULL OR t.category_auto = 1) AND t.linked_transaction_id IS NULL
                   AND NOT EXISTS (SELECT 1 FROM categories c JOIN category_groups g ON g.id = c.group_id
                                   WHERE c.id = t.category_id AND g.is_transfer = 1)",
                RULE_MATCHES.replace("transactions.", "t.")
            ))?
            .query_map([rule_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, _) in &before {
            tx.execute("UPDATE transactions SET category_id = NULL WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        self.apply_rules()?;
        // Count the transactions whose category actually changed.
        let mut changed = 0;
        for (id, old) in before {
            let now: Option<i64> = self.conn.query_row("SELECT category_id FROM transactions WHERE id = ?1", [id], |r| r.get(0))?;
            if now != old {
                changed += 1;
            }
        }
        Ok(changed)
    }

    pub fn delete_rule(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM category_rules WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Categorize every uncategorized transaction that a rule matches. A rule with an
    /// amount range beats one without; then the longest text wins.
    pub fn apply_rules(&self) -> Result<usize> {
        Ok(self.conn.execute(
            &format!(
                "UPDATE transactions SET category_auto = 1, category_id = (
                     SELECT r.category_id FROM category_rules r WHERE {RULE_MATCHES}
                     ORDER BY (r.min_cents IS NOT NULL OR r.max_cents IS NOT NULL) DESC, length(r.match_text) DESC, r.id
                     LIMIT 1)
                 WHERE category_id IS NULL AND EXISTS (SELECT 1 FROM category_rules r WHERE {RULE_MATCHES})"
            ),
            [],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AccountKind, NewTransaction, TransactionFilter};

    #[test]
    fn suggests_stable_payee_text() {
        assert_eq!(suggest_match_text("STARBUCKS #1234"), "STARBUCKS");
        assert_eq!(suggest_match_text("SHELL OIL 57444"), "SHELL OIL");
        assert_eq!(suggest_match_text("TRADER JOE'S #552"), "TRADER JOE'S");
        assert_eq!(suggest_match_text("NETFLIX.COM"), "NETFLIX.COM");
        assert_eq!(suggest_match_text("SQ *BLUE BOTTLE 123"), "BLUE BOTTLE");
        assert_eq!(suggest_match_text("TST* JOE'S PIZZA"), "JOE'S PIZZA");
        assert_eq!(suggest_match_text("7-ELEVEN 123"), "7-ELEVEN 123");
    }

    #[test]
    fn rules_categorize_uncategorized_and_longest_wins() {
        let b = Budget::open_in_memory().unwrap();
        let acct = b.create_account("Card", AccountKind::CreditCard).unwrap();
        let cats: Vec<_> = b.list_category_groups().unwrap().into_iter().flat_map(|g| g.categories).collect();
        let id = |n: &str| cats.iter().find(|c| c.name == n).unwrap().id;
        for (payee, cat) in [("Starbucks #12", None), ("AMAZON FRESH", None), ("AMAZON.COM", None), ("STARBUCKS #9", Some(id("Restaurants")))] {
            b.add_transaction(&NewTransaction {
                account_id: acct,
                date: "2026-11-01".into(),
                payee: payee.into(),
                amount_cents: -100,
                category_id: cat,
                ..Default::default()
            })
            .unwrap();
        }
        assert_eq!(b.add_rule("STARBUCKS", id("Coffee")).unwrap(), 1); // the already-categorized one is left alone
        b.add_rule("amazon", id("Shopping")).unwrap();
        // A more specific rule re-decides what an earlier rule categorized.
        assert_eq!(b.add_rule("amazon fresh", id("Groceries")).unwrap(), 1);
        let txns = b.list_transactions(&TransactionFilter::default()).unwrap();
        let cat_of = |p: &str| txns.iter().find(|t| t.payee == p).unwrap().category_name.clone().unwrap();
        assert_eq!(cat_of("Starbucks #12"), "Coffee");
        assert_eq!(cat_of("STARBUCKS #9"), "Restaurants");
        assert_eq!(cat_of("AMAZON FRESH"), "Groceries");
        assert_eq!(cat_of("AMAZON.COM"), "Shopping");
        assert_eq!(b.list_rules().unwrap().len(), 3);

        // Re-adding the same text (any case) updates instead of duplicating.
        b.add_rule("Starbucks", id("Restaurants")).unwrap();
        let rules = b.list_rules().unwrap();
        assert_eq!(rules.len(), 3);
        assert!(rules.iter().any(|r| r.category_name == "Restaurants"));
        assert!(b.add_rule(" ", id("Coffee")).is_err());
    }

    #[test]
    fn new_rules_reach_back_but_respect_people_and_transfers() {
        let b = Budget::open_in_memory().unwrap();
        let acct = b.create_account("Checking", AccountKind::Checking).unwrap();
        let cats: Vec<_> = b.list_category_groups().unwrap().into_iter().flat_map(|g| g.categories).collect();
        let id = |n: &str| cats.iter().find(|c| c.name == n).unwrap().id;
        let add = |date: &str| {
            b.add_transaction(&NewTransaction { account_id: acct, date: date.into(), payee: "USAA.COM PAYMNT".into(), amount_cents: -15_000, ..Default::default() })
                .unwrap()
        };
        let (guessed, blank, mine, transfer) = (add("2026-01-01"), add("2026-02-01"), add("2026-03-01"), add("2026-04-01"));
        // Plaid guessed Shopping; a person picked Gifts; one is a matched transfer.
        b.conn.execute("UPDATE transactions SET category_id = ?2, category_auto = 1 WHERE id = ?1", params![guessed, id("Shopping")]).unwrap();
        b.set_transaction_category(mine, Some(id("Gifts"))).unwrap();
        b.conn.execute("UPDATE transactions SET category_id = ?2, category_auto = 1, linked_transaction_id = ?3 WHERE id = ?1", params![transfer, id("Between my accounts"), guessed]).unwrap();
        b.conn.execute("UPDATE transactions SET linked_transaction_id = NULL WHERE id = ?1", [guessed]).unwrap();

        assert_eq!(b.add_rule("USAA", id("Car Insurance")).unwrap(), 2);
        let txns = b.list_transactions(&TransactionFilter::default()).unwrap();
        let cat_of = |i: i64| txns.iter().find(|t| t.id == i).unwrap().category_name.clone().unwrap();
        assert_eq!(cat_of(guessed), "Car Insurance");
        assert_eq!(cat_of(blank), "Car Insurance");
        assert_eq!(cat_of(mine), "Gifts");
        assert_eq!(cat_of(transfer), "Between my accounts");
    }

    #[test]
    fn amount_ranges_split_one_payee() {
        let b = Budget::open_in_memory().unwrap();
        let acct = b.create_account("Checking", AccountKind::Checking).unwrap();
        let cats: Vec<_> = b.list_category_groups().unwrap().into_iter().flat_map(|g| g.categories).collect();
        let id = |n: &str| cats.iter().find(|c| c.name == n).unwrap().id;
        for cents in [-1_999, -16_250, -24_999, -25_001, -2_813_702] {
            b.add_transaction(&NewTransaction { account_id: acct, date: "2026-11-01".into(), payee: "Tesla".into(), amount_cents: cents, ..Default::default() })
                .unwrap();
        }
        assert_eq!(b.add_ranged_rule("TESLA", id("Car Insurance"), Some(12_000), Some(25_000)).unwrap(), 2);
        assert_eq!(b.add_ranged_rule("tesla", id("Gas"), None, Some(10_000)).unwrap(), 1);
        assert!(b.add_ranged_rule("tesla", id("Gas"), Some(500), Some(100)).is_err());
        let txns = b.list_transactions(&TransactionFilter::default()).unwrap();
        let cat_of = |c: i64| txns.iter().find(|t| t.amount_cents == c).unwrap().category_name.clone();
        assert_eq!(cat_of(-16_250).as_deref(), Some("Car Insurance"));
        assert_eq!(cat_of(-24_999).as_deref(), Some("Car Insurance"));
        assert_eq!(cat_of(-1_999).as_deref(), Some("Gas"));
        assert_eq!(cat_of(-25_001), None);
        assert_eq!(cat_of(-2_813_702), None);
        // Same text, same range: updates. Same text, different range: a second rule.
        b.add_ranged_rule("Tesla", id("Medical"), Some(12_000), Some(25_000)).unwrap();
        let rules = b.list_rules().unwrap();
        assert_eq!(rules.len(), 2);
        assert!(rules.iter().any(|r| r.category_name == "Medical" && r.min_cents == Some(12_000)));
    }
}
