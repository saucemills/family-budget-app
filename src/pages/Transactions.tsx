import { useCallback, useEffect, useState } from "react";
import { api, Account, CategoryGroup, Transaction, errorText, formatMoney, parseMoney, today } from "../api";
import { CategorySelect, ConfirmButton, ErrorBanner } from "../components";

export default function TransactionsPage() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [groups, setGroups] = useState<CategoryGroup[]>([]);
  const [txns, setTxns] = useState<Transaction[]>([]);
  const [accountFilter, setAccountFilter] = useState<number | null>(null);
  const [search, setSearch] = useState("");
  const [uncategorizedOnly, setUncategorizedOnly] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [a, g, t] = await Promise.all([
        api.listAccounts(),
        api.listCategoryGroups(),
        api.listTransactions({ account_id: accountFilter, search, uncategorized_only: uncategorizedOnly, limit: 1000 }),
      ]);
      setAccounts(a);
      setGroups(g);
      setTxns(t);
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [accountFilter, search, uncategorizedOnly]);

  useEffect(() => {
    load();
  }, [load]);

  const setCategory = async (t: Transaction, categoryId: number | null) => {
    try {
      await api.setTransactionCategory(t.id, categoryId);
      await load();
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
        <input placeholder="Search payee or memo" value={search} onChange={(e) => setSearch(e.target.value)} />
        <label className="check">
          <input type="checkbox" checked={uncategorizedOnly} onChange={(e) => setUncategorizedOnly(e.target.checked)} />
          Only uncategorized
        </label>
        <span className="muted">
          {txns.length} shown{uncategorizedCount > 0 && `, ${uncategorizedCount} need a category`}
        </span>
      </div>

      <table className="txns">
        <thead>
          <tr>
            <th>Date</th>
            <th>Account</th>
            <th>Payee</th>
            <th>Category</th>
            <th className="num">Amount</th>
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
