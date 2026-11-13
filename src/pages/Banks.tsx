import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  bankApi,
  describeSync,
  errorText,
  Account,
  ItemSync,
  NewConnection,
  PlaidEnv,
  PlaidItem,
  PlaidSettings,
} from "../api";
import { ConfirmButton, ErrorBanner } from "../components";

const POLL_MS = 3000;
const POLL_LIMIT_MS = 30 * 60 * 1000;

export default function BanksPage() {
  const [settings, setSettings] = useState<PlaidSettings | null>(null);
  const [items, setItems] = useState<PlaidItem[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [waiting, setWaiting] = useState(false);
  const [pending, setPending] = useState<NewConnection[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [lastSync, setLastSync] = useState<ItemSync[]>([]);
  const cancelled = useRef(false);

  const load = useCallback(async () => {
    try {
      const [s, i, a] = await Promise.all([bankApi.getSettings(), bankApi.listItems(), api.listAccounts()]);
      setSettings(s);
      setItems(i);
      setAccounts(a);
    } catch (e) {
      setError(errorText(e));
    }
  }, []);
  useEffect(() => {
    load();
    return () => {
      cancelled.current = true;
    };
  }, [load]);

  const sync = async (item: number | null) => {
    setSyncing(true);
    setError(null);
    try {
      const r = await bankApi.sync(item);
      setLastSync(r);
      setMessage(describeSync(r));
      await load();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSyncing(false);
    }
  };

  const connect = async (repairItem: number | null) => {
    setError(null);
    setMessage(null);
    cancelled.current = false;
    try {
      const token = await bankApi.connectStart(repairItem);
      setWaiting(true);
      const started = Date.now();
      while (!cancelled.current && Date.now() - started < POLL_LIMIT_MS) {
        await new Promise((r) => setTimeout(r, POLL_MS));
        if (cancelled.current) break;
        const res = await bankApi.connectPoll(token);
        if (res.status === "waiting") continue;
        if (res.status === "exited") {
          setMessage("The bank sign-in was closed before it finished. Nothing was connected.");
        } else if (repairItem != null) {
          setMessage("Reconnected. Syncing now…");
          await sync(repairItem);
        } else {
          setPending(res.connections);
        }
        break;
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      setWaiting(false);
      load();
    }
  };

  const connectSandbox = async () => {
    setError(null);
    try {
      setPending([await bankApi.sandboxConnect()]);
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <div>
      <h1>Bank sync</h1>
      <p className="muted">
        Connect your banks and cards through Plaid. Your bank sign-in happens on Plaid's own page in your browser; this app
        only keeps a connection token, stored in Windows Credential Manager with your Plaid keys.
      </p>
      <ErrorBanner error={error} />
      {message && <div className="notice">{message}</div>}

      <PlaidKeys settings={settings} onSaved={load} onError={setError} />

      {settings?.configured && (
        <>
          <div className="add-row">
            <button className="primary" disabled={waiting} onClick={() => connect(null)}>
              Connect a bank
            </button>
            {settings.env === "sandbox" && (
              <button onClick={connectSandbox} title="Plaid's fake bank. Sign-in: user_good / pass_good">
                Connect test bank
              </button>
            )}
            {items.length > 0 && (
              <button disabled={syncing} onClick={() => sync(null)}>
                {syncing ? "Syncing…" : "Sync all"}
              </button>
            )}
          </div>
          {waiting && (
            <div className="notice">
              Finish signing in to your bank in the browser window that just opened. This page will continue on its own.{" "}
              <button className="link" onClick={() => (cancelled.current = true)}>
                Cancel
              </button>
            </div>
          )}
        </>
      )}

      {pending.map((c) => (
        <AccountMatcher
          key={c.item}
          connection={c}
          accounts={accounts}
          onDone={async () => {
            setPending((p) => p.filter((x) => x.item !== c.item));
            await sync(c.item);
          }}
          onError={setError}
        />
      ))}

      {items.length > 0 && <h2>Connected banks</h2>}
      <div className="groups">
        {items.map((it) => {
          const status = lastSync.find((s) => s.item === it.id);
          const needsReconnect = status?.needs_reconnect ?? false;
          return (
            <section className="group" key={it.id}>
              <h2>{it.institution_name}</h2>
              <ul className="plain">
                {it.accounts.map((a) => (
                  <li key={a.id}>
                    {a.name}
                    {a.plaid_since && <span className="muted"> (from {a.plaid_since})</span>}
                  </li>
                ))}
              </ul>
              <div className="muted small">
                {it.last_synced_at ? `Last synced ${new Date(it.last_synced_at + "Z").toLocaleString()}` : "Not synced yet"}
              </div>
              {it.last_error && <div className="error small">{it.last_error}</div>}
              <div className="row-buttons">
                <button disabled={syncing} onClick={() => sync(it.id)}>
                  Sync
                </button>
                {(needsReconnect || it.last_error) && (
                  <button disabled={waiting} onClick={() => connect(it.id)}>
                    Sign in again
                  </button>
                )}
                <ConfirmButton
                  label="Disconnect"
                  confirmLabel="Disconnect? (keeps history)"
                  onConfirm={async () => {
                    try {
                      await bankApi.disconnect(it.id);
                      load();
                    } catch (e) {
                      setError(errorText(e));
                    }
                  }}
                />
              </div>
            </section>
          );
        })}
      </div>
    </div>
  );
}

function PlaidKeys(props: { settings: PlaidSettings | null; onSaved: () => void; onError: (e: string | null) => void }) {
  const [editing, setEditing] = useState(false);
  const [clientId, setClientId] = useState("");
  const [secret, setSecret] = useState("");
  const [env, setEnv] = useState<PlaidEnv>("production");
  const s = props.settings;
  if (!s) return null;

  if (s.configured && !editing) {
    return (
      <p className="muted">
        Plaid keys saved (client ID ending …{s.client_id_hint}, {s.env === "sandbox" ? "Sandbox test mode" : "real banks"}).{" "}
        <button className="link" onClick={() => (setEnv(s.env ?? "production"), setEditing(true))}>
          Change
        </button>
      </p>
    );
  }

  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      await bankApi.saveSettings(clientId, secret, env);
      setSecret("");
      setEditing(false);
      props.onError(null);
      props.onSaved();
    } catch (err) {
      props.onError(errorText(err));
    }
  };

  return (
    <form className="keys" onSubmit={save}>
      <h2>Plaid keys</h2>
      <p className="muted">
        From your Plaid dashboard: Developers → Keys. Use the <strong>Production</strong> secret for real banks (the
        free Trial plan allows 10 bank logins), or the <strong>Sandbox</strong> secret to try it with a fake bank.
      </p>
      <label>
        Client ID
        <input value={clientId} onChange={(e) => setClientId(e.target.value)} autoComplete="off" />
      </label>
      <label>
        Secret
        <input type="password" value={secret} onChange={(e) => setSecret(e.target.value)} autoComplete="off" />
      </label>
      <label>
        Which secret is this?
        <select value={env} onChange={(e) => setEnv(e.target.value as PlaidEnv)}>
          <option value="production">Production (real banks)</option>
          <option value="sandbox">Sandbox (fake test bank)</option>
        </select>
      </label>
      <div className="row-buttons">
        <button type="submit" className="primary">
          Save keys
        </button>
        {s.configured && (
          <button type="button" onClick={() => setEditing(false)}>
            Cancel
          </button>
        )}
      </div>
    </form>
  );
}

function AccountMatcher(props: {
  connection: NewConnection;
  accounts: Account[];
  onDone: () => void;
  onError: (e: string) => void;
}) {
  const [choice, setChoice] = useState<Record<string, number | null>>({});
  const save = async () => {
    try {
      await bankApi.linkAccounts(
        props.connection.item,
        props.connection.accounts.map((a) => ({ account: a, existing_account_id: choice[a.account_id] ?? null })),
      );
      props.onDone();
    } catch (e) {
      props.onError(errorText(e));
    }
  };
  const empty = props.connection.accounts.length === 0;
  useEffect(() => {
    if (empty) props.onDone(); // every account was already linked (e.g. a repeat connection)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [empty]);
  if (empty) return null;
  return (
    <div className="matcher">
      <h2>Connected {props.connection.institution_name}</h2>
      <p className="muted">
        Match each bank account to an account in the app. If you already imported CSV history into one, pick it: only
        transactions after its latest one will come in, so nothing is duplicated.
      </p>
      <table>
        <tbody>
          {props.connection.accounts.map((a) => (
            <tr key={a.account_id}>
              <td>
                {a.name}
                {a.mask && <span className="muted"> ••{a.mask}</span>}
              </td>
              <td>
                <select
                  value={choice[a.account_id] ?? ""}
                  onChange={(e) => setChoice({ ...choice, [a.account_id]: e.target.value ? Number(e.target.value) : null })}
                >
                  <option value="">Create a new account</option>
                  {props.accounts.map((x) => (
                    <option key={x.id} value={x.id}>
                      Use "{x.name}"
                    </option>
                  ))}
                </select>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <button className="primary" onClick={save}>
        Save and sync
      </button>
    </div>
  );
}
