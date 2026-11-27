#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Route {
    #[default]
    Friends,
    Groups,
    Messages,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPage {
    #[default]
    Account,
    Devices,
    General,
    Appearance,
    About,
}

impl SettingsPage {
    pub const ALL: [Self; 5] = [
        Self::Account,
        Self::Devices,
        Self::General,
        Self::Appearance,
        Self::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Account => "Account",
            Self::Devices => "Devices",
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::About => "About",
        }
    }
}
