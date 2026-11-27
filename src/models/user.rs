use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub name: String,
    pub initials: String,
    pub color: u32,
    pub status: String,
    pub endpoint: Option<String>,
    #[serde(default)]
    pub profile_image: Option<String>,
}

impl User {
    pub fn new(
        id: &str,
        username: &str,
        name: &str,
        initials: &str,
        color: u32,
        status: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            username: username.to_string(),
            name: name.to_string(),
            initials: initials.to_string(),
            color,
            status: status.to_string(),
            endpoint: None,
            profile_image: None,
        }
    }
}
