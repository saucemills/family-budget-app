//! Reflect: where the money went over a date range. Feeds the summary numbers,
//! the spending breakdown, and the Sankey chart (income → groups → categories).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::db::{validate_date, Budget};
use crate::error::{invalid, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomeSource {
    pub category_id: i64,
    pub name: String,
    pub amount_cents: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpendingCategory {
    pub category_id: i64,
    pub name: String,
    /// Net spending (purchases minus refunds), positive.
    pub amount_cents: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpendingGroup {
    pub group_id: i64,
    pub name: String,
    /// Position in the category list; the chart uses it to pick a stable color.
    pub sort_order: i64,
    pub amount_cents: i64,
    pub categories: Vec<SpendingCategory>,
}

/// Money moved into a savings or investment account (net of what was taken back out).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTo {
    pub account_id: i64,
    pub name: String,
    pub amount_cents: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectSummary {
    pub start: String,
    pub end: String,
    pub income_cents: i64,
    pub spending_cents: i64,
    pub income_sources: Vec<IncomeSource>,
    /// Largest first; categories inside each group are largest first too.
    pub spending_groups: Vec<SpendingGroup>,
    pub uncategorized_count: i64,
    pub uncategorized_out_cents: i64,
    /// Savings and investment accounts that money was moved into, largest first.
    /// Transfers aren't spending, but "where did the money go" should still show them.
    pub saved_to: Vec<SavedTo>,
    /// Total of `saved_to`.
    pub saved_cents: i64,
    /// Net money taken out of savings accounts that had more out than in (positive).
    pub from_savings_cents: i64,
    /// Every spending group that has ever had spending, in category-list order.
    /// The chart gives these the colors, so a group keeps its color across date ranges.
    pub color_order: Vec<i64>,
}

impl Budget {
    /// Summarize categorized transactions from `start` to `end`, inclusive (YYYY-MM-DD).
    pub fn reflect(&self, start: &str, end: &str) -> Result<ReflectSummary> {
        validate_date(start)?;
        validate_date(end)?;
        if start > end {
            return invalid("the start date is after the end date");
        }

        let mut stmt = self.conn.prepare(
            "SELECT g.id, g.name, g.sort_order, g.is_income, c.id, c.name, SUM(t.amount_cents)
             FROM transactions t
             JOIN categories c ON c.id = t.category_id
             JOIN category_groups g ON g.id = c.group_id
             WHERE t.date BETWEEN ?1 AND ?2 AND g.is_transfer = 0
             GROUP BY c.id",
        )?;
        let rows = stmt.query_map([start, end], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;

        let mut income_sources = Vec::new();
        let mut groups: BTreeMap<i64, SpendingGroup> = BTreeMap::new();
        for row in rows {
            let (gid, gname, gsort, is_income, cid, cname, sum) = row?;
            if is_income {
                if sum > 0 {
                    income_sources.push(IncomeSource { category_id: cid, name: cname, amount_cents: sum });
                }
                continue;
            }
            let spent = -sum;
            if spent <= 0 {
                continue; // more refunds than purchases: nothing to show
            }
            let g = groups.entry(gid).or_insert_with(|| SpendingGroup {
                group_id: gid,
                name: gname,
                sort_order: gsort,
                amount_cents: 0,
                categories: Vec::new(),
            });
            g.amount_cents += spent;
            g.categories.push(SpendingCategory { category_id: cid, name: cname, amount_cents: spent });
        }

        income_sources.sort_by(|a, b| b.amount_cents.cmp(&a.amount_cents));
        let mut spending_groups: Vec<SpendingGroup> = groups.into_values().collect();
        for g in &mut spending_groups {
            g.categories.sort_by(|a, b| b.amount_cents.cmp(&a.amount_cents));
        }
        spending_groups.sort_by(|a, b| b.amount_cents.cmp(&a.amount_cents));

        let (uncategorized_count, uncategorized_out_cents): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN amount_cents < 0 THEN -amount_cents ELSE 0 END), 0)
             FROM transactions WHERE category_id IS NULL AND date BETWEEN ?1 AND ?2",
            [start, end],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let color_order: Vec<i64> = self
            .conn
            .prepare(
                "SELECT g.id FROM category_groups g
                 WHERE g.is_income = 0 AND g.is_transfer = 0 AND EXISTS (
                     SELECT 1 FROM transactions t JOIN categories c ON c.id = t.category_id
                     WHERE c.group_id = g.id AND t.amount_cents < 0)
                 ORDER BY g.sort_order, g.id",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;

        // Net transfers into each savings / investment account. Only transfer-category
        // rows count: interest is already income, and uncategorized deposits are unknown.
        let mut saved_to: Vec<SavedTo> = Vec::new();
        let mut from_savings_cents = 0;
        {
            let mut stmt = self.conn.prepare(
                "SELECT a.id, a.name, SUM(t.amount_cents)
                 FROM transactions t
                 JOIN accounts a ON a.id = t.account_id
                 JOIN categories c ON c.id = t.category_id
                 JOIN category_groups g ON g.id = c.group_id
                 WHERE g.is_transfer = 1 AND a.kind IN ('savings', 'investment')
                   AND t.date BETWEEN ?1 AND ?2
                 GROUP BY a.id",
            )?;
            for row in stmt.query_map([start, end], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))? {
                let (account_id, name, net) = row?;
                if net > 0 {
                    saved_to.push(SavedTo { account_id, name, amount_cents: net });
                } else {
                    from_savings_cents -= net;
                }
            }
        }
        saved_to.sort_by(|a, b| b.amount_cents.cmp(&a.amount_cents));

        Ok(ReflectSummary {
            start: start.into(),
            end: end.into(),
            income_cents: income_sources.iter().map(|s| s.amount_cents).sum(),
            spending_cents: spending_groups.iter().map(|g| g.amount_cents).sum(),
            income_sources,
            spending_groups,
            uncategorized_count,
            uncategorized_out_cents,
            saved_cents: saved_to.iter().map(|s| s.amount_cents).sum(),
            saved_to,
            from_savings_cents,
            color_order,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::models::{AccountKind, NewTransaction};
    use crate::Budget;

    #[test]
    fn summarizes_income_and_spending() {
        let b = Budget::open_in_memory().unwrap();
        let acct = b.create_account("Checking", AccountKind::Checking).unwrap();
        let id = |name: &str| {
            b.list_category_groups().unwrap().into_iter().flat_map(|g| g.categories).find(|c| c.name == name).unwrap().id
        };
        let add = |date: &str, cents: i64, cat: Option<i64>| {
            b.add_transaction(&NewTransaction {
                account_id: acct,
                date: date.into(),
                payee: "x".into(),
                amount_cents: cents,
                category_id: cat,
                ..Default::default()
            })
            .unwrap();
        };
        add("2026-11-01", 400_000, Some(id("Paycheck")));
        add("2026-11-02", -150_000, Some(id("Rent / Mortgage")));
        add("2026-11-03", -20_000, Some(id("Groceries")));
        add("2026-11-04", -5_000, Some(id("Coffee")));
        add("2026-11-05", 1_000, Some(id("Coffee"))); // refund
        add("2026-11-06", -7_500, None);
        add("2026-10-31", -99_999, Some(id("Groceries"))); // outside the range

        let r = b.reflect("2026-11-01", "2026-11-30").unwrap();
        assert_eq!(r.income_cents, 400_000);
        assert_eq!(r.spending_cents, 150_000 + 20_000 + 4_000);
        assert_eq!(r.spending_groups[0].name, "Housing");
        let food = r.spending_groups.iter().find(|g| g.name == "Food").unwrap();
        assert_eq!(food.amount_cents, 24_000);
        assert_eq!(food.categories[0].name, "Groceries");
        assert_eq!((r.uncategorized_count, r.uncategorized_out_cents), (1, 7_500));

        // Housing, Food in list order; Health etc. never had spending.
        assert_eq!(r.color_order.len(), 2);
        assert_eq!(r.color_order[0], r.spending_groups[0].group_id);

        assert!(b.reflect("2026-12-01", "2026-11-01").is_err());
        assert_eq!(r.saved_cents, 0);
    }

    #[test]
    fn shows_money_moved_to_savings() {
        let b = Budget::open_in_memory().unwrap();
        let chk = b.create_account("Checking", AccountKind::Checking).unwrap();
        let sav = b.create_account("SoFi Savings", AccountKind::Savings).unwrap();
        let robo = b.create_account("SoFi Robo", AccountKind::Investment).unwrap();
        let add = |acct: i64, date: &str, cents: i64| {
            b.add_transaction(&NewTransaction { account_id: acct, date: date.into(), payee: "x".into(), amount_cents: cents, ..Default::default() })
                .unwrap()
        };
        let paycheck = add(chk, "2026-11-01", 500_000);
        let id = |name: &str| b.list_category_groups().unwrap().into_iter().flat_map(|g| g.categories).find(|c| c.name == name).unwrap().id;
        b.set_transaction_category(paycheck, Some(id("Paycheck"))).unwrap();
        add(chk, "2026-11-02", -600_000);
        add(sav, "2026-11-02", 600_000); // checking -> savings
        add(sav, "2026-11-20", -100_000);
        add(chk, "2026-11-20", 100_000); // a little back out
        add(chk, "2026-11-21", -50_000);
        add(robo, "2026-11-22", 50_000); // checking -> investing
        let interest = add(sav, "2026-11-30", 1_200);
        b.set_transaction_category(interest, Some(id("Other Income"))).unwrap();
        b.detect_transfers().unwrap();

        let r = b.reflect("2026-11-01", "2026-11-30").unwrap();
        assert_eq!(r.saved_cents, 550_000);
        assert_eq!(r.saved_to.iter().map(|s| (s.name.as_str(), s.amount_cents)).collect::<Vec<_>>(), [("SoFi Savings", 500_000), ("SoFi Robo", 50_000)]);
        assert_eq!(r.from_savings_cents, 0);
        assert_eq!(r.income_cents, 501_200); // interest is income, transfers aren't
        assert_eq!(r.spending_cents, 0);

        // A month where more came out of savings than went in.
        add(sav, "2026-12-05", -200_000);
        add(chk, "2026-12-05", 200_000);
        b.detect_transfers().unwrap();
        let dec = b.reflect("2026-12-01", "2026-12-31").unwrap();
        assert_eq!((dec.saved_cents, dec.from_savings_cents), (0, 200_000));
    }
}
