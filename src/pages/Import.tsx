import { useEffect, useState } from "react";
import { api, Account, CsvMapping, CsvPreview, ImportResult, errorText } from "../api";
import { ErrorBanner } from "../components";

const DATE_FORMATS = [
  { value: "%m/%d/%Y", label: "11/25/2026" },
  { value: "%m/%d/%y", label: "11/25/26" },
  { value: "%Y-%m-%d", label: "2026-11-25" },
  { value: "%m-%d-%Y", label: "11-25-2026" },
  { value: "%Y/%m/%d", label: "2026/11/25" },
  { value: "%d/%m/%Y", label: "25/11/2026" },
];

export default function ImportPage({ onDone }: { onDone: () => void }) {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [accountId, setAccountId] = useState<number | null>(null);
  const [fileName, setFileName] = useState("");
  const [content, setContent] = useState("");
  const [preview, setPreview] = useState<CsvPreview | null>(null);
  const [mapping, setMapping] = useState<CsvMapping | null>(null);
  const [result, setResult] = useState<ImportResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .listAccounts()
      .then((a) => {
        setAccounts(a);
        if (a.length) setAccountId(a[0].id);
      })
      .catch((e) => setError(errorText(e)));
  }, []);

  const chooseFile = async (file: File | undefined) => {
    setResult(null);
    setPreview(null);
    setMapping(null);
    if (!file) return;
    try {
      const text = await file.text();
      const p = await api.previewCsv(text);
      setFileName(file.name);
      setContent(text);
      setPreview(p);
      setMapping(
        p.suggested ?? {
          has_header: true,
          date_column: 0,
          date_format: "%m/%d/%Y",
          payee_column: Math.min(1, p.headers.length - 1),
          memo_column: null,
          amount: { kind: "single", column: Math.min(2, p.headers.length - 1) },
          flip_sign: false,
        },
      );
      setError(p.suggested ? null : "We couldn't work out this file's columns. Please pick them below.");
    } catch (e) {
      setError(errorText(e));
    }
  };

  const runImport = async () => {
    if (accountId == null || !mapping) return;
    setBusy(true);
    try {
      setResult(await api.importCsv(accountId, content, mapping));
      setError(null);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  if (accounts.length === 0) {
    return (
      <div>
        <h1>Import</h1>
        <ErrorBanner error={error} />
        <p className="empty">Add an account on the Accounts page first, then come back to import its CSV.</p>
      </div>
    );
  }

  const set = (patch: Partial<CsvMapping>) => mapping && setMapping({ ...mapping, ...patch });
  const columnOptions = preview?.headers.map((h, i) => (
    <option key={i} value={i}>
      {h || `Column ${i + 1}`}
    </option>
  ));
  const highlight = (i: number) => {
    if (!mapping) return "";
    if (i === mapping.date_column) return "col-date";
    if (i === mapping.payee_column) return "col-payee";
    if (i === mapping.memo_column) return "col-memo";
    const a = mapping.amount;
    if (a.kind === "single" ? i === a.column : i === a.outflow || i === a.inflow) return "col-amount";
    return "";
  };

  return (
    <div>
      <h1>Import a bank CSV</h1>
      <p className="muted">
        Download transactions from your bank's website as a CSV file, then open it here. Importing the same file twice is
        safe: rows already in the app are skipped.
      </p>
      <ErrorBanner error={error} />

      <div className="add-row">
        <label>
          Import into{" "}
          <select value={accountId ?? ""} onChange={(e) => setAccountId(Number(e.target.value))}>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
        </label>
        <input type="file" accept=".csv,text/csv" onChange={(e) => chooseFile(e.target.files?.[0])} />
      </div>

      {preview && mapping && (
        <>
          <h2>
            {fileName}: {preview.total_rows} rows
          </h2>
          <div className="mapping">
            <label>
              Date
              <select value={mapping.date_column} onChange={(e) => set({ date_column: Number(e.target.value) })}>
                {columnOptions}
              </select>
            </label>
            <label>
              Date looks like
              <select value={mapping.date_format} onChange={(e) => set({ date_format: e.target.value })}>
                {DATE_FORMATS.map((f) => (
                  <option key={f.value} value={f.value}>
                    {f.label}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Payee / description
              <select value={mapping.payee_column} onChange={(e) => set({ payee_column: Number(e.target.value) })}>
                {columnOptions}
              </select>
            </label>
            <label>
              Memo
              <select
                value={mapping.memo_column ?? ""}
                onChange={(e) => set({ memo_column: e.target.value === "" ? null : Number(e.target.value) })}
              >
                <option value="">(none)</option>
                {columnOptions}
              </select>
            </label>
            <label>
              Amounts are in
              <select
                value={mapping.amount.kind}
                onChange={(e) =>
                  set({
                    amount:
                      e.target.value === "single"
                        ? { kind: "single", column: 0 }
                        : { kind: "split", outflow: 0, inflow: Math.min(1, preview.headers.length - 1) },
                  })
                }
              >
                <option value="single">One column</option>
                <option value="split">Separate money out / money in columns</option>
              </select>
            </label>
            {mapping.amount.kind === "single" ? (
              <label>
                Amount
                <select
                  value={mapping.amount.column}
                  onChange={(e) => set({ amount: { kind: "single", column: Number(e.target.value) } })}
                >
                  {columnOptions}
                </select>
              </label>
            ) : (
              <>
                <label>
                  Money out
                  <select
                    value={mapping.amount.outflow}
                    onChange={(e) =>
                      mapping.amount.kind === "split" &&
                      set({ amount: { ...mapping.amount, outflow: Number(e.target.value) } })
                    }
                  >
                    {columnOptions}
                  </select>
                </label>
                <label>
                  Money in
                  <select
                    value={mapping.amount.inflow}
                    onChange={(e) =>
                      mapping.amount.kind === "split" &&
                      set({ amount: { ...mapping.amount, inflow: Number(e.target.value) } })
                    }
                  >
                    {columnOptions}
                  </select>
                </label>
              </>
            )}
            <label className="check">
              <input type="checkbox" checked={mapping.has_header} onChange={(e) => set({ has_header: e.target.checked })} />
              First row is column names
            </label>
            <label className="check">
              <input type="checkbox" checked={mapping.flip_sign} onChange={(e) => set({ flip_sign: e.target.checked })} />
              Purchases show as positive numbers (common for credit cards)
            </label>
          </div>

          <div className="table-scroll">
            <table className="preview">
              <thead>
                <tr>
                  {preview.headers.map((h, i) => (
                    <th key={i} className={highlight(i)}>
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {preview.sample_rows.map((r, ri) => (
                  <tr key={ri}>
                    {preview.headers.map((_, i) => (
                      <td key={i} className={highlight(i)}>
                        {r[i] ?? ""}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <button className="primary" disabled={busy} onClick={runImport}>
            {busy ? "Importing…" : `Import ${preview.total_rows} rows`}
          </button>
        </>
      )}

      {result && (
        <div className="result">
          <strong>{result.imported} imported</strong>
          {result.skipped_duplicates > 0 && <span>, {result.skipped_duplicates} already in the app (skipped)</span>}
          {result.errors.length > 0 && (
            <>
              <span>, {result.errors.length} couldn't be read:</span>
              <ul>
                {result.errors.slice(0, 20).map((e) => (
                  <li key={e}>{e}</li>
                ))}
              </ul>
            </>
          )}
          <div>
            <button onClick={onDone}>Go categorize them</button>
          </div>
        </div>
      )}
    </div>
  );
}
