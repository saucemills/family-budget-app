//! Import transactions from the CSV files banks let you download.
//!
//! Every bank lays its CSV out differently, so import is two steps:
//! [`preview_csv`] reads the file and guesses which column is which, the person
//! checks or fixes that guess in the app, and [`Budget::import_csv`] does the import.
//! Re-importing the same file, or an overlapping date range, skips rows already imported.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::Budget;
use crate::error::{invalid, Result};
use crate::money::parse_amount;

/// Date layouts seen in US bank exports, tried in this order when guessing.
pub const DATE_FORMATS: &[&str] = &["%m/%d/%Y", "%Y-%m-%d", "%m/%d/%y", "%m-%d-%Y", "%Y/%m/%d", "%d/%m/%Y"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AmountColumns {
    /// One signed column, e.g. `Amount`.
    Single { column: usize },
    /// Separate money-out and money-in columns, e.g. `Debit` and `Credit`.
    Split { outflow: usize, inflow: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CsvMapping {
    pub has_header: bool,
    pub date_column: usize,
    /// A chrono format string such as `%m/%d/%Y`.
    pub date_format: String,
    pub payee_column: usize,
    pub memo_column: Option<usize>,
    pub amount: AmountColumns,
    /// Some credit card exports show purchases as positive numbers. Set this to flip them.
    pub flip_sign: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsvPreview {
    pub headers: Vec<String>,
    /// The first few data rows, for showing the person what the file looks like.
    pub sample_rows: Vec<Vec<String>>,
    pub total_rows: usize,
    /// Our best guess at the mapping, or `None` if we couldn't find a date and amount.
    pub suggested: Option<CsvMapping>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported: usize,
    pub skipped_duplicates: usize,
    /// Rows that couldn't be read, with their line number and the reason.
    pub errors: Vec<String>,
}

fn read_records(content: &str) -> Result<Vec<Vec<String>>> {
    // Some banks put a byte-order mark at the start of the file.
    let content = content.trim_start_matches('\u{feff}');
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(content.as_bytes());
    let mut rows = Vec::new();
    for rec in reader.records() {
        let rec = rec?;
        let row: Vec<String> = rec.iter().map(str::to_string).collect();
        if row.iter().all(|c| c.is_empty()) {
            continue;
        }
        rows.push(row);
    }
    Ok(rows)
}

pub fn preview_csv(content: &str) -> Result<CsvPreview> {
    let rows = read_records(content)?;
    if rows.is_empty() {
        return invalid("the file is empty");
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let first_row_has_date = rows[0].iter().any(|c| guess_date_format(&[c.as_str()]).is_some());
    let has_header = !first_row_has_date;
    let (headers, data) = if has_header {
        let mut h = rows[0].clone();
        h.resize(width, String::new());
        (h, &rows[1..])
    } else {
        ((1..=width).map(|i| format!("Column {i}")).collect(), &rows[..])
    };
    let suggested = guess_mapping(&headers, data, has_header);
    Ok(CsvPreview {
        headers,
        sample_rows: data.iter().take(8).cloned().collect(),
        total_rows: data.len(),
        suggested,
    })
}

fn find_header(headers: &[String], names: &[&str]) -> Option<usize> {
    let norm: Vec<String> = headers.iter().map(|h| h.to_lowercase().replace(['.', '_'], " ").trim().to_string()).collect();
    // Exact matches first, then "contains", so "Amount" beats "Original Amount".
    for name in names {
        if let Some(i) = norm.iter().position(|h| h == name) {
            return Some(i);
        }
    }
    for name in names {
        if let Some(i) = norm.iter().position(|h| h.contains(name)) {
            return Some(i);
        }
    }
    None
}

fn column_values<'a>(data: &'a [Vec<String>], col: usize) -> Vec<&'a str> {
    data.iter()
        .filter_map(|r| r.get(col).map(String::as_str))
        .filter(|s| !s.is_empty())
        .take(50)
        .collect()
}

fn guess_date_format(values: &[&str]) -> Option<&'static str> {
    if values.is_empty() {
        return None;
    }
    DATE_FORMATS
        .iter()
        .copied()
        .find(|fmt| values.iter().all(|v| parse_date(v, fmt).is_some()))
}

/// Parse a bank date, ignoring any time of day after it (e.g. `11/02/2026 13:45`).
fn parse_date(value: &str, fmt: &str) -> Option<NaiveDate> {
    let first = value.split_whitespace().next().unwrap_or("");
    NaiveDate::parse_from_str(first, fmt).ok().filter(|d| {
        // Two-digit years like "26" parse as year 26 with %Y; reject those.
        (1900..=2200).contains(&chrono::Datelike::year(d))
    })
}

fn guess_mapping(headers: &[String], data: &[Vec<String>], has_header: bool) -> Option<CsvMapping> {
    let is_amount_col = |c: usize| {
        let v = column_values(data, c);
        !v.is_empty() && v.iter().all(|s| parse_amount(s).is_ok())
    };

    let date_column = if has_header {
        find_header(headers, &["date", "transaction date", "trans date", "posted date", "posting date", "post date"])
    } else {
        None
    }
    .or_else(|| (0..headers.len()).find(|&c| guess_date_format(&column_values(data, c)).is_some()))?;
    let date_format = guess_date_format(&column_values(data, date_column))?.to_string();

    let amount = if has_header {
        let single = find_header(headers, &["amount", "transaction amount"]).filter(|&c| is_amount_col(c));
        let out = find_header(headers, &["debit", "withdrawal", "withdrawals", "outflow", "money out"]);
        let inn = find_header(headers, &["credit", "deposit", "deposits", "inflow", "money in"]);
        match (single, out, inn) {
            (Some(c), _, _) => Some(AmountColumns::Single { column: c }),
            (None, Some(o), Some(i)) => Some(AmountColumns::Split { outflow: o, inflow: i }),
            _ => None,
        }
    } else {
        None
    }
    .or_else(|| {
        // No helpful header: use the last column that's all amounts (balance columns are
        // usually last too, so prefer a column that has some negatives).
        let cols: Vec<usize> = (0..headers.len()).filter(|&c| c != date_column && is_amount_col(c)).collect();
        cols.iter()
            .copied()
            .find(|&c| column_values(data, c).iter().any(|s| parse_amount(s).map(|v| v < 0).unwrap_or(false)))
            .or(cols.first().copied())
            .map(|column| AmountColumns::Single { column })
    })?;

    let used = |c: usize| {
        c == date_column
            || match amount {
                AmountColumns::Single { column } => c == column,
                AmountColumns::Split { outflow, inflow } => c == outflow || c == inflow,
            }
    };
    let payee_column = if has_header {
        find_header(headers, &["payee", "description", "merchant", "name", "original description", "details"])
    } else {
        None
    }
    .or_else(|| {
        // The text column with the longest values is usually the description.
        (0..headers.len())
            .filter(|&c| !used(c) && !is_amount_col(c))
            .max_by_key(|&c| column_values(data, c).iter().map(|s| s.len()).sum::<usize>())
    })?;
    let memo_column = if has_header { find_header(headers, &["memo", "notes", "note"]) } else { None }
        .filter(|&c| c != payee_column && !used(c));

    Some(CsvMapping { has_header, date_column, date_format, payee_column, memo_column, amount, flip_sign: false })
}

impl Budget {
    pub fn import_csv(&mut self, account_id: i64, content: &str, mapping: &CsvMapping) -> Result<ImportResult> {
        if !self.account_exists(account_id)? {
            return invalid("choose an account to import into");
        }
        let rows = read_records(content)?;
        let skip = usize::from(mapping.has_header);
        let mut result = ImportResult::default();
        // Identical rows (two $5 coffees on the same day) are told apart by how many
        // times the same row has appeared so far in this file.
        let mut seen: HashMap<String, u32> = HashMap::new();

        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO transactions
                   (account_id, date, payee, memo, amount_cents, source, import_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'csv', ?6)",
            )?;
            for (i, row) in rows.iter().enumerate().skip(skip) {
                let line = i + 1;
                match parse_row(row, mapping) {
                    Ok((date, payee, memo, amount)) => {
                        let key = format!("{date}|{amount}|{payee}|{memo}");
                        let n = seen.entry(key.clone()).or_insert(0);
                        *n += 1;
                        let import_id = hex::encode(Sha256::digest(format!("{key}|{n}")));
                        let inserted = insert.execute(rusqlite::params![
                            account_id, date, payee, memo, amount, import_id
                        ])?;
                        if inserted == 1 {
                            result.imported += 1;
                        } else {
                            result.skipped_duplicates += 1;
                        }
                    }
                    Err(e) => result.errors.push(format!("line {line}: {e}")),
                }
            }
        }
        tx.commit()?;
        self.apply_rules()?;
        Ok(result)
    }
}

