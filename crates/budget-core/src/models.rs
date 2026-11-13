use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Checking,
    Savings,
    CreditCard,
    Cash,
}

impl AccountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountKind::Checking => "checking",
            AccountKind::Savings => "savings",
            AccountKind::CreditCard => "credit_card",
            AccountKind::Cash => "cash",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "checking" => AccountKind::Checking,
            "savings" => AccountKind::Savings,
            "credit_card" => AccountKind::CreditCard,
            "cash" => AccountKind::Cash,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub kind: AccountKind,
    /// Sum of all transactions in the account, in cents.
    pub balance_cents: i64,
    pub transaction_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: i64,
    pub group_id: i64,
    pub name: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryGroup {
    pub id: i64,
    pub name: String,
    /// Income groups are where paychecks go; the Sankey uses them as sources.
    pub is_income: bool,
    pub sort_order: i64,
    pub categories: Vec<Category>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: i64,
    pub account_id: i64,
    pub account_name: String,
    /// ISO date, `YYYY-MM-DD`.
    pub date: String,
    pub payee: String,
    pub memo: String,
    /// Negative is money out, positive is money in.
    pub amount_cents: i64,
    pub category_id: Option<i64>,
    pub category_name: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewTransaction {
    pub account_id: i64,
    pub date: String,
    pub payee: String,
    #[serde(default)]
    pub memo: String,
    pub amount_cents: i64,
    #[serde(default)]
    pub category_id: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransactionFilter {
    #[serde(default)]
    pub account_id: Option<i64>,
    /// Only transactions with no category yet.
    #[serde(default)]
    pub uncategorized_only: bool,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}
