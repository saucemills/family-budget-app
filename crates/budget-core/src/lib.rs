//! Core logic for Family Budget: the local SQLite database, the data model,
//! and bank CSV import. Kept free of any UI code so it can be tested on its own.

pub mod db;
pub mod error;
pub mod import;
pub mod models;
pub mod money;

pub use db::Budget;
pub use error::{Error, Result};