fn parse_row(row: &[String], m: &CsvMapping) -> Result<(String, String, String, i64)> {
    let cell = |c: usize| row.get(c).map(String::as_str).unwrap_or("");
    let raw_date = cell(m.date_column);
    let date = parse_date(raw_date, &m.date_format)
        .ok_or_else(|| crate::Error::Invalid(format!("can't read date {raw_date:?}")))?
        .format("%Y-%m-%d")
        .to_string();
    let amount = match m.amount {
        AmountColumns::Single { column } => parse_amount(cell(column))?,
        AmountColumns::Split { outflow, inflow } => {
            let out = cell(outflow);
            let inn = cell(inflow);
            if out.is_empty() && inn.is_empty() {
                return invalid("no amount");
            }
            let out = if out.is_empty() { 0 } else { parse_amount(out)?.abs() };
            let inn = if inn.is_empty() { 0 } else { parse_amount(inn)?.abs() };
            inn - out
        }
    };
    let amount = if m.flip_sign { -amount } else { amount };
    let payee = cell(m.payee_column).to_string();
    let memo = m.memo_column.map(|c| cell(c).to_string()).unwrap_or_default();
    Ok((date, payee, memo, amount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AccountKind, TransactionFilter};

    const CHASE_STYLE: &str = "\u{feff}Transaction Date,Post Date,Description,Category,Type,Amount,Memo
11/02/2026,11/03/2026,STARBUCKS #123,Food & Drink,Sale,-5.25,
11/02/2026,11/03/2026,STARBUCKS #123,Food & Drink,Sale,-5.25,
11/03/2026,11/04/2026,TRADER JOE'S,Groceries,Sale,-82.10,weekly shop
11/05/2026,11/05/2026,PAYMENT THANK YOU,,Payment,500.00,
";

    const SPLIT_STYLE: &str = "Date,Description,Withdrawals,Deposits,Balance
2026-11-01,ACME PAYROLL,,\"2,500.00\",3000.00
2026-11-02,CITY WATER,64.12,,2935.88
";

    const NO_HEADER: &str = "\"11/01/26\",\"-12.00\",\"*\",\"\",\"NETFLIX.COM\"
\"11/02/26\",\"1500.00\",\"*\",\"\",\"DIRECT DEP EMPLOYER\"
";

    #[test]
    fn guesses_single_amount_layout() {
        let p = preview_csv(CHASE_STYLE).unwrap();
        assert_eq!(p.total_rows, 4);
        let m = p.suggested.unwrap();
        assert!(m.has_header);
        assert_eq!(p.headers[m.date_column], "Transaction Date");
        assert_eq!(p.headers[m.payee_column], "Description");
        assert_eq!(m.memo_column.map(|c| p.headers[c].as_str()), Some("Memo"));
        assert_eq!(m.amount, AmountColumns::Single { column: 5 });
        assert_eq!(m.date_format, "%m/%d/%Y");
    }

    #[test]
    fn guesses_split_amount_layout() {
        let m = preview_csv(SPLIT_STYLE).unwrap().suggested.unwrap();
        assert_eq!(m.amount, AmountColumns::Split { outflow: 2, inflow: 3 });
        assert_eq!(m.date_format, "%Y-%m-%d");
    }

    #[test]
    fn guesses_headerless_layout() {
        let p = preview_csv(NO_HEADER).unwrap();
        let m = p.suggested.unwrap();
        assert!(!m.has_header);
        assert_eq!(p.total_rows, 2);
        assert_eq!(m.date_column, 0);
        assert_eq!(m.date_format, "%m/%d/%y");
        assert_eq!(m.amount, AmountColumns::Single { column: 1 });
        assert_eq!(m.payee_column, 4);
    }

    #[test]
    fn imports_and_skips_duplicates_on_reimport() {
        let mut b = Budget::open_in_memory().unwrap();
        let acct = b.create_account("Visa", AccountKind::CreditCard).unwrap();
        let m = preview_csv(CHASE_STYLE).unwrap().suggested.unwrap();

        let r = b.import_csv(acct, CHASE_STYLE, &m).unwrap();
        assert_eq!((r.imported, r.skipped_duplicates, r.errors.len()), (4, 0, 0));

        let again = b.import_csv(acct, CHASE_STYLE, &m).unwrap();
        assert_eq!((again.imported, again.skipped_duplicates), (0, 4));

        let txns = b.list_transactions(&TransactionFilter::default()).unwrap();
        assert_eq!(txns.len(), 4);
        assert_eq!(txns.iter().filter(|t| t.payee == "STARBUCKS #123").count(), 2);
        let shop = txns.iter().find(|t| t.payee == "TRADER JOE'S").unwrap();
        assert_eq!((shop.date.as_str(), shop.amount_cents, shop.memo.as_str()), ("2026-11-03", -8210, "weekly shop"));
        assert_eq!(b.list_accounts().unwrap()[0].balance_cents, 50000 - 525 - 525 - 8210);
    }

    #[test]
    fn split_columns_and_flip_sign() {
        let mut b = Budget::open_in_memory().unwrap();
        let acct = b.create_account("Checking", AccountKind::Checking).unwrap();
        let mut m = preview_csv(SPLIT_STYLE).unwrap().suggested.unwrap();
        b.import_csv(acct, SPLIT_STYLE, &m).unwrap();
        assert_eq!(b.list_accounts().unwrap()[0].balance_cents, 250_000 - 6_412);

        let other = b.create_account("Flipped", AccountKind::CreditCard).unwrap();
        m.flip_sign = true;
        b.import_csv(other, SPLIT_STYLE, &m).unwrap();
        let flipped = b.list_accounts().unwrap().into_iter().find(|a| a.id == other).unwrap();
        assert_eq!(flipped.balance_cents, -(250_000 - 6_412));
    }

    #[test]
    fn bad_rows_are_reported_not_fatal() {
        let mut b = Budget::open_in_memory().unwrap();
        let acct = b.create_account("Checking", AccountKind::Checking).unwrap();
        let csv = "Date,Description,Amount\n11/01/2026,OK,-1.00\nnot a date,BAD,-2.00\n11/03/2026,BAD AMT,abc\n";
        let m = preview_csv("Date,Description,Amount\n11/01/2026,OK,-1.00\n").unwrap().suggested.unwrap();
        let r = b.import_csv(acct, csv, &m).unwrap();
        assert_eq!(r.imported, 1);
        assert_eq!(r.errors.len(), 2);
        assert!(r.errors[0].starts_with("line 3"));
    }
}
