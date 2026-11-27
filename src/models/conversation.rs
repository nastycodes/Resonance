use serde::{Deserialize, Serialize};

use super::user::User;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Conversation {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub preview: String,
    pub updated_at: String,
    #[serde(default)]
    pub is_closed: bool,
    #[serde(default)]
    pub members: Vec<User>,
    #[serde(default = "initial_membership_version")]
    pub membership_version: u64,
}

impl Conversation {
    pub fn new(id: &str, user_id: &str, name: &str, preview: &str, updated_at: &str) -> Self {
        Self {
            id: id.to_string(),
            user_id: user_id.to_string(),
            name: name.to_string(),
            preview: preview.to_string(),
            updated_at: updated_at.to_string(),
            is_closed: false,
            members: Vec::new(),
            membership_version: initial_membership_version(),
        }
    }

    pub fn is_group(&self) -> bool {
        !self.members.is_empty()
    }
}

fn initial_membership_version() -> u64 {
    1
}
