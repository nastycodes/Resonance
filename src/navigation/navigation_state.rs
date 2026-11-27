use super::route::{Route, SettingsPage};

#[derive(Clone, Debug)]
pub struct NavigationState {
    pub route: Route,
    pub settings_open: bool,
    pub settings_page: SettingsPage,
}

impl Default for NavigationState {
    fn default() -> Self {
        Self {
            route: Route::Friends,
            settings_open: false,
            settings_page: SettingsPage::Account,
        }
    }
}

impl NavigationState {
    pub fn navigate(&mut self, route: Route) {
        self.settings_open = false;
        self.route = route;
    }

    pub fn open_settings(&mut self) {
        if !self.settings_open {
            self.settings_page = SettingsPage::Account;
        }
        self.settings_open = true;
    }

    pub fn close_settings(&mut self) {
        self.settings_open = false;
    }

    pub fn select_settings_page(&mut self, page: SettingsPage) {
        self.settings_page = page;
        self.settings_open = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_route_is_friends() {
        let navigation = NavigationState::default();
        assert_eq!(navigation.route, Route::Friends);
    }

    #[test]
    fn settings_navigation_remembers_the_page_until_reopened() {
        let mut navigation = NavigationState::default();
        navigation.select_settings_page(SettingsPage::About);
        assert!(navigation.settings_open);
        assert_eq!(navigation.settings_page, SettingsPage::About);

        navigation.navigate(Route::Friends);
        navigation.open_settings();
        assert_eq!(navigation.route, Route::Friends);
        assert_eq!(navigation.settings_page, SettingsPage::Account);
    }

    #[test]
    fn selecting_a_settings_page_opens_settings_without_changing_the_route() {
        let mut navigation = NavigationState::default();
        navigation.navigate(Route::Messages);
        navigation.select_settings_page(SettingsPage::About);

        assert_eq!(navigation.route, Route::Messages);
        assert!(navigation.settings_open);
        assert_eq!(navigation.settings_page, SettingsPage::About);
    }

    #[test]
    fn closing_settings_keeps_the_current_route() {
        let mut navigation = NavigationState::default();
        navigation.navigate(Route::Friends);
        navigation.open_settings();
        navigation.close_settings();

        assert_eq!(navigation.route, Route::Friends);
        assert!(!navigation.settings_open);
    }
}
