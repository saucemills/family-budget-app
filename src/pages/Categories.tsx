import { useCallback, useEffect, useState } from "react";
import { api, Category, CategoryGroup, errorText } from "../api";
import { ConfirmButton, ErrorBanner } from "../components";

export default function CategoriesPage() {
  const [groups, setGroups] = useState<CategoryGroup[]>([]);
  const [newGroup, setNewGroup] = useState("");
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setGroups(await api.listCategoryGroups());
    } catch (e) {
      setError(errorText(e));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  const run = async (f: () => Promise<unknown>) => {
    try {
      await f();
      setError(null);
      await load();
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <div>
      <h1>Categories</h1>
      <p className="muted">
        Groups hold related categories, like Food → Groceries, Restaurants, Coffee. These are what the budget and the
        "where did my money go" chart will use.
      </p>
      <ErrorBanner error={error} />
      <form
        className="add-row"
        onSubmit={(e) => {
          e.preventDefault();
          run(() => api.createCategoryGroup(newGroup, false)).then(() => setNewGroup(""));
        }}
      >
        <input placeholder="New group name" value={newGroup} onChange={(e) => setNewGroup(e.target.value)} />
        <button type="submit" className="primary">
          Add group
        </button>
      </form>
      <div className="groups">
        {groups.map((g) => (
          <GroupCard key={g.id} group={g} run={run} />
        ))}
      </div>
    </div>
  );
}

function GroupCard({ group, run }: { group: CategoryGroup; run: (f: () => Promise<unknown>) => Promise<void> }) {
  const [name, setName] = useState("");
  return (
    <section className="group">
      <h2>
        {group.name}
        {group.is_income && <span className="tag">Income</span>}
      </h2>
      <ul>
        {group.categories.map((c) => (
          <CategoryRow key={c.id} cat={c} run={run} />
        ))}
      </ul>
      <form
        className="inline"
        onSubmit={(e) => {
          e.preventDefault();
          run(() => api.createCategory(group.id, name)).then(() => setName(""));
        }}
      >
        <input placeholder="Add a category" value={name} onChange={(e) => setName(e.target.value)} />
        <button type="submit">Add</button>
      </form>
    </section>
  );
}

function CategoryRow({ cat, run }: { cat: Category; run: (f: () => Promise<unknown>) => Promise<void> }) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(cat.name);
  if (editing) {
    return (
      <li>
        <form
          className="inline"
          onSubmit={(e) => {
            e.preventDefault();
            run(() => api.renameCategory(cat.id, name)).then(() => setEditing(false));
          }}
        >
          <input autoFocus value={name} onChange={(e) => setName(e.target.value)} />
          <button type="submit">Save</button>
          <button type="button" onClick={() => (setName(cat.name), setEditing(false))}>
            Cancel
          </button>
        </form>
      </li>
    );
  }
  return (
    <li>
      <span>{cat.name}</span>
      <span className="row-actions">
        <button onClick={() => setEditing(true)}>Rename</button>
        <ConfirmButton label="Hide" confirmLabel="Hide it?" onConfirm={() => run(() => api.hideCategory(cat.id))} />
      </span>
    </li>
  );
}
