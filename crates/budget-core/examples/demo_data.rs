//! Developer helper: fill a budget file with three months of made-up family spending,
//! for trying out Plan and Reflect.
//!
//!     cargo run -p budget-core --example demo_data -- demo.sqlite

use budget_core::{
    models::{AccountKind, NewTransaction},
    Budget,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: demo_data <budget.sqlite>")?;
    let b = Budget::open(&path)?;
    let checking = b.create_account("Joint Checking", AccountKind::Checking)?;
    let card = b.create_account("Visa", AccountKind::CreditCard)?;
    let cats: Vec<(i64, String)> = b
        .list_category_groups()?
        .into_iter()
        .flat_map(|g| g.categories)
        .map(|c| (c.id, c.name))
        .collect();
    let id = |n: &str| cats.iter().find(|(_, name)| name == n).map(|(i, _)| *i).unwrap();

    // (day, payee, dollars, category, on card?)
    let month: &[(u32, &str, f64, &str, bool)] = &[
        (1, "ACME CORP PAYROLL", 3250.0, "Paycheck", false),
        (15, "ACME CORP PAYROLL", 3250.0, "Paycheck", false),
        (1, "OAKWOOD APARTMENTS", -1850.0, "Rent / Mortgage", false),
        (9, "PG&E", -142.77, "Utilities", false),
        (12, "COMCAST", -79.99, "Utilities", false),
        (3, "TRADER JOE'S", -96.42, "Groceries", true),
        (10, "COSTCO", -211.35, "Groceries", true),
        (17, "SAFEWAY", -88.10, "Groceries", true),
        (24, "TRADER JOE'S", -104.66, "Groceries", true),
        (6, "THE LOCAL TAQUERIA", -38.20, "Restaurants", true),
        (20, "SUSHI PLACE", -72.40, "Restaurants", true),
        (4, "STARBUCKS", -6.45, "Coffee", true),
        (11, "BLUE BOTTLE", -12.50, "Coffee", true),
        (18, "STARBUCKS", -7.10, "Coffee", true),
        (5, "SHELL", -48.10, "Gas", true),
        (19, "CHEVRON", -52.33, "Gas", true),
        (8, "NETFLIX", -15.49, "Subscriptions", true),
        (8, "SPOTIFY", -16.99, "Subscriptions", true),
        (7, "CHEWY.COM", -62.18, "Pet Food & Supplies", true),
        (21, "CVS PHARMACY", -24.15, "Pharmacy", true),
        (14, "AMAZON", -63.44, "Shopping", true),
        (2, "VANGUARD TRANSFER", -500.0, "Investments", false),
        (2, "HIGH YIELD SAVINGS", -300.0, "Emergency Fund", false),
    ];
    for (m, extra) in [("2026-09", 1.0), ("2026-10", 1.08), ("2026-11", 0.94)] {
        for (day, payee, dollars, cat, on_card) in month {
            let scale = if *dollars > 0.0 || *cat == "Rent / Mortgage" { 1.0 } else { extra };
            b.add_transaction(&NewTransaction {
                account_id: if *on_card { card } else { checking },
                date: format!("{m}-{day:02}"),
                payee: payee.to_string(),
                memo: String::new(),
                amount_cents: (dollars * scale * 100.0).round() as i64,
                category_id: Some(id(cat)),
            })?;
        }
        for (cat, dollars) in [
            ("Rent / Mortgage", 1850), ("Utilities", 230), ("Groceries", 500), ("Restaurants", 100),
            ("Coffee", 30), ("Gas", 110), ("Subscriptions", 35), ("Pet Food & Supplies", 70),
            ("Pharmacy", 40), ("Shopping", 100), ("Investments", 500), ("Emergency Fund", 300),
        ] {
            b.set_assigned(m, id(cat), dollars * 100)?;
        }
    }
    b.add_transaction(&NewTransaction {
        account_id: checking,
        date: "2026-11-12".into(),
        payee: "VENMO PAYMENT".into(),
        amount_cents: -4500,
        ..Default::default()
    })?;
    println!("demo data written to {path}");
    Ok(())
}
