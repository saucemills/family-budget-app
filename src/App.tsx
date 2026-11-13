import { useEffect, useState } from "react";
import { api } from "./api";
import TransactionsPage from "./pages/Transactions";
import AccountsPage from "./pages/Accounts";
import CategoriesPage from "./pages/Categories";
import ImportPage from "./pages/Import";
import PlanPage from "./pages/Plan";
import ReflectPage from "./pages/Reflect";

type Page = "plan" | "reflect" | "transactions" | "accounts" | "categories" | "import";

const NAV: { id: Page; label: string }[] = [
  { id: "plan", label: "Plan" },
  { id: "reflect", label: "Reflect" },
  { id: "transactions", label: "Transactions" },
  { id: "accounts", label: "Accounts" },
  { id: "categories", label: "Categories" },
  { id: "import", label: "Import" },
];

export default function App() {
  const [page, setPage] = useState<Page>("plan");
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
        {dbPath && (
          <div className="db-path" title={dbPath}>
            Your data is saved on this computer at
            <br />
            <code>{dbPath}</code>
          </div>
        )}
      </nav>
      <main className="content">
        {page === "plan" && <PlanPage goToTransactions={() => setPage("transactions")} />}
        {page === "reflect" && <ReflectPage goToTransactions={() => setPage("transactions")} />}
        {page === "transactions" && <TransactionsPage />}
        {page === "accounts" && <AccountsPage />}
        {page === "categories" && <CategoriesPage />}
        {page === "import" && <ImportPage onDone={() => setPage("transactions")} />}
      </main>
    </div>
  );
}
