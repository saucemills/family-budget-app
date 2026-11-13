use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Checking,
    Savings,
    CreditCard,
    Cash,
    /// Brokerage, retirement, HSA: tracked for its balance, not budgeted.
    Investment,
    /// Car loan, mortgage, student loan.
    Loan,
}

impl AccountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountKind::Checking => "checking",
            AccountKind::Savings => "savings",
            AccountKind::CreditCard => "credit_card",
            AccountKind::Cash => "cash",
            AccountKind::Investment => "investment",
            AccountKind::Loan => "loan",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "checking" => AccountKind::Checking,
            "savings" => AccountKind::Savings,
            "credit_card" => AccountKind::CreditCard,
            "cash" => AccountKind::Cash,
            "investment" => AccountKind::Investment,
            "loan" => AccountKind::Loan,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub kind: AccountKind,
    /// The bank's own balance when the account syncs with a bank, otherwise the sum of
    /// its transactions. In cents; negative for money owed (credit cards, loans).
    pub balance_cents: i64,
    /// True when `balance_cents` came from the bank.
    pub balance_from_bank: bool,
    /// When the bank balance was last fetched (UTC, `YYYY-MM-DD HH:MM:SS`).
    pub balance_as_of: Option<String>,
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
    /// Transfer groups hold money moving between the family's own accounts. Plan and
    /// Reflect leave them out, since that money wasn't earned or spent.
    pub is_transfer: bool,
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
    /// The other side of a transfer, or the purchase a PayPal-style payment paid for.
    pub linked_transaction_id: Option<i64>,
    pub linked_payee: Option<String>,
    pub linked_account_name: Option<String>,
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
