import { useCallback, useEffect, useState } from "react";
import {
  api,
  bankApi,
  describeRange,
  describeSync,
  Account,
  CategoryGroup,
  SortBy,
  Transaction,
  errorText,
  formatMoney,
  parseMoney,
  today,
} from "../api";
import { CategorySelect, ConfirmButton, ErrorBanner } from "../components";

interface RulePrompt {
  text: string;
  categoryId: number;
  categoryName: string;
  /** Limit the rule to an amount range. */
  limit: boolean;
  min: string;
  max: string;
  /** The amount of the transaction just categorized, to pre-fill the range. */
  example: number;
}

/** "" = all, "none" = uncategorized, "g:5" = a whole group, "c:12" = one category. */
type CategoryChoice = string;

type DatePreset = "all" | "this_month" | "last_month" | "last_3_months" | "this_year" | "last_year" | "custom";

const DATE_PRESETS: { value: DatePreset; label: string }[] = [
  { value: "all", label: "All dates" },
  { value: "this_month", label: "This month" },
  { value: "last_month", label: "Last month" },
  { value: "last_3_months", label: "Last 3 months" },
  { value: "this_year", label: "This year" },
  { value: "last_year", label: "Last year" },
  { value: "custom", label: "Pick dates…" },
];

const ymd = (d: Date) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

/** The start and end dates (inclusive) of a preset, in local time. */
function presetRange(p: DatePreset): [string | null, string | null] {
  const now = new Date();
  const y = now.getFullYear();
  const m = now.getMonth();
  switch (p) {
    case "this_month":
      return [ymd(new Date(y, m, 1)), ymd(new Date(y, m + 1, 0))];
    case "last_month":
      return [ymd(new Date(y, m - 1, 1)), ymd(new Date(y, m, 0))];
    case "last_3_months":
      return [ymd(new Date(y, m - 2, 1)), ymd(new Date(y, m + 1, 0))];
    case "this_year":
      return [`${y}-01-01`, `${y}-12-31`];
    case "last_year":
      return [`${y - 1}-01-01`, `${y - 1}-12-31`];
    default:
      return [null, null];
  }
}

