//! Postgres DB stuff.

use sqlx::{Executor, PgPool};

/// Connect to the Postgres DB and run the init script to create tables if they don't exist.
pub async fn connect_and_init(db_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPool::connect(db_url).await?;

    pool.execute(include_str!("./create.sql")).await?;

    Ok(pool)
}
