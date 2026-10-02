//! `SeaORM` Entity for user-scoped device archive.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "devices")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub user_id: i32,
    #[sea_orm(column_type = "Text")]
    pub device_id: String,
    #[sea_orm(column_type = "Text")]
    pub hostname: String,
    /// Web-console display alias; empty means fall back to `hostname`.
    #[sea_orm(column_type = "Text")]
    pub display_name: String,
    #[sea_orm(column_type = "Text")]
    pub last_easytier_version: String,
    #[sea_orm(column_type = "Text")]
    pub last_client_url: String,
    pub last_seen_at: i64,
    pub create_time: DateTimeWithTimeZone,
    pub update_time: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Users,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
