import { useEffect, useState } from "react";
import { api } from "./api";
import TransactionsPage from "./pages/Transactions";
import AccountsPage from "./pages/Accounts";
import CategoriesPage from "./pages/Categories";
import ImportPage from "./pages/Import";

type Page = "transactions" | "accounts" | "categories" | "import";

const NAV: { id: Page; label: string }[] = [
  { id: "transactions", label: "Transactions" },
  { id: "accounts", label: "Accounts" },
  { id: "categories", label: "Categories" },
  { id: "import", label: "Import" },
];

export default function App() {
  const [page, setPage] = useState<Page>("transactions");
  const [dbPath, setDbPath] = useState("");

  useEffect(() => {
    api.databasePath().then(setDbPath).catch(() => {});
  }, []);

  return (
    <div className="app">
      <nav className="sidebar">
        <div className="brand">Family Budget</div>
        {NAV.map((n) => (
          <button key={n.id} className={page === n.id ? "nav active" : "nav"} onClick={() => setPage(n.id)}>
            {n.label}
          </button>
        ))}
        <div className="nav-section">Coming next</div>
        <button className="nav" disabled title="Monthly budget: give every dollar a job">
          Plan
        </button>
        <button className="nav" disabled title="Where did the money go?">
          Reflect
        </button>
        {dbPath && (
          <div className="db-path" title={dbPath}>
            Your data is saved on this computer at
            <br />
            <code>{dbPath}</code>
          </div>
        )}
      </nav>
      <main className="content">
        {page === "transactions" && <TransactionsPage />}
        {page === "accounts" && <AccountsPage />}
        {page === "categories" && <CategoriesPage />}
        {page === "import" && <ImportPage onDone={() => setPage("transactions")} />}
      </main>
    </div>
  );
}
