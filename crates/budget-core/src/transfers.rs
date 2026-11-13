//! Spotting money that only moved between the family's own accounts, so Plan and Reflect
//! don't count it as income or spending.
//!
//! Three kinds of match, tried in this order:
//! 1. **Paid through another account.** A bank charge whose payee names another linked
//!    institution (`Web Authorized Pmt Paypal`) and an equal charge in that institution's
//!    account a few days around it (the PayPal purchase from Sony). The bank side goes to
//!    "Paid through another account" and the purchase keeps its own category, so the
//!    spending is counted once, under what was actually bought.
//! 2. **Matching in and out.** Money leaving one account and the same amount arriving in
//!    another within a few days: "Between my accounts", or "Credit card payment" /
//!    "Loan payment" when it lands in a card or loan account.
//! 3. **From my other bank.** Money arriving from an institution the family has linked
//!    (`US BANK NA` arriving at SoFi) whose other half is older than the linked history.
//!
//! Only transactions that are uncategorized, or that the app categorized itself, are
//! touched. A category a person picked is never changed.

use std::collections::HashSet;

use chrono::NaiveDate;
use rusqlite::params;

use crate::db::Budget;
use crate::error::Result;
use crate::models::AccountKind;

pub const BETWEEN_ACCOUNTS: &str = "Between my accounts";
pub const CARD_PAYMENT: &str = "Credit card payment";
pub const LOAN_PAYMENT: &str = "Loan payment";
pub const PAID_THROUGH: &str = "Paid through another account";

/// How far apart (in days) the two halves of a transfer may post.
const PAIR_DAYS: i64 = 4;

struct Row {
    id: i64,
    account_id: i64,
    item: Option<i64>,
    kind: AccountKind,
    date: NaiveDate,
    payee: String,
    amount: i64,
    /// Uncategorized, or categorized by the app (not a person).
    free: bool,
    /// Uncategorized, or auto-categorized as income or a transfer: safe to call a transfer
    /// on an amount match alone. (An auto-categorized purchase isn't.)
    soft: bool,
}

/// Lowercase letters and digits only, so "U.S. Bank" matches "US BANK NA".
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// Card and loan statements label incoming payments like "AUTOPAY PAYMENT - THANK YOU",
/// "ONLINE PYMT", "MOBILE PMT". `payee` is already squashed.
fn looks_like_payment(payee: &str) -> bool {
    ["payment", "pymt", "pmt", "thankyou", "autopay"].iter().any(|w| payee.contains(w))
}

