use database_entity::dto::AFWorkspaceMember;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub enum UserMessage {
    ProfileChange(AFUserChange),
    WorkspaceMemberChange(AFWorkspaceMemberChange),
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct AFUserChange {
    pub uid: i64,
    pub name: Option<String>,
    pub email: Option<String>,
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct AFWorkspaceMemberChange {
    added: Vec<AFWorkspaceMember>,
    updated: Vec<AFWorkspaceMember>,
    removed: Vec<AFWorkspaceMember>,
}
