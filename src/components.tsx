import { useEffect, useState } from "react";
import type { CategoryGroup } from "./api";

/** A category dropdown grouped by category group. Empty value means "uncategorized". */
export function CategorySelect(props: {
  groups: CategoryGroup[];
  value: number | null;
  onChange: (id: number | null) => void;
}) {
  return (
    <select
      className={props.value == null ? "uncategorized" : undefined}
      value={props.value ?? ""}
      onChange={(e) => props.onChange(e.target.value === "" ? null : Number(e.target.value))}
    >
      <option value="">Uncategorized</option>
      {props.groups.map((g) => (
        <optgroup key={g.id} label={g.name}>
          {g.categories.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </optgroup>
      ))}
    </select>
  );
}

/** A button that asks "Sure?" on the first click and acts on the second. */
export function ConfirmButton(props: { label: string; confirmLabel?: string; onConfirm: () => void }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 3000);
    return () => clearTimeout(t);
  }, [armed]);
  return (
    <button
      className={armed ? "danger armed" : "danger"}
      onClick={() => (armed ? (setArmed(false), props.onConfirm()) : setArmed(true))}
    >
      {armed ? (props.confirmLabel ?? "Click again to confirm") : props.label}
    </button>
  );
}

export function ErrorBanner({ error }: { error: string | null }) {
  return error ? <div className="error">{error}</div> : null;
}
