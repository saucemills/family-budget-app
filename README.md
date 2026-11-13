# Family Budget

A free budgeting app for the family, in the spirit of YNAB. It runs on your own
computer and keeps all its data there, in a single SQLite file.

Built with [Tauri](https://tauri.app) (a Rust backend with a React interface).

## What works today

- **Plan:** a YNAB-style monthly budget. Money in income categories becomes "Ready to Assign"; you assign it to categories each month, see what's available, and leftovers roll forward. Overspending is taken out of next month's Ready to Assign. "Copy last month's amounts" fills in a new month quickly.
- **Reflect:** "where did the money go" for any date range: totals for money in, out and left over, a Sankey diagram (income → groups → categories) with hover details, and a spending table you can expand by category.
- **Bank sync (Plaid):** connect banks and cards on Plaid's own sign-in page; new, changed and removed transactions come in when you click Sync. Pending charges are picked up once they post. Linking a bank account to an account that already has CSV history only imports newer transactions. Plaid keys and connection tokens are kept in Windows Credential Manager (macOS Keychain; a private file on Linux), never in the budget file.
- **Auto-categorize:** after you categorize a transaction the app offers to make a rule ("payees containing STARBUCKS → Coffee"). Rules run on every import; when no rule matches, Plaid's own category guess fills in starter categories.
- **Accounts:** checking, savings, credit cards and cash, with running balances.
- **Categories:** groups of categories (Food → Groceries, Restaurants, Coffee…), seeded with a starter set you can rename, hide or add to.
- **Transactions:** add them by hand, search, filter by account or "needs a category", and categorize each one.
- **CSV import:** open a CSV downloaded from your bank. The app guesses which column is the date, description and amount (including banks that use separate Debit/Credit columns), shows you its guess to fix if needed, and skips rows you've already imported.

## Roadmap

1. ~~App setup, local database, CSV import~~
2. ~~Plan and Reflect~~
3. ~~Windows installer with in-app updates~~
4. ~~Plaid bank syncing and auto-categorization rules~~
5. Everyday polish: review queue, split transactions, backups

## Installing

Download the latest `Family Budget_x.y.z_x64-setup.exe` from
[Releases](https://github.com/saucemills/family-budget-app/releases/latest) and run it.
It installs for your Windows user (no admin needed) and adds Family Budget to the Start menu.

The app looks for a new version when it opens and every few hours while it's open, and
**Check for updates** at the bottom of the sidebar checks right away. When one is found it
shows **Update and restart**. Updates replace the app only; your budget file is kept.

## Transfers

Money moving between your own accounts isn't income or spending, so it goes to the
**Transfers** group, which Plan and Reflect leave out. The app matches these itself:

- **Paid through another account:** a bank charge like `Web Authorized Pmt Paypal` is paired
  with the PayPal purchase it paid for, when PayPal is connected too. The purchase keeps its
  real category, so the spending is counted once, under what was bought.
- **Between my accounts / Credit card payment / Loan payment:** the same amount leaving one
  account and arriving in another within 4 days. Between two different banks, one side has
  to name the other bank (`US BANK NA` arriving at SoFi), or the money has to land in a
  connected card or loan as a payment (`PAYMENT - THANK YOU`).
- Money arriving from another bank you've connected, whose other half is older than that
  bank's history.

Only transactions that are uncategorized, or that the app categorized itself, are matched.
A category you pick is never changed, and picking a non-transfer category undoes a match.
Rules ("payees containing TESLA go to Car Insurance") can be limited to an amount range, so
one payee can go to different categories depending on the size of the charge.

Payments to cards or loans that aren't connected are left for you to categorize, since
they're the only record of that spending.

Windows may show "Windows protected your PC" the first time, because the installer isn't
signed with a paid code-signing certificate. Click **More info → Run anyway**.

### Publishing a new version

Bump `"version"` in `package.json` (e.g. `0.3.0`) in a pull request and merge it. The
**Windows build** workflow sees a version that hasn't been released yet, builds the
installer, signs the update, and publishes release `v0.3.0`. Installed copies pick it up
the next time they open. Merges that don't change the version don't publish anything.

Updates are signed with a private key stored as the `TAURI_SIGNING_PRIVATE_KEY` repository
secret; the matching public key is in `src-tauri/tauri.conf.json`. If the private key is
ever lost, generate a new pair (`npx tauri signer generate`), update both, and install the
next version by hand once.

## Setting up bank sync

1. Sign up at [dashboard.plaid.com](https://dashboard.plaid.com/signup) and choose the free **Trial** plan
   (real banks, up to 10 bank logins; removing one doesn't free its slot).
2. Copy your **client ID** and **Production secret** from Developers → Keys.
3. In the app, open **Bank sync**, paste them in, and click **Connect a bank**.

To try it safely first, use the **Sandbox** secret and **Connect test bank** (a fake bank).

## Developing on Windows

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

To try Plan and Reflect with three months of made-up family spending:

```sh
cargo run -p budget-core --example demo_data -- demo.sqlite
```

To load a CSV into a budget file without opening the app:

```sh
cargo run -p budget-core --example import_file -- budget.sqlite "Joint Checking" samples/example-checking.csv
```
