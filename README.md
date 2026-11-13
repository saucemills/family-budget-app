# Family Budget

A free budgeting app for the family, in the spirit of YNAB. It runs on your own
computer and keeps all its data there, in a single SQLite file.

Built with [Tauri](https://tauri.app) (a Rust backend with a React interface).

## What works today (step 1)

- **Accounts:** checking, savings, credit cards and cash, with running balances.
- **Categories:** groups of categories (Food → Groceries, Restaurants, Coffee…), seeded with a starter set you can rename, hide or add to.
- **Transactions:** add them by hand, search, filter by account or "needs a category", and categorize each one.
- **CSV import:** open a CSV downloaded from your bank. The app guesses which column is the date, description and amount (including banks that use separate Debit/Credit columns), shows you its guess to fix if needed, and skips rows you've already imported.

## Roadmap

1. ~~App setup, local database, CSV import~~
2. **Plan** (YNAB-style monthly budget: give every dollar a job) and **Reflect** ("where did my money go", including a Sankey diagram of income → categories)
3. Plaid bank syncing and auto-categorization rules
4. Everyday polish: review queue, split transactions, backups, Windows installer

## Running it on Windows

One-time setup:

1. Install the [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) and tick **Desktop development with C++**.
2. Install Rust from [rustup.rs](https://rustup.rs).
3. Install [Node.js](https://nodejs.org) (the LTS version).
4. WebView2 already comes with Windows 10 and 11.

Then, in this folder:

```sh
npm install
npm run tauri dev      # opens the app, reloading as code changes
npm run tauri build    # makes an installer in target/release/bundle/
```

On Windows your data is saved at `%APPDATA%\app.familybudget.desktop\budget.sqlite`.
The app shows the exact path at the bottom of its sidebar. Copy that file to back it up.

## Project layout

| Path | What's there |
| --- | --- |
| `crates/budget-core/` | All the real logic: database schema, accounts, categories, transactions, CSV import. Has its own tests. |
| `src-tauri/` | The desktop app shell. Each command the UI can call is a thin wrapper over `budget-core`. |
| `src/` | The React interface. `src/api.ts` lists every call into Rust. |
| `samples/` | An example bank CSV to try importing. |

Money is stored as whole cents (`-1234` is −$12.34), so totals never pick up rounding errors.
Negative amounts are money going out, positive amounts are money coming in.

## Tests

```sh
cargo test -p budget-core
```

To load a CSV into a budget file without opening the app:

```sh
cargo run -p budget-core --example import_file -- budget.sqlite "Joint Checking" samples/example-checking.csv
```