impl Budget {
    fn transfer_category(&self, name: &str) -> Result<Option<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id FROM categories c JOIN category_groups g ON g.id = c.group_id
             WHERE g.is_transfer = 1 AND c.name = ?1 AND c.hidden = 0 ORDER BY c.id LIMIT 1",
        )?;
        let mut rows = stmt.query([name])?;
        Ok(match rows.next()? {
            Some(r) => Some(r.get(0)?),
            None => None,
        })
    }

    /// Find and mark transfers. Returns how many transactions changed. Safe to run any
    /// time; runs when the budget opens, after a CSV import and after each bank sync.
    pub fn detect_transfers(&self) -> Result<usize> {
        let between = self.transfer_category(BETWEEN_ACCOUNTS)?;
        let card = self.transfer_category(CARD_PAYMENT)?;
        let loan = self.transfer_category(LOAN_PAYMENT)?;
        let paid_through = self.transfer_category(PAID_THROUGH)?;

        let rows: Vec<Row> = self
            .conn
            .prepare(
                "SELECT t.id, t.account_id, a.plaid_item_id, a.kind, t.date, t.payee, t.amount_cents,
                        t.category_id IS NULL OR t.category_auto = 1,
                        t.category_id IS NULL OR (t.category_auto = 1 AND (g.is_income = 1 OR g.is_transfer = 1))
                 FROM transactions t
                 JOIN accounts a ON a.id = t.account_id
                 LEFT JOIN categories c ON c.id = t.category_id
                 LEFT JOIN category_groups g ON g.id = c.group_id
                 WHERE t.linked_transaction_id IS NULL AND t.amount_cents <> 0
                 ORDER BY t.date, t.id",
            )?
            .query_map([], |r| {
                let kind: String = r.get(3)?;
                let date: String = r.get(4)?;
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, kind, date, r.get::<_, String>(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
            })?
            .filter_map(|row| {
                let (id, account_id, item, kind, date, payee, amount, free, soft) = match row {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                let date = NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok()?;
                Some(Ok(Row {
                    id,
                    account_id,
                    item,
                    kind: AccountKind::parse(&kind).unwrap_or(AccountKind::Checking),
                    date,
                    payee: squash(&payee),
                    amount,
                    free,
                    soft,
                }))
            })
            .collect::<rusqlite::Result<_>>()?;

        // Linked institutions, by squashed name. Short names would match too much.
        let institutions: Vec<(i64, String)> = self
            .conn
            .prepare("SELECT id, institution_name FROM plaid_items")?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, squash(&r.get::<_, String>(1)?))))?
            .filter(|r| r.as_ref().map_or(true, |(_, n)| n.len() >= 4))
            .collect::<rusqlite::Result<_>>()?;
        let names_other_institution = |r: &Row| -> Option<i64> {
            institutions.iter().find(|(id, name)| Some(*id) != r.item && r.payee.contains(name.as_str())).map(|(id, _)| *id)
        };
        let names_institution = |r: &Row, item: Option<i64>| -> bool {
            institutions.iter().any(|(id, name)| Some(*id) == item && r.payee.contains(name.as_str()))
        };
        // Same bank (or a hand-entered account): an amount and date match is enough. Between
        // two different banks, one side must name the other, or any two equal amounts a few
        // days apart (a card payment and someone else's deposit) would look like a transfer.
        // A card or loan receiving a "payment" is paid from one of your accounts, even though
        // neither side names the other bank ("Amex Epayment" vs "PAYMENT - THANK YOU").
        let plausible_pair = |out: &Row, inn: &Row| -> bool {
            out.item.is_none() || inn.item.is_none() || out.item == inn.item
                || names_institution(out, inn.item) || names_institution(inn, out.item)
                || (matches!(inn.kind, AccountKind::CreditCard | AccountKind::Loan) && looks_like_payment(&inn.payee))
        };

        let mut used: HashSet<i64> = HashSet::new();
        // (row, partner, category for row, category for partner)
        let mut links: Vec<(i64, Option<i64>, Option<i64>, Option<i64>)> = Vec::new();

        // 1. Paid through another account (PayPal and the like).
        if let Some(paid_through) = paid_through {
            for a in rows.iter().filter(|r| r.amount < 0 && r.free) {
                if used.contains(&a.id) {
                    continue;
                }
                let Some(inst) = names_other_institution(a) else { continue };
                // The purchase posts first; the bank pulls the money up to a few days later.
                let best = rows
                    .iter()
                    .filter(|b| b.item == Some(inst) && b.amount == a.amount && b.id != a.id && !used.contains(&b.id))
                    .filter(|b| (-1..=5).contains(&(a.date - b.date).num_days()))
                    .min_by_key(|b| (a.date - b.date).num_days().abs());
                if let Some(b) = best {
                    used.insert(a.id);
                    used.insert(b.id);
                    links.push((a.id, Some(b.id), Some(paid_through), None));
                }
            }
        }

        // 2. The same amount leaving one account and arriving in another.
        for out in rows.iter().filter(|r| r.amount < 0 && r.soft) {
            if used.contains(&out.id) {
                continue;
            }
            let best = rows
                .iter()
                .filter(|i| i.amount == -out.amount && i.account_id != out.account_id && i.soft && !used.contains(&i.id))
                .filter(|i| (i.date - out.date).num_days().abs() <= PAIR_DAYS && plausible_pair(out, i))
                .min_by_key(|i| (i.date - out.date).num_days().abs());
            if let Some(inn) = best {
                let cat = match inn.kind {
                    AccountKind::CreditCard => card.or(between),
                    AccountKind::Loan => loan.or(between),
                    _ => between,
                };
                if cat.is_none() {
                    continue;
                }
                used.insert(out.id);
                used.insert(inn.id);
                links.push((out.id, Some(inn.id), cat, cat));
            }
        }

        // 3. Money arriving from another linked bank, whose other half we don't have.
        if let Some(between) = between {
            for r in rows.iter().filter(|r| r.amount > 0 && r.soft && !used.contains(&r.id)) {
                if names_other_institution(r).is_some() {
                    links.push((r.id, None, Some(between), None));
                }
            }
        }

        let tx = self.conn.unchecked_transaction()?;
        let mut changed = 0;
        {
            let mut set_cat = tx.prepare(
                "UPDATE transactions SET category_id = ?2, category_auto = 1
                 WHERE id = ?1 AND category_id IS NOT ?2",
            )?;
            let mut set_link = tx.prepare("UPDATE transactions SET linked_transaction_id = ?2 WHERE id = ?1")?;
            for (a, b, cat_a, cat_b) in &links {
                if let Some(c) = cat_a {
                    changed += set_cat.execute(params![a, c])?;
                }
                if let Some(b) = b {
                    if let Some(c) = cat_b {
                        set_cat.execute(params![b, c])?;
                    }
                    set_link.execute(params![a, b])?;
                    set_link.execute(params![b, a])?;
                    changed += 1;
                }
            }
        }
        tx.commit()?;
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NewTransaction, TransactionFilter};

    fn add(b: &Budget, account_id: i64, date: &str, payee: &str, cents: i64) -> i64 {
        b.add_transaction(&NewTransaction { account_id, date: date.into(), payee: payee.into(), amount_cents: cents, ..Default::default() })
            .unwrap()
    }

    fn cat_id(b: &Budget, name: &str) -> i64 {
        b.list_category_groups().unwrap().into_iter().flat_map(|g| g.categories).find(|c| c.name == name).unwrap().id
    }

    fn get(b: &Budget, id: i64) -> crate::models::Transaction {
        b.list_transactions(&TransactionFilter::default()).unwrap().into_iter().find(|t| t.id == id).unwrap()
    }

    fn link_to_item(b: &Budget, account: i64, item: &str, institution: &str) {
        let item_id = b.save_plaid_item(item, institution).unwrap();
        b.conn.execute("UPDATE accounts SET plaid_item_id = ?2 WHERE id = ?1", params![account, item_id]).unwrap();
    }

    #[test]
    fn paypal_payment_pairs_with_the_purchase() {
        let b = Budget::open_in_memory().unwrap();
        let bank = b.create_account("U.S. Bank Checking", AccountKind::Checking).unwrap();
        let paypal = b.create_account("PayPal", AccountKind::Checking).unwrap();
        link_to_item(&b, bank, "i1", "U.S. Bank");
        link_to_item(&b, paypal, "i2", "PayPal");
        let fun = cat_id(&b, "Fun Money");

        let sony = add(&b, paypal, "2026-11-02", "Sony Interactive Entertainment", -9999);
        b.set_transaction_category(sony, Some(fun)).unwrap();
        let valve = add(&b, paypal, "2026-11-05", "Valve Corporation", -1349);
        let pmt1 = add(&b, bank, "2026-11-03", "Web Authorized Pmt Paypal", -9999);
        let pmt2 = add(&b, bank, "2026-11-06", "Web Authorized Pmt Paypal", -1349);
        let orphan = add(&b, bank, "2026-10-14", "Web Authorized Pmt Paypal", -4899);

        assert_eq!(b.detect_transfers().unwrap(), 4);
        let p1 = get(&b, pmt1);
        assert_eq!(p1.category_name.as_deref(), Some(PAID_THROUGH));
        assert_eq!(p1.linked_payee.as_deref(), Some("Sony Interactive Entertainment"));
        assert_eq!(p1.linked_account_name.as_deref(), Some("PayPal"));
        assert_eq!(get(&b, sony).category_name.as_deref(), Some("Fun Money")); // the purchase keeps its category
        assert_eq!(get(&b, sony).linked_transaction_id, Some(pmt1));
        assert_eq!(get(&b, pmt2).linked_transaction_id, Some(valve));
        assert_eq!(get(&b, valve).category_name, None); // still waiting for the family to say what it was
        assert_eq!(get(&b, orphan).category_name, None);

        // Running again changes nothing.
        assert_eq!(b.detect_transfers().unwrap(), 0);

        // Spending is counted once, under what was bought.
        let r = b.reflect("2026-11-01", "2026-11-30").unwrap();
        assert_eq!(r.spending_cents, 9999);
    }

    #[test]
    fn matching_in_and_out_between_own_accounts() {
        let b = Budget::open_in_memory().unwrap();
        let chk = b.create_account("Checking", AccountKind::Checking).unwrap();
        let sav = b.create_account("Savings", AccountKind::Savings).unwrap();
        let card = b.create_account("Visa", AccountKind::CreditCard).unwrap();
        let other_income = cat_id(&b, "Other Income");
        let shopping = cat_id(&b, "Shopping");

        let out = add(&b, chk, "2026-10-30", "SoFi", -600_000);
        let inn = add(&b, sav, "2026-10-30", "US BANK NA", 600_000);
        // A Plaid guess of "Other Income" on a transfer gets corrected...
        b.conn.execute("UPDATE transactions SET category_id = ?2, category_auto = 1 WHERE id = ?1", params![inn, other_income]).unwrap();
        let pay = add(&b, chk, "2026-11-01", "CARD PAYMENT", -25_000);
        let pay_in = add(&b, card, "2026-11-03", "PAYMENT THANK YOU", 25_000);
        // ...but a person's choice and an auto-categorized purchase are left alone.
        let mine = add(&b, chk, "2026-11-05", "Gift to mom", -5_000);
        b.set_transaction_category(mine, Some(cat_id(&b, "Gifts"))).unwrap();
        add(&b, sav, "2026-11-05", "Deposit", 5_000);
        let buy = add(&b, chk, "2026-11-07", "Target", -2_000);
        b.conn.execute("UPDATE transactions SET category_id = ?2, category_auto = 1 WHERE id = ?1", params![buy, shopping]).unwrap();
        add(&b, card, "2026-11-08", "Refund", 2_000);
        // Too far apart.
        add(&b, chk, "2026-11-10", "x", -777);
        add(&b, sav, "2026-11-20", "y", 777);

        b.detect_transfers().unwrap();
        assert_eq!(get(&b, out).category_name.as_deref(), Some(BETWEEN_ACCOUNTS));
        assert_eq!(get(&b, inn).category_name.as_deref(), Some(BETWEEN_ACCOUNTS));
        assert_eq!(get(&b, out).linked_transaction_id, Some(inn));
        assert_eq!(get(&b, pay).category_name.as_deref(), Some(CARD_PAYMENT));
        assert_eq!(get(&b, pay_in).category_name.as_deref(), Some(CARD_PAYMENT));
        assert_eq!(get(&b, mine).category_name.as_deref(), Some("Gifts"));
        assert_eq!(get(&b, buy).category_name.as_deref(), Some("Shopping"));
        let linked = b.list_transactions(&TransactionFilter::default()).unwrap().into_iter().filter(|t| t.linked_transaction_id.is_some()).count();
        assert_eq!(linked, 4);

        // Transfers aren't income or spending.
        let r = b.reflect("2026-10-01", "2026-11-30").unwrap();
        assert_eq!(r.income_cents, 0);
        assert!(r.spending_groups.iter().all(|g| g.name != "Transfers"));
        let plan = b.get_month_plan("2026-10").unwrap();
        assert!(plan.groups.iter().all(|g| g.name != "Transfers"));

        // Moving one side out of Transfers undoes the match on both sides.
        b.set_transaction_category(out, Some(cat_id(&b, "Investments"))).unwrap();
        assert_eq!(get(&b, out).linked_transaction_id, None);
        assert_eq!(get(&b, inn).linked_transaction_id, None);
        b.detect_transfers().unwrap();
        assert_eq!(get(&b, out).category_name.as_deref(), Some("Investments")); // a person's choice sticks
    }

    #[test]
    fn money_from_another_linked_bank() {
        let b = Budget::open_in_memory().unwrap();
        let usb = b.create_account("U.S. Bank Checking", AccountKind::Checking).unwrap();
        let sofi = b.create_account("SoFi Savings", AccountKind::Savings).unwrap();
        link_to_item(&b, usb, "i1", "U.S. Bank");
        link_to_item(&b, sofi, "i2", "SoFi");
        let from_usb = add(&b, sofi, "2025-09-02", "US BANK NA", 350_000);
        let own_fee = add(&b, sofi, "2025-09-03", "SoFi", 1_000); // its own institution: not a transfer
        let boa = add(&b, sofi, "2025-09-04", "BANK OF AMERICA, N.A.", 2_000); // not linked: unknown
        // Equal amounts at two different banks that don't name each other aren't a transfer.
        let amex = add(&b, usb, "2025-09-01", "Web Authorized Pmt Amex Epayment", -2_000);
        // But ones that do are.
        let to_sofi = add(&b, usb, "2025-09-10", "SoFi", -600_000);
        let at_sofi = add(&b, sofi, "2025-09-12", "Transfer in", 600_000);
        b.detect_transfers().unwrap();
        assert_eq!(get(&b, amex).category_name, None);
        assert_eq!(get(&b, to_sofi).linked_transaction_id, Some(at_sofi));

        // A card payment pairs with the card once the card is connected, though neither names the other.
        let amex_card = b.create_account("American Express Gold Card", AccountKind::CreditCard).unwrap();
        link_to_item(&b, amex_card, "i3", "American Express");
        let thanks = add(&b, amex_card, "2025-09-03", "ONLINE PAYMENT - THANK YOU", 2_000);
        let refund = add(&b, amex_card, "2025-09-02", "Whole Foods refund", 2_000);
        b.detect_transfers().unwrap();
        assert_eq!(get(&b, amex).linked_transaction_id, Some(thanks));
        assert_eq!(get(&b, amex).category_name.as_deref(), Some(CARD_PAYMENT));
        assert_eq!(get(&b, refund).linked_transaction_id, None);
        assert_eq!(get(&b, from_usb).category_name.as_deref(), Some(BETWEEN_ACCOUNTS));
        assert_eq!(get(&b, own_fee).category_name, None);
        assert_eq!(get(&b, boa).category_name, None);
    }
}
