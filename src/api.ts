// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
import { invoke } from "@tauri-apps/api/core";

export type AccountKind = "checking" | "savings" | "credit_card" | "cash";

export const ACCOUNT_KINDS: { value: AccountKind; label: string }[] = [
  { value: "checking", label: "Checking" },
  { value: "savings", label: "Savings" },
  { value: "credit_card", label: "Credit card" },
  { value: "cash", label: "Cash" },
];

export interface Account {
  id: number;
  name: string;
  kind: AccountKind;
  balance_cents: number;
  transaction_count: number;
}

export interface Category {
  id: number;
  group_id: number;
  name: string;
  sort_order: number;
}

export interface CategoryGroup {
  id: number;
  name: string;
  is_income: boolean;
  sort_order: number;
  categories: Category[];
}

export interface Transaction {
  id: number;
  account_id: number;
  account_name: string;
  date: string;
  payee: string;
  memo: string;
  amount_cents: number;
  category_id: number | null;
  category_name: string | null;
  source: string;
}

export interface NewTransaction {
  account_id: number;
  date: string;
  payee: string;
  memo: string;
  amount_cents: number;
  category_id: number | null;
}

export interface TransactionFilter {
  account_id?: number | null;
  uncategorized_only?: boolean;
  search?: string | null;
  limit?: number | null;
}

export type AmountColumns =
  | { kind: "single"; column: number }
  | { kind: "split"; outflow: number; inflow: number };

export interface CsvMapping {
  has_header: boolean;
  date_column: number;
  date_format: string;
  payee_column: number;
  memo_column: number | null;
  amount: AmountColumns;
  flip_sign: boolean;
}

export interface CsvPreview {
  headers: string[];
  sample_rows: string[][];
  total_rows: number;
  suggested: CsvMapping | null;
}

export interface ImportResult {
  imported: number;
  skipped_duplicates: number;
  errors: string[];
}

export const api = {
  listAccounts: () => invoke<Account[]>("list_accounts"),
  createAccount: (name: string, kind: AccountKind) => invoke<number>("create_account", { name, kind }),
  deleteAccount: (id: number) => invoke<void>("delete_account", { id }),

  listCategoryGroups: () => invoke<CategoryGroup[]>("list_category_groups"),
  createCategoryGroup: (name: string, isIncome: boolean) =>
    invoke<number>("create_category_group", { name, isIncome }),
  createCategory: (groupId: number, name: string) => invoke<number>("create_category", { groupId, name }),
  renameCategory: (id: number, name: string) => invoke<void>("rename_category", { id, name }),
  hideCategory: (id: number) => invoke<void>("hide_category", { id }),

  listTransactions: (filter: TransactionFilter) => invoke<Transaction[]>("list_transactions", { filter }),
  addTransaction: (txn: NewTransaction) => invoke<number>("add_transaction", { txn }),
  setTransactionCategory: (id: number, categoryId: number | null) =>
    invoke<void>("set_transaction_category", { id, categoryId }),
  deleteTransaction: (id: number) => invoke<void>("delete_transaction", { id }),

  previewCsv: (content: string) => invoke<CsvPreview>("preview_csv", { content }),
  importCsv: (accountId: number, content: string, mapping: CsvMapping) =>
    invoke<ImportResult>("import_csv", { accountId, content, mapping }),

  databasePath: () => invoke<string>("database_path"),
};

const money = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
export const formatMoney = (cents: number) => money.format(cents / 100);

/** Parse what a person types ("12.34", "$1,200") into cents, or null if it isn't a number. */
export function parseMoney(input: string): number | null {
  const cleaned = input.replace(/[$,\s]/g, "");
  if (!/^-?\d*(\.\d{0,2})?$/.test(cleaned) || cleaned === "" || cleaned === "-" || cleaned === ".") return null;
  return Math.round(parseFloat(cleaned) * 100);
}

export const today = () => {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};

export const errorText = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

// ---------- Plan ----------

export interface PlanCategory {
  id: number;
  name: string;
  assigned_cents: number;
  activity_cents: number;
  available_cents: number;
}

export interface PlanGroup {
  id: number;
  name: string;
  assigned_cents: number;
  activity_cents: number;
  available_cents: number;
  categories: PlanCategory[];
}

export interface MonthPlan {
  month: string;
  ready_to_assign_cents: number;
  income_this_month_cents: number;
  assigned_this_month_cents: number;
  overspent_last_month_cents: number;
  groups: PlanGroup[];
  uncategorized_count: number;
}

// ---------- Reflect ----------

export interface ReflectSummary {
  start: string;
  end: string;
  income_cents: number;
  spending_cents: number;
  income_sources: { category_id: number; name: string; amount_cents: number }[];
  spending_groups: {
    group_id: number;
    name: string;
    sort_order: number;
    amount_cents: number;
    categories: { category_id: number; name: string; amount_cents: number }[];
  }[];
  uncategorized_count: number;
  uncategorized_out_cents: number;
  color_order: number[];
}

export const planApi = {
  getMonthPlan: (month: string) => invoke<MonthPlan>("get_month_plan", { month }),
  setAssigned: (month: string, categoryId: number, cents: number) =>
    invoke<void>("set_assigned", { month, categoryId, cents }),
  copyPreviousAssignments: (month: string) => invoke<number>("copy_previous_assignments", { month }),
  reflect: (start: string, end: string) => invoke<ReflectSummary>("reflect", { start, end }),
};

export const isoDate = (d: Date) => {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};

export const thisMonth = () => today().slice(0, 7);

export function shiftMonth(month: string, delta: number): string {
  const [y, m] = month.split("-").map(Number);
  const d = new Date(y, m - 1 + delta, 1);
  return isoDate(d).slice(0, 7);
}

export const monthLabel = (month: string) => {
  const [y, m] = month.split("-").map(Number);
  return new Date(y, m - 1, 1).toLocaleDateString("en-US", { month: "long", year: "numeric" });
};
