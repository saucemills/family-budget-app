//! Plan: the YNAB-style monthly budget.
//!
//! The rules, in brief:
//! - Money that lands in an **income** category becomes "Ready to Assign".
//! - Each month you **assign** some of it to spending categories.
//! - A category's **available** amount is what's left: last month's leftover + this
//!   month's assigned + this month's activity (spending is negative).
//! - Leftover money rolls into next month. If a category ends a month overspent
//!   (below zero), it starts the next month at zero and the overspent amount comes
//!   out of next month's Ready to Assign instead, the way YNAB handles cash overspending.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{Datelike, NaiveDate};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Budget;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanCategory {
    pub id: i64,
    pub name: String,
    pub assigned_cents: i64,
    /// Net transactions this month (spending is negative).
    pub activity_cents: i64,
    pub available_cents: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanGroup {
    pub id: i64,
    pub name: String,
    pub assigned_cents: i64,
    pub activity_cents: i64,
    pub available_cents: i64,
    pub categories: Vec<PlanCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthPlan {
    /// `YYYY-MM`
    pub month: String,
    /// Money not yet given a job. Negative means more was assigned than has come in.
    pub ready_to_assign_cents: i64,
    pub income_this_month_cents: i64,
    pub assigned_this_month_cents: i64,
    /// Overspending carried in from last month, already taken out of Ready to Assign (zero or negative).
    pub overspent_last_month_cents: i64,
    pub groups: Vec<PlanGroup>,
    /// Transactions this month with no category; they don't count anywhere until categorized.
    pub uncategorized_count: i64,
}

pub fn parse_month(month: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")
        .map_err(|_| Error::Invalid(format!("month must be YYYY-MM, got {month:?}")))
}

fn next_month(d: NaiveDate) -> NaiveDate {
    if d.month() == 12 {
        NaiveDate::from_ymd_opt(d.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1).unwrap()
    }
}

fn prev_month(d: NaiveDate) -> NaiveDate {
    if d.month() == 1 {
        NaiveDate::from_ymd_opt(d.year() - 1, 12, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(d.year(), d.month() - 1, 1).unwrap()
    }
}

fn fmt_month(d: NaiveDate) -> String {
    d.format("%Y-%m").to_string()
}

struct CatInfo {
    id: i64,
    group_id: i64,
    name: String,
    hidden: bool,
    is_income: bool,
}

impl Budget {
    pub fn get_month_plan(&self, month: &str) -> Result<MonthPlan> {
        let target = parse_month(month)?;
        let target_s = fmt_month(target);

        let cats: Vec<CatInfo> = self
            .conn
            .prepare(
                "SELECT c.id, c.group_id, c.name, c.hidden, g.is_income
                 FROM categories c JOIN category_groups g ON g.id = c.group_id
                 WHERE g.is_transfer = 0
                 ORDER BY g.sort_order, g.id, c.sort_order, c.id",
            )?
            .query_map([], |r| {
                Ok(CatInfo { id: r.get(0)?, group_id: r.get(1)?, name: r.get(2)?, hidden: r.get(3)?, is_income: r.get(4)? })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let income_ids: HashSet<i64> = cats.iter().filter(|c| c.is_income).map(|c| c.id).collect();

        // (month, category) -> cents
        let mut activity: HashMap<(String, i64), i64> = HashMap::new();
        let mut assigned: HashMap<(String, i64), i64> = HashMap::new();
        let mut months: BTreeSet<String> = BTreeSet::new();
        months.insert(target_s.clone());

        let mut stmt = self.conn.prepare(
            "SELECT substr(date, 1, 7) AS m, category_id, SUM(amount_cents)
             FROM transactions WHERE category_id IS NOT NULL AND substr(date, 1, 7) <= ?1
             GROUP BY m, category_id",
        )?;
        for row in stmt.query_map([&target_s], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))? {
            let (m, c, v) = row?;
            months.insert(m.clone());
            activity.insert((m, c), v);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT month, category_id, assigned_cents FROM budget_assignments WHERE month <= ?1")?;
        for row in stmt.query_map([&target_s], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))? {
            let (m, c, v) = row?;
            months.insert(m.clone());
            assigned.insert((m, c), v);
        }

        // Walk month by month from the earliest data up to the target month.
        let mut cursor = parse_month(months.first().unwrap())?;
        let mut carry: HashMap<i64, i64> = HashMap::new();
        let mut total_income = 0i64;
        let mut total_assigned = 0i64;
        let mut total_overspent_before = 0i64; // overspending in months before the target (<= 0)
        let mut overspent_last_month = 0i64;
        let mut rows: HashMap<i64, PlanCategory> = HashMap::new();

        loop {
            let m = fmt_month(cursor);
            let is_target = m == target_s;
            let mut overspent_this_month = 0i64;
            for c in &cats {
                let act = activity.get(&(m.clone(), c.id)).copied().unwrap_or(0);
                if c.is_income {
                    total_income += act;
                    continue;
                }
                let asg = assigned.get(&(m.clone(), c.id)).copied().unwrap_or(0);
                total_assigned += asg;
                let available = carry.get(&c.id).copied().unwrap_or(0) + asg + act;
                if is_target {
                    rows.insert(
                        c.id,
                        PlanCategory { id: c.id, name: c.name.clone(), assigned_cents: asg, activity_cents: act, available_cents: available },
                    );
                } else {
                    carry.insert(c.id, available.max(0));
                    overspent_this_month += available.min(0);
                }
            }
            if is_target {
                break;
            }
            total_overspent_before += overspent_this_month;
            overspent_last_month = overspent_this_month;
            cursor = next_month(cursor);
        }

        let income_this_month: i64 = cats
            .iter()
            .filter(|c| income_ids.contains(&c.id))
            .map(|c| activity.get(&(target_s.clone(), c.id)).copied().unwrap_or(0))
            .sum();

        // Build visible groups (hidden categories only show if they still hold money).
        let mut groups: Vec<PlanGroup> = Vec::new();
        let group_names: HashMap<i64, String> = self
            .conn
            .prepare("SELECT id, name FROM category_groups")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for c in cats.iter().filter(|c| !c.is_income) {
            let row = rows.remove(&c.id).unwrap();
            if c.hidden && row.assigned_cents == 0 && row.activity_cents == 0 && row.available_cents == 0 {
                continue;
            }
            if groups.last().map(|g| g.id) != Some(c.group_id) {
                groups.push(PlanGroup {
                    id: c.group_id,
                    name: group_names.get(&c.group_id).cloned().unwrap_or_default(),
                    assigned_cents: 0,
                    activity_cents: 0,
                    available_cents: 0,
                    categories: Vec::new(),
                });
            }
            let g = groups.last_mut().unwrap();
            g.assigned_cents += row.assigned_cents;
            g.activity_cents += row.activity_cents;
            g.available_cents += row.available_cents;
            g.categories.push(row);
        }

        let assigned_this_month = groups.iter().map(|g| g.assigned_cents).sum();
        let uncategorized_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM transactions WHERE category_id IS NULL AND substr(date, 1, 7) = ?1",
            [&target_s],
            |r| r.get(0),
        )?;

        Ok(MonthPlan {
            month: target_s,
            ready_to_assign_cents: total_income - total_assigned + total_overspent_before,
            income_this_month_cents: income_this_month,
            assigned_this_month_cents: assigned_this_month,
            overspent_last_month_cents: overspent_last_month,
            groups,
            uncategorized_count,
        })
    }

    /// Set how much is assigned to a category in a month. Zero clears it.
    pub fn set_assigned(&self, month: &str, category_id: i64, cents: i64) -> Result<()> {
        let m = fmt_month(parse_month(month)?);
        if cents == 0 {
            self.conn
                .execute("DELETE FROM budget_assignments WHERE month = ?1 AND category_id = ?2", params![m, category_id])?;
        } else {
            self.conn.execute(
                "INSERT INTO budget_assignments (month, category_id, assigned_cents) VALUES (?1, ?2, ?3)
                 ON CONFLICT (month, category_id) DO UPDATE SET assigned_cents = excluded.assigned_cents",
                params![m, category_id, cents],
            )?;
        }
        Ok(())
    }

    /// Copy last month's assigned amounts into this month, for categories that have
    /// nothing assigned yet this month. Returns how many categories were filled in.
    pub fn copy_previous_assignments(&self, month: &str) -> Result<usize> {
        let target = parse_month(month)?;
        let n = self.conn.execute(
            "INSERT INTO budget_assignments (month, category_id, assigned_cents)
             SELECT ?1, p.category_id, p.assigned_cents FROM budget_assignments p
             JOIN categories c ON c.id = p.category_id AND c.hidden = 0
             WHERE p.month = ?2
             ON CONFLICT (month, category_id) DO NOTHING",
            params![fmt_month(target), fmt_month(prev_month(target))],
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AccountKind, NewTransaction};

    fn cat_id(b: &Budget, name: &str) -> i64 {
        b.list_category_groups().unwrap().into_iter().flat_map(|g| g.categories).find(|c| c.name == name).unwrap().id
    }

    fn txn(b: &Budget, acct: i64, date: &str, cents: i64, cat: Option<i64>) {
        b.add_transaction(&NewTransaction {
            account_id: acct,
            date: date.into(),
            payee: "x".into(),
            amount_cents: cents,
            category_id: cat,
            ..Default::default()
        })
        .unwrap();
    }

    fn cat<'a>(p: &'a MonthPlan, id: i64) -> &'a PlanCategory {
        p.groups.iter().flat_map(|g| &g.categories).find(|c| c.id == id).unwrap()
    }

    #[test]
    fn income_assign_spend_rollover_and_overspending() {
        let b = Budget::open_in_memory().unwrap();
        let acct = b.create_account("Checking", AccountKind::Checking).unwrap();
        let paycheck = cat_id(&b, "Paycheck");
        let groceries = cat_id(&b, "Groceries");
        let dining = cat_id(&b, "Restaurants");

        // October: $3,000 in. Assign $500 groceries, $100 dining. Spend $450 and $130.
        txn(&b, acct, "2026-10-01", 300_000, Some(paycheck));
        b.set_assigned("2026-10", groceries, 50_000).unwrap();
        b.set_assigned("2026-10", dining, 10_000).unwrap();
        txn(&b, acct, "2026-10-05", -45_000, Some(groceries));
        txn(&b, acct, "2026-10-06", -13_000, Some(dining));
        txn(&b, acct, "2026-10-07", -999, None); // uncategorized: ignored by the plan

        let oct = b.get_month_plan("2026-10").unwrap();
        assert_eq!(oct.ready_to_assign_cents, 300_000 - 60_000);
        assert_eq!(oct.income_this_month_cents, 300_000);
        assert_eq!(oct.assigned_this_month_cents, 60_000);
        assert_eq!(cat(&oct, groceries).available_cents, 5_000);
        assert_eq!(cat(&oct, dining).available_cents, -3_000);
        assert_eq!(oct.uncategorized_count, 1);

        // November: groceries carries $50 forward; dining starts at 0 and its
        // $30 overspend comes out of Ready to Assign.
        b.set_assigned("2026-11", groceries, 40_000).unwrap();
        let nov = b.get_month_plan("2026-11").unwrap();
        assert_eq!(cat(&nov, groceries).available_cents, 45_000);
        assert_eq!(cat(&nov, dining).available_cents, 0);
        assert_eq!(nov.overspent_last_month_cents, -3_000);
        assert_eq!(nov.ready_to_assign_cents, 300_000 - 60_000 - 40_000 - 3_000);
        assert_eq!(nov.income_this_month_cents, 0);

        // A month far in the future still rolls forward correctly.
        let later = b.get_month_plan("2027-02").unwrap();
        assert_eq!(cat(&later, groceries).available_cents, 45_000);
        assert_eq!(later.ready_to_assign_cents, nov.ready_to_assign_cents);
    }

    #[test]
    fn set_to_zero_clears_and_copy_previous() {
        let b = Budget::open_in_memory().unwrap();
        let groceries = cat_id(&b, "Groceries");
        let gas = cat_id(&b, "Gas");
        b.set_assigned("2026-10", groceries, 50_000).unwrap();
        b.set_assigned("2026-10", gas, 8_000).unwrap();
        b.set_assigned("2026-11", gas, 9_000).unwrap();

        assert_eq!(b.copy_previous_assignments("2026-11").unwrap(), 1);
        let nov = b.get_month_plan("2026-11").unwrap();
        assert_eq!(cat(&nov, groceries).assigned_cents, 50_000);
        assert_eq!(cat(&nov, gas).assigned_cents, 9_000); // kept, not overwritten

        b.set_assigned("2026-11", groceries, 0).unwrap();
        assert_eq!(cat(&b.get_month_plan("2026-11").unwrap(), groceries).assigned_cents, 0);
        assert!(b.set_assigned("Nov 2026", groceries, 1).is_err());
    }

    #[test]
    fn year_boundary() {
        let b = Budget::open_in_memory().unwrap();
        let groceries = cat_id(&b, "Groceries");
        b.set_assigned("2026-12", groceries, 1_000).unwrap();
        let jan = b.get_month_plan("2027-01").unwrap();
        assert_eq!(cat(&jan, groceries).available_cents, 1_000);
    }
}
