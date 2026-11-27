use std::borrow::Cow;

use gpui::prelude::*;
use gpui::{AssetSource, SharedString, div, px, rgb, svg};

use crate::theme::{colors, spacing};

const HOME: &[u8] = include_bytes!("../../assets/icons/home.svg");
const FRIENDS: &[u8] = include_bytes!("../../assets/icons/friends.svg");
const SETTINGS: &[u8] = include_bytes!("../../assets/icons/settings.svg");
const CHECK: &[u8] = include_bytes!("../../assets/icons/check.svg");
const PLUS: &[u8] = include_bytes!("../../assets/icons/plus.svg");
const CLOSE: &[u8] = include_bytes!("../../assets/icons/close.svg");
const TRASH: &[u8] = include_bytes!("../../assets/icons/trash.svg");
const MORE_VERTICAL: &[u8] = include_bytes!("../../assets/icons/more_vertical.svg");
const PAPERCLIP: &[u8] = include_bytes!("../../assets/icons/paperclip.svg");
const MICROPHONE: &[u8] = include_bytes!("../../assets/icons/microphone.svg");
const MICROPHONE_OFF: &[u8] = include_bytes!("../../assets/icons/microphone_off.svg");
const PHONE: &[u8] = include_bytes!("../../assets/icons/phone.svg");
const PHONE_OFF: &[u8] = include_bytes!("../../assets/icons/phone_off.svg");
const ARROW_UP: &[u8] = include_bytes!("../../assets/icons/arrow_up.svg");
const FILE: &[u8] = include_bytes!("../../assets/icons/file.svg");
const DOWNLOAD: &[u8] = include_bytes!("../../assets/icons/download.svg");
const STOP: &[u8] = include_bytes!("../../assets/icons/stop.svg");
const EYE: &[u8] = include_bytes!("../../assets/icons/eye.svg");
const PLAY: &[u8] = include_bytes!("../../assets/icons/play.svg");
const PAUSE: &[u8] = include_bytes!("../../assets/icons/pause.svg");
const IDENTITY: &[u8] = include_bytes!("../../assets/icons/identity.svg");
const DEVICES: &[u8] = include_bytes!("../../assets/icons/devices.svg");
const INFO: &[u8] = include_bytes!("../../assets/icons/info.svg");

pub struct IconAssets;

impl AssetSource for IconAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let icon = match path {
            "icons/home.svg" => HOME,
            "icons/friends.svg" => FRIENDS,
            "icons/settings.svg" => SETTINGS,
            "icons/check.svg" => CHECK,
            "icons/plus.svg" => PLUS,
            "icons/close.svg" => CLOSE,
            "icons/trash.svg" => TRASH,
            "icons/more_vertical.svg" => MORE_VERTICAL,
            "icons/paperclip.svg" => PAPERCLIP,
            "icons/microphone.svg" => MICROPHONE,
            "icons/microphone_off.svg" => MICROPHONE_OFF,
            "icons/phone.svg" => PHONE,
            "icons/phone_off.svg" => PHONE_OFF,
            "icons/arrow_up.svg" => ARROW_UP,
            "icons/file.svg" => FILE,
            "icons/download.svg" => DOWNLOAD,
            "icons/stop.svg" => STOP,
            "icons/eye.svg" => EYE,
            "icons/play.svg" => PLAY,
            "icons/pause.svg" => PAUSE,
            "icons/identity.svg" => IDENTITY,
            "icons/devices.svg" => DEVICES,
            "icons/info.svg" => INFO,
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(icon)))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Copy)]
pub enum IconName {
    Friends,
    Settings,
    Check,
    Plus,
    Close,
    Trash,
    MoreVertical,
    Paperclip,
    Microphone,
    MicrophoneOff,
    Phone,
    PhoneOff,
    ArrowUp,
    File,
    Download,
    Stop,
    Eye,
    Play,
    Pause,
    Identity,
    Devices,
    Info,
}

pub fn render(icon: IconName, size: f32, color: u32) -> impl IntoElement {
    let path = match icon {
        IconName::Friends => "icons/friends.svg",
        IconName::Settings => "icons/settings.svg",
        IconName::Check => "icons/check.svg",
        IconName::Plus => "icons/plus.svg",
        IconName::Close => "icons/close.svg",
        IconName::Trash => "icons/trash.svg",
        IconName::MoreVertical => "icons/more_vertical.svg",
        IconName::Paperclip => "icons/paperclip.svg",
        IconName::Microphone => "icons/microphone.svg",
        IconName::MicrophoneOff => "icons/microphone_off.svg",
        IconName::Phone => "icons/phone.svg",
        IconName::PhoneOff => "icons/phone_off.svg",
        IconName::ArrowUp => "icons/arrow_up.svg",
        IconName::File => "icons/file.svg",
        IconName::Download => "icons/download.svg",
        IconName::Stop => "icons/stop.svg",
        IconName::Eye => "icons/eye.svg",
        IconName::Play => "icons/play.svg",
        IconName::Pause => "icons/pause.svg",
        IconName::Identity => "icons/identity.svg",
        IconName::Devices => "icons/devices.svg",
        IconName::Info => "icons/info.svg",
    };

    div()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .child(svg().path(path).size(px(size)).text_color(rgb(color)))
}

pub fn checkbox(checked: bool) -> impl IntoElement {
    div()
        .size(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(spacing::RADIUS_SM))
        .border_1()
        .border_color(rgb(if checked {
            colors::ACCENT
        } else {
            colors::BORDER
        }))
        .bg(rgb(if checked {
            colors::ACCENT
        } else {
            colors::CANVAS
        }))
        .when(checked, |this| {
            this.child(render(IconName::Check, 12.0, colors::ACCENT_FOREGROUND))
        })
}