export default function TransactionsPage() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [groups, setGroups] = useState<CategoryGroup[]>([]);
  const [txns, setTxns] = useState<Transaction[]>([]);
  const [accountFilter, setAccountFilter] = useState<number | null>(null);
  const [search, setSearch] = useState("");
  const [categoryChoice, setCategoryChoice] = useState<CategoryChoice>("");
  const [direction, setDirection] = useState<"" | "in" | "out">("");
  const [datePreset, setDatePreset] = useState<DatePreset>("all");
  const [customStart, setCustomStart] = useState("");
  const [customEnd, setCustomEnd] = useState("");
  const [sort, setSort] = useState<SortBy>("date");
  const [ascending, setAscending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hasBanks, setHasBanks] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [rulePrompt, setRulePrompt] = useState<RulePrompt | null>(null);

  const [start, end] =
    datePreset === "custom" ? [customStart || null, customEnd || null] : presetRange(datePreset);

  const filtered =
    accountFilter != null || search.trim() !== "" || categoryChoice !== "" || direction !== "" || datePreset !== "all";
  const clearFilters = () => {
    setAccountFilter(null);
    setSearch("");
    setCategoryChoice("");
    setDirection("");
    setDatePreset("all");
  };

  /** Click a column header: sort by it, or flip the order if it's already the sort. */
  const sortBy = (col: SortBy) => {
    if (sort === col) setAscending(!ascending);
    else {
      setSort(col);
      // Dates and amounts read best biggest/newest first; names A to Z.
      setAscending(col === "payee" || col === "category" || col === "account");
    }
  };
  const SortHeader = ({ col, label, className }: { col: SortBy; label: string; className?: string }) => (
    <th className={["sortable", className].filter(Boolean).join(" ")} onClick={() => sortBy(col)} title="Sort">
      {label}
      {sort === col && <span className="sort-arrow">{ascending ? " ▲" : " ▼"}</span>}
    </th>
  );

  const load = useCallback(async () => {
    try {
      const [a, g, t] = await Promise.all([
        api.listAccounts(),
        api.listCategoryGroups(),
        api.listTransactions({
          account_id: accountFilter,
          search,
          uncategorized_only: categoryChoice === "none",
          category_id: categoryChoice.startsWith("c:") ? Number(categoryChoice.slice(2)) : null,
          group_id: categoryChoice.startsWith("g:") ? Number(categoryChoice.slice(2)) : null,
          direction: direction || null,
          start,
          end,
          sort,
          ascending,
          limit: 2000,
        }),
      ]);
      setAccounts(a);
      setGroups(g);
      setTxns(t);
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [accountFilter, search, categoryChoice, direction, start, end, sort, ascending]);

  useEffect(() => {
    load();
  }, [load]);
  useEffect(() => {
    bankApi.listItems().then((i) => setHasBanks(i.length > 0)).catch(() => {});
  }, []);

  const syncBanks = async () => {
    setSyncing(true);
    try {
      setNotice(describeSync(await bankApi.sync(null)));
      await load();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSyncing(false);
    }
  };

  const createRule = async () => {
    if (!rulePrompt) return;
    const min = rulePrompt.limit && rulePrompt.min.trim() ? parseMoney(rulePrompt.min) : null;
    const max = rulePrompt.limit && rulePrompt.max.trim() ? parseMoney(rulePrompt.max) : null;
    if ((rulePrompt.limit && rulePrompt.min.trim() && min == null) || (rulePrompt.limit && rulePrompt.max.trim() && max == null)) {
      setError("Enter amounts like 120 or 249.99");
      return;
    }
    try {
      const n = await bankApi.addRule(rulePrompt.text, rulePrompt.categoryId, min, max);
      const range = min != null || max != null ? ` (${describeRange(min && Math.abs(min), max && Math.abs(max))})` : "";
      setNotice(
        `Rule saved: payees containing "${rulePrompt.text}"${range} go to ${rulePrompt.categoryName}.` +
          (n ? ` It also updated ${n} past transaction${n === 1 ? "" : "s"}.` : ""),
      );
      setRulePrompt(null);
      await load();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const setCategory = async (t: Transaction, categoryId: number | null) => {
    try {
      await api.setTransactionCategory(t.id, categoryId);
      await load();
      if (categoryId != null) {
        const name = groups.flatMap((g) => g.categories).find((c) => c.id === categoryId)?.name ?? "";
        setRulePrompt({
          text: await bankApi.suggestRuleText(t.payee),
          categoryId,
          categoryName: name,
          limit: false,
          min: "",
          max: "",
          example: Math.abs(t.amount_cents),
        });
      }
    } catch (e) {
      setError(errorText(e));
    }
  };

  const remove = async (id: number) => {
    try {
      await api.deleteTransaction(id);
      await load();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const uncategorizedCount = txns.filter((t) => t.category_id == null).length;
  const totalOut = txns.reduce((s, t) => (t.amount_cents < 0 ? s + t.amount_cents : s), 0);
  const totalIn = txns.reduce((s, t) => (t.amount_cents > 0 ? s + t.amount_cents : s), 0);

  return (
    <div>
      <h1>Transactions</h1>
      <ErrorBanner error={error} />
      {accounts.length === 0 ? (
        <p className="empty">Add an account first (Accounts page), then import a CSV or add transactions here.</p>
      ) : (
        <AddTransaction accounts={accounts} groups={groups} onAdded={load} onError={setError} />
      )}

      <div className="toolbar">
        <select value={accountFilter ?? ""} onChange={(e) => setAccountFilter(e.target.value ? Number(e.target.value) : null)}>
          <option value="">All accounts</option>
          {accounts.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
        <input placeholder="Search payee or amount" value={search} onChange={(e) => setSearch(e.target.value)} />
        <select value={categoryChoice} onChange={(e) => setCategoryChoice(e.target.value)}>
          <option value="">All categories</option>
          <option value="none">Uncategorized</option>
          {groups.map((g) => (
            <optgroup key={g.id} label={g.name}>
              <option value={`g:${g.id}`}>All of {g.name}</option>
              {g.categories.map((c) => (
                <option key={c.id} value={`c:${c.id}`}>
                  {c.name}
                </option>
              ))}
            </optgroup>
          ))}
        </select>
        <select value={direction} onChange={(e) => setDirection(e.target.value as "" | "in" | "out")}>
          <option value="">Money in and out</option>
          <option value="out">Money out (spending)</option>
          <option value="in">Money in (deposits)</option>
        </select>
        <select value={datePreset} onChange={(e) => setDatePreset(e.target.value as DatePreset)}>
          {DATE_PRESETS.map((p) => (
            <option key={p.value} value={p.value}>
              {p.label}
            </option>
          ))}
        </select>
        {datePreset === "custom" && (
          <>
            <input type="date" value={customStart} onChange={(e) => setCustomStart(e.target.value)} aria-label="From" />
            to
            <input type="date" value={customEnd} onChange={(e) => setCustomEnd(e.target.value)} aria-label="To" />
          </>
        )}
        {filtered && (
          <button className="link" onClick={clearFilters}>
            Clear filters
          </button>
        )}
        {hasBanks && (
          <button disabled={syncing} onClick={syncBanks}>
            {syncing ? "Syncing…" : "Sync banks"}
          </button>
        )}
        <span className="muted">
          {txns.length} shown
          {totalOut !== 0 && ` · ${formatMoney(-totalOut)} out`}
          {totalIn !== 0 && ` · ${formatMoney(totalIn)} in`}
          {uncategorizedCount > 0 && ` · ${uncategorizedCount} need a category`}
        </span>
      </div>

      {notice && <div className="notice">{notice}</div>}
      {rulePrompt && (
        <div className="notice rule-prompt">
          Always put payees containing{" "}
          <input value={rulePrompt.text} onChange={(e) => setRulePrompt({ ...rulePrompt, text: e.target.value })} /> in{" "}
          <strong>{rulePrompt.categoryName}</strong>?{" "}
          <label className="check">
            <input
              type="checkbox"
              checked={rulePrompt.limit}
              onChange={(e) =>
                setRulePrompt({
                  ...rulePrompt,
                  limit: e.target.checked,
                  min: rulePrompt.min || (rulePrompt.example / 100).toFixed(2),
                  max: rulePrompt.max || (rulePrompt.example / 100).toFixed(2),
                })
              }
            />
            only for amounts
          </label>
          {rulePrompt.limit && (
            <>
              {" "}from $<input className="amount" value={rulePrompt.min} onChange={(e) => setRulePrompt({ ...rulePrompt, min: e.target.value })} />{" "}
              to $<input className="amount" value={rulePrompt.max} onChange={(e) => setRulePrompt({ ...rulePrompt, max: e.target.value })} />
            </>
          )}{" "}
          <button className="primary" onClick={createRule}>
            Make it a rule
          </button>{" "}
          <button onClick={() => setRulePrompt(null)}>No thanks</button>
        </div>
      )}

      <table className="txns">
        <thead>
          <tr>
            <SortHeader col="date" label="Date" />
            <SortHeader col="account" label="Account" />
            <SortHeader col="payee" label="Payee" />
            <SortHeader col="category" label="Category" />
            <SortHeader col="amount" label="Amount" className="num" />
            <th />
          </tr>
        </thead>
        <tbody>
          {txns.map((t) => (
            <tr key={t.id}>
              <td className="date">{t.date}</td>
              <td>{t.account_name}</td>
              <td>
                {t.payee}
                {t.memo && <div className="memo">{t.memo}</div>}
                {t.linked_transaction_id != null && (
                  <div className="memo linked" title="Matched automatically. Pick a non-transfer category to undo the match.">
                    ↔ {t.linked_account_name}: {t.linked_payee}
                  </div>
                )}
              </td>
              <td>
                <CategorySelect groups={groups} value={t.category_id} onChange={(id) => setCategory(t, id)} />
              </td>
              <td className={t.amount_cents < 0 ? "num out" : "num in"}>{formatMoney(t.amount_cents)}</td>
              <td>
                <ConfirmButton label="Delete" confirmLabel="Sure?" onConfirm={() => remove(t.id)} />
              </td>
            </tr>
          ))}
          {txns.length === 0 && accounts.length > 0 && (
            <tr>
              <td colSpan={6} className="empty">
                No transactions match. Try the Import page to bring in a bank CSV.
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

function AddTransaction(props: {
  accounts: Account[];
  groups: CategoryGroup[];
  onAdded: () => void;
  onError: (e: string | null) => void;
}) {
  const [accountId, setAccountId] = useState<number>(props.accounts[0].id);
  const [date, setDate] = useState(today());
  const [payee, setPayee] = useState("");
  const [amount, setAmount] = useState("");
  const [direction, setDirection] = useState<"out" | "in">("out");
  const [categoryId, setCategoryId] = useState<number | null>(null);

  useEffect(() => {
    if (!props.accounts.some((a) => a.id === accountId)) setAccountId(props.accounts[0].id);
  }, [props.accounts, accountId]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const cents = parseMoney(amount);
    if (cents == null || cents === 0) return props.onError("Enter an amount, like 12.34");
    try {
      await api.addTransaction({
        account_id: accountId,
        date,
        payee,
        memo: "",
        amount_cents: direction === "out" ? -Math.abs(cents) : Math.abs(cents),
        category_id: categoryId,
      });
      setPayee("");
      setAmount("");
      props.onError(null);
      props.onAdded();
    } catch (err) {
      props.onError(errorText(err));
    }
  };

  return (
    <form className="add-row" onSubmit={submit}>
      <input type="date" value={date} onChange={(e) => setDate(e.target.value)} required />
      <select value={accountId} onChange={(e) => setAccountId(Number(e.target.value))}>
        {props.accounts.map((a) => (
          <option key={a.id} value={a.id}>
            {a.name}
          </option>
        ))}
      </select>
      <input placeholder="Payee" value={payee} onChange={(e) => setPayee(e.target.value)} />
      <CategorySelect groups={props.groups} value={categoryId} onChange={setCategoryId} />
      <select value={direction} onChange={(e) => setDirection(e.target.value as "out" | "in")}>
        <option value="out">Spent</option>
        <option value="in">Received</option>
      </select>
      <input className="amount" placeholder="0.00" value={amount} onChange={(e) => setAmount(e.target.value)} />
      <button type="submit" className="primary">
        Add
      </button>
    </form>
  );
}
