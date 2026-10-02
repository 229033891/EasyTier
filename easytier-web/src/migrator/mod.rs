use sea_orm_migration::prelude::*;

mod m20241029_000001_init;
mod m20260403_000002_scope_network_config_unique;
mod m20260421_000003_add_network_config_source;
mod m20260514_000004_rename_web_config_source;
mod m20260619_000005_managed_config_revisions;
mod m20260929_000006_peer_conn_history;
mod m20260930_000007_devices_networks_hardening;
mod m20261001_000008_user_config_token;
mod m20261001_000009_user_config_tokens;
mod m20261001_000010_repair_config_tokens;
mod m20261002_000011_device_display_name;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20241029_000001_init::Migration),
            Box::new(m20260403_000002_scope_network_config_unique::Migration),
            Box::new(m20260421_000003_add_network_config_source::Migration),
            Box::new(m20260514_000004_rename_web_config_source::Migration),
            Box::new(m20260619_000005_managed_config_revisions::Migration),
            Box::new(m20260929_000006_peer_conn_history::Migration),
            Box::new(m20260930_000007_devices_networks_hardening::Migration),
            Box::new(m20261001_000008_user_config_token::Migration),
            Box::new(m20261001_000009_user_config_tokens::Migration),
            Box::new(m20261001_000010_repair_config_tokens::Migration),
            Box::new(m20261002_000011_device_display_name::Migration),
        ]
    }
}
