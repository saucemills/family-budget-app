//! Developer helper: import a CSV into a budget file without opening the app.
//!
//!     cargo run -p budget-core --example import_file -- <budget.sqlite> <account name> <file.csv>

use budget_core::{import::preview_csv, models::AccountKind, Budget};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, db, account, file] = args.as_slice() else {
        eprintln!("usage: import_file <budget.sqlite> <account name> <file.csv>");
        std::process::exit(2);
    };
    let mut budget = Budget::open(db)?;
    let id = match budget.list_accounts()?.into_iter().find(|a| &a.name == account) {
        Some(a) => a.id,
        None => budget.create_account(account, AccountKind::Checking)?,
    };
    let content = std::fs::read_to_string(file)?;
    let mapping = preview_csv(&content)?.suggested.ok_or("couldn't work out the CSV columns")?;
    let r = budget.import_csv(id, &content, &mapping)?;
    println!("imported {}, skipped {}, errors {:?}", r.imported, r.skipped_duplicates, r.errors);
    Ok(())
}
