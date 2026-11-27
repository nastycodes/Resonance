use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};

const MAX_PROFILE_IMAGE_BYTES: usize = 1_000_000;

impl AppState {
    pub fn choose_profile_image(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a profile picture".into()),
        });
        cx.spawn(async move |this, cx| match receiver.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |state, cx| state.save_profile_image_path(path, cx));
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => {
                let error = error.to_string();
                let _ = this.update(cx, |state, cx| {
                    state.set_toast(
                        Some(format!("Could not choose a profile picture: {error}")),
                        cx,
                    );
                    cx.notify();
                });
            }
            Err(error) => {
                let error = error.to_string();
                let _ = this.update(cx, |state, cx| {
                    state.set_toast(
                        Some(format!("Could not choose a profile picture: {error}")),
                        cx,
                    );
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn save_profile_image_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.set_toast(
                    Some(format!(
                        "Could not read the selected profile picture: {error}"
                    )),
                    cx,
                );
                cx.notify();
                return;
            }
        };
        if bytes.is_empty() || bytes.len() > MAX_PROFILE_IMAGE_BYTES {
            self.set_toast(
                Some("Profile pictures must be between 1 byte and 1 MB.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        if profile_image_format(&bytes).is_none() {
            self.set_toast(
                Some("Choose a PNG, JPEG, or WebP profile picture.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        let profile_image = STANDARD.encode(bytes);
        self.update_profile_image(Some(&profile_image), cx);
    }

    pub fn remove_profile_image(&mut self, cx: &mut Context<Self>) {
        self.update_profile_image(None, cx);
    }

    fn update_profile_image(&mut self, image: Option<&str>, cx: &mut Context<Self>) {
        let (api, token) = match self.api_session() {
            Ok(session) => session,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        match api.update_profile_image(&token, image) {
            Ok(account) => {
                if self
                    .current_user
                    .as_ref()
                    .is_some_and(|user| user.id == account.public_key)
                {
                    if let Some(user) = self.current_user.as_mut() {
                        user.profile_image = account.profile_image.clone();
                    }
                    if let Some(identity) = self.identity.as_mut() {
                        identity.user.profile_image = account.profile_image;
                    }
                    self.set_toast(Some("Profile picture saved to your account.".into()), cx);
                } else {
                    self.set_toast(
                        Some("The account API returned a different identity.".into()),
                        cx,
                    );
                }
            }
            Err(error) => self.set_toast(Some(error.message), cx),
        }
        cx.notify();
    }

    pub fn change_account_password(&mut self, cx: &mut Context<Self>) {
        let current_password = Zeroizing::new(std::mem::take(&mut self.current_password_input));
        let new_password = Zeroizing::new(std::mem::take(&mut self.new_password_input));
        let confirm_password = Zeroizing::new(std::mem::take(&mut self.confirm_password_input));
        if new_password != confirm_password {
            self.set_toast(Some("The new passwords do not match.".into()), cx);
            cx.notify();
            return;
        }

        let (api, token) = match self.api_session() {
            Ok(session) => session,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let Some(identity) = self.identity.as_ref() else {
            self.set_toast(
                Some("You must be signed in to change your password.".into()),
                cx,
            );
            cx.notify();
            return;
        };
        let encrypted_identity = match self.identity_service.reencrypt(identity, &new_password) {
            Ok(identity) => identity,
            Err(error) => {
                self.set_toast(Some(error.to_string()), cx);
                cx.notify();
                return;
            }
        };

        match api.change_password(
            &token,
            &current_password,
            &new_password,
            &encrypted_identity,
        ) {
            Ok(()) => {
                self.set_toast(Some("Account password updated.".into()), cx);
                if self.remember_me {
                    let username = self
                        .current_user
                        .as_ref()
                        .map(|user| user.username.clone())
                        .unwrap_or_default();
                    let remember_password = Zeroizing::new(new_password.to_string());
                    cx.spawn(async move |this, cx| {
                        let task = cx.update(|cx| {
                            cx.write_credentials(
                                AUTH_CREDENTIAL_URL,
                                &username,
                                remember_password.as_bytes(),
                            )
                        });
                        let result = task.await;
                        this.update(cx, |state, cx| {
                            if let Err(error) = result {
                                state.set_toast(
                                    Some(format!(
                                        "Password changed, but saved credentials could not be updated: {error}"
                                    )),
                                    cx,
                                );
                            }
                            cx.notify();
                        })
                    })
                    .detach();
                }
            }
            Err(error) => self.set_toast(Some(error.message), cx),
        }
        cx.notify();
    }
}

fn profile_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}
