import { useCallback, useEffect, useState } from "react";
import { api, Account, AccountKind, ACCOUNT_KINDS, errorText, formatMoney } from "../api";
import { ConfirmButton, ErrorBanner } from "../components";

export default function AccountsPage() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<AccountKind>("checking");
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setAccounts(await api.listAccounts());
    } catch (e) {
      setError(errorText(e));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  const add = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      await api.createAccount(name, kind);
      setName("");
      setError(null);
      load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const remove = async (id: number) => {
    try {
      await api.deleteAccount(id);
      load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const total = accounts.reduce((s, a) => s + a.balance_cents, 0);
  const kindLabel = (k: AccountKind) => ACCOUNT_KINDS.find((x) => x.value === k)?.label ?? k;

  return (
    <div>
      <h1>Accounts</h1>
      <ErrorBanner error={error} />
      <form className="add-row" onSubmit={add}>
        <input placeholder="Account name, e.g. Joint Checking" value={name} onChange={(e) => setName(e.target.value)} />
        <select value={kind} onChange={(e) => setKind(e.target.value as AccountKind)}>
          {ACCOUNT_KINDS.map((k) => (
            <option key={k.value} value={k.value}>
              {k.label}
            </option>
          ))}
        </select>
        <button type="submit" className="primary">
          Add account
        </button>
      </form>

      <table>
        <thead>
          <tr>
            <th>Account</th>
            <th>Type</th>
            <th className="num">Transactions</th>
            <th className="num">Balance</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {accounts.map((a) => (
            <tr key={a.id}>
              <td>{a.name}</td>
              <td>{kindLabel(a.kind)}</td>
              <td className="num">{a.transaction_count}</td>
              <td className={a.balance_cents < 0 ? "num out" : "num"}>{formatMoney(a.balance_cents)}</td>
              <td>
                <ConfirmButton
                  label="Delete"
                  confirmLabel={`Delete with ${a.transaction_count} transactions?`}
                  onConfirm={() => remove(a.id)}
                />
              </td>
            </tr>
          ))}
          {accounts.length === 0 && (
            <tr>
              <td colSpan={5} className="empty">
                No accounts yet. Add your checking account and credit cards above.
              </td>
            </tr>
          )}
        </tbody>
        {accounts.length > 1 && (
          <tfoot>
            <tr>
              <td colSpan={3}>Net worth across accounts</td>
              <td className="num">{formatMoney(total)}</td>
              <td />
            </tr>
          </tfoot>
        )}
      </table>
      <p className="muted">
        Balances are the sum of transactions in the app. If you start mid-way through an account's history, add one
        "Starting balance" transaction so the balance matches your bank.
      </p>
    </div>
  );
}
