pub mod embedded;
pub mod settings;

use anyhow::Result;
use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;

static MIGRATOR: Migrator = sqlx::migrate!("src/database/migrations");

pub async fn create_pool(connection_url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(connection_url)
        .await?;
    Ok(pool)
}

pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::MIGRATOR;

    #[test]
    fn embeds_every_migration() {
        let versions = MIGRATOR
            .iter()
            .map(|migration| migration.version)
            .collect::<Vec<_>>();
        assert_eq!(versions, (1..=31).collect::<Vec<_>>());
    }
}
