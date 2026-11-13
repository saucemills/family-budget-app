//! The desktop app shell. Each `#[tauri::command]` here is a thin wrapper that the
//! React UI calls with `invoke(...)`; the real logic lives in `budget-core`.

use std::sync::Mutex;

use budget_core::import::{self, CsvMapping, CsvPreview, ImportResult};
use budget_core::models::*;
use budget_core::plan::MonthPlan;
use budget_core::reflect::ReflectSummary;
use budget_core::Budget;
use tauri::{Manager, State};

struct AppState(Mutex<Budget>);

type CmdResult<T> = Result<T, String>;

fn with<T>(state: &State<AppState>, f: impl FnOnce(&mut Budget) -> budget_core::Result<T>) -> CmdResult<T> {
    let mut budget = state.0.lock().map_err(|_| "the database is busy, try again".to_string())?;
    f(&mut budget).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_accounts(state: State<AppState>) -> CmdResult<Vec<Account>> {
    with(&state, |b| b.list_accounts())
}

#[tauri::command]
fn create_account(state: State<AppState>, name: String, kind: AccountKind) -> CmdResult<i64> {
    with(&state, |b| b.create_account(&name, kind))
}

#[tauri::command]
fn delete_account(state: State<AppState>, id: i64) -> CmdResult<()> {
    with(&state, |b| b.delete_account(id))
}

#[tauri::command]
fn list_category_groups(state: State<AppState>) -> CmdResult<Vec<CategoryGroup>> {
    with(&state, |b| b.list_category_groups())
}

#[tauri::command]
fn create_category_group(state: State<AppState>, name: String, is_income: bool) -> CmdResult<i64> {
    with(&state, |b| b.create_category_group(&name, is_income))
}

#[tauri::command]
fn create_category(state: State<AppState>, group_id: i64, name: String) -> CmdResult<i64> {
    with(&state, |b| b.create_category(group_id, &name))
}

#[tauri::command]
fn rename_category(state: State<AppState>, id: i64, name: String) -> CmdResult<()> {
    with(&state, |b| b.rename_category(id, &name))
}

#[tauri::command]
fn hide_category(state: State<AppState>, id: i64) -> CmdResult<()> {
    with(&state, |b| b.hide_category(id))
}

#[tauri::command]
fn list_transactions(state: State<AppState>, filter: TransactionFilter) -> CmdResult<Vec<Transaction>> {
    with(&state, |b| b.list_transactions(&filter))
}

#[tauri::command]
fn add_transaction(state: State<AppState>, txn: NewTransaction) -> CmdResult<i64> {
    with(&state, |b| b.add_transaction(&txn))
}

#[tauri::command]
fn set_transaction_category(state: State<AppState>, id: i64, category_id: Option<i64>) -> CmdResult<()> {
    with(&state, |b| b.set_transaction_category(id, category_id))
}

#[tauri::command]
fn delete_transaction(state: State<AppState>, id: i64) -> CmdResult<()> {
    with(&state, |b| b.delete_transaction(id))
}

#[tauri::command]
fn preview_csv(content: String) -> CmdResult<CsvPreview> {
    import::preview_csv(&content).map_err(|e| e.to_string())
}

#[tauri::command]
fn import_csv(state: State<AppState>, account_id: i64, content: String, mapping: CsvMapping) -> CmdResult<ImportResult> {
    with(&state, |b| b.import_csv(account_id, &content, &mapping))
}

#[tauri::command]
fn get_month_plan(state: State<AppState>, month: String) -> CmdResult<MonthPlan> {
    with(&state, |b| b.get_month_plan(&month))
}

#[tauri::command]
fn set_assigned(state: State<AppState>, month: String, category_id: i64, cents: i64) -> CmdResult<()> {
    with(&state, |b| b.set_assigned(&month, category_id, cents))
}

#[tauri::command]
fn copy_previous_assignments(state: State<AppState>, month: String) -> CmdResult<usize> {
    with(&state, |b| b.copy_previous_assignments(&month))
}

#[tauri::command]
fn reflect(state: State<AppState>, start: String, end: String) -> CmdResult<ReflectSummary> {
    with(&state, |b| b.reflect(&start, &end))
}

#[tauri::command]
fn database_path(app: tauri::AppHandle) -> CmdResult<String> {
    Ok(db_path(&app)?.display().to_string())
}

fn db_path(app: &tauri::AppHandle) -> CmdResult<std::path::PathBuf> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("budget.sqlite"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let path = db_path(app.handle())?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let budget = Budget::open(&path)?;
            app.manage(AppState(Mutex::new(budget)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            create_account,
            delete_account,
            list_category_groups,
            create_category_group,
            create_category,
            rename_category,
            hide_category,
            list_transactions,
            add_transaction,
            set_transaction_category,
            delete_transaction,
            preview_csv,
            import_csv,
            get_month_plan,
            set_assigned,
            copy_previous_assignments,
            reflect,
            database_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Family Budget");
}
