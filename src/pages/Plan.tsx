import { useCallback, useEffect, useState } from "react";
import { planApi, MonthPlan, PlanCategory, errorText, formatMoney, monthLabel, parseMoney, shiftMonth, thisMonth } from "../api";
import { ErrorBanner } from "../components";

export default function PlanPage({ goToTransactions }: { goToTransactions: () => void }) {
  const [month, setMonth] = useState(thisMonth());
  const [plan, setPlan] = useState<MonthPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setPlan(await planApi.getMonthPlan(month));
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [month]);
  useEffect(() => {
    setNote(null);
    load();
  }, [load]);

  const assign = async (c: PlanCategory, cents: number) => {
    if (cents === c.assigned_cents) return;
    try {
      await planApi.setAssigned(month, c.id, cents);
      await load();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const copyLast = async () => {
    try {
      const n = await planApi.copyPreviousAssignments(month);
      setNote(n ? `Copied ${n} amounts from ${monthLabel(shiftMonth(month, -1))}.` : "Nothing to copy: last month had no amounts that aren't already set here.");
      await load();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const rta = plan?.ready_to_assign_cents ?? 0;

  return (
    <div>
      <div className="plan-header">
        <div className="month-switch">
          <button aria-label="Previous month" onClick={() => setMonth(shiftMonth(month, -1))}>
            ‹
          </button>
          <h1>{monthLabel(month)}</h1>
          <button aria-label="Next month" onClick={() => setMonth(shiftMonth(month, 1))}>
            ›
          </button>
          {month !== thisMonth() && (
            <button className="link" onClick={() => setMonth(thisMonth())}>
              Today
            </button>
          )}
        </div>
        <div className={rta > 0 ? "rta positive" : rta < 0 ? "rta negative" : "rta zero"}>
          <div className="rta-amount">{formatMoney(rta)}</div>
          <div className="rta-label">
            {rta > 0 ? "Ready to assign" : rta < 0 ? "Assigned more than you have" : "Every dollar has a job"}
          </div>
        </div>
      </div>
      <ErrorBanner error={error} />

      {plan && (
        <>
          <p className="muted plan-sub">
            {formatMoney(plan.income_this_month_cents)} came in this month, {formatMoney(plan.assigned_this_month_cents)} assigned.
            {plan.overspent_last_month_cents < 0 &&
              ` ${formatMoney(-plan.overspent_last_month_cents)} overspent last month was taken out of Ready to Assign.`}{" "}
            <button className="link" onClick={copyLast}>
              Copy last month's amounts
            </button>
          </p>
          {note && <p className="muted">{note}</p>}
          {plan.uncategorized_count > 0 && (
            <div className="notice">
              {plan.uncategorized_count} transaction{plan.uncategorized_count === 1 ? " this month needs" : "s this month need"} a category
              before they count here.{" "}
              <button className="link" onClick={goToTransactions}>
                Categorize them
              </button>
            </div>
          )}

          <table className="plan">
            <thead>
              <tr>
                <th>Category</th>
                <th className="num">Assigned</th>
                <th className="num">Activity</th>
                <th className="num">Available</th>
              </tr>
            </thead>
            {plan.groups.map((g) => (
              <tbody key={g.id}>
                <tr className="group-row">
                  <td>{g.name}</td>
                  <td className="num">{formatMoney(g.assigned_cents)}</td>
                  <td className="num">{formatMoney(g.activity_cents)}</td>
                  <td className="num">{formatMoney(g.available_cents)}</td>
                </tr>
                {g.categories.map((c) => (
                  <tr key={c.id}>
                    <td className="cat-name">{c.name}</td>
                    <td className="num">
                      <AssignInput key={`${month}-${c.id}-${c.assigned_cents}`} cents={c.assigned_cents} onSave={(v) => assign(c, v)} />
                    </td>
                    <td className="num muted">{formatMoney(c.activity_cents)}</td>
                    <td className="num">
                      <span className={c.available_cents > 0 ? "pill positive" : c.available_cents < 0 ? "pill negative" : "pill zero"}>
                        {formatMoney(c.available_cents)}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            ))}
          </table>
          <p className="muted">
            Type an amount in Assigned and press Enter. Leftover money rolls into next month. If a category goes
            negative, next month it starts at zero and the overspent amount comes out of Ready to Assign.
          </p>
        </>
      )}
    </div>
  );
}

function AssignInput({ cents, onSave }: { cents: number; onSave: (cents: number) => void }) {
  const [text, setText] = useState(cents ? (cents / 100).toFixed(2) : "");
  const [bad, setBad] = useState(false);
  const commit = () => {
    if (text.trim() === "") return onSave(0);
    const v = parseMoney(text);
    if (v == null) return setBad(true);
    setBad(false);
    onSave(v);
  };
  return (
    <input
      className={bad ? "assign bad" : "assign"}
      value={text}
      placeholder="0.00"
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        if (e.key === "Escape") setText(cents ? (cents / 100).toFixed(2) : "");
      }}
      onFocus={(e) => e.target.select()}
    />
  );
}
