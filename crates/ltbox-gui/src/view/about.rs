//! About panel: app icon, version, project links, and license.

use crate::*;
use iced::widget::{self, button, column, container, row, text};
use iced::{Element, Length, Theme};
use theme::mix_color;

const GITHUB_URL: &str = "https://github.com/miner7222/LTBox";
const WIKI_URL: &str = "https://github.com/miner7222/LTBox/wiki";

impl App {
    pub(crate) fn view_about(&self) -> Element<'_, Message> {
        let app_icon = about_app_icon(88.0);
        let title = text("LTBox").size(26);
        // No width cap: the text sizes to its content so it stays on one line
        // when the content area has room (the column centers it). A fixed
        // max_width forced a needless second line even on a wide window.
        let description = text(self.t("about_description").to_string())
            .size(12)
            .style(muted_style)
            .center();
        let version = text(concat!("v", env!("CARGO_PKG_VERSION")))
            .size(13)
            .style(muted_style);

        let links = row![
            about_link_button(
                icon::about_github(),
                GITHUB_URL,
                self.t("about_github").to_string(),
            ),
            about_link_button(
                icon::about_wiki(),
                WIKI_URL,
                self.t("about_wiki").to_string(),
            ),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);

        let license = text(format!("{}: GPL-3.0-or-later", self.t("about_license")))
            .size(12)
            .style(muted_style);

        let col = column![app_icon, title, description, version, links, license]
            .spacing(14)
            .align_x(iced::Alignment::Center);

        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }
}

/// App icon for the About panel. macOS uses the rounded squircle PNG; other
/// platforms use the scalable flat SVG logo so it stays crisp at any size.
fn about_app_icon(size: f32) -> Element<'static, Message> {
    #[cfg(target_os = "macos")]
    {
        iced::widget::image(iced::widget::image::Handle::from_bytes(
            include_bytes!("../../assets/icon_macos.png").as_slice(),
        ))
        .width(size)
        .height(size)
        .into()
    }
    #[cfg(not(target_os = "macos"))]
    {
        svg_icon(include_bytes!("../../assets/icon_source.svg"), size)
    }
}

/// Round icon-only button that opens `url` in the host browser, with a tooltip.
/// Styled like the Settings inline icon buttons (tonal `secondary_container`
/// base + pre-composited M3 state layer).
fn about_link_button(
    glyph: iced::widget::Text<'static, Theme, iced::Renderer>,
    url: &'static str,
    tip: String,
) -> Element<'static, Message> {
    let btn = button(
        container(lucide_icon(glyph, 20.0, |t: &Theme| {
            pal_of(t).on_secondary_container
        }))
        .width(40)
        .height(40)
        .center_x(40)
        .center_y(40),
    )
    .on_press(Message::OpenUrl(url))
    .padding(0)
    .style(|t: &Theme, status| {
        let p = pal_of(t);
        let base = p.secondary_container;
        let bg = match status {
            button::Status::Hovered => {
                mix_color(base, p.on_secondary_container, theme::state::HOVER)
            }
            button::Status::Pressed => {
                mix_color(base, p.on_secondary_container, theme::state::PRESSED)
            }
            _ => base,
        };
        button::Style {
            background: Some(bg.into()),
            border: iced::Border {
                radius: theme::shape::FULL.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    });
    widget::tooltip(
        btn,
        container(text(tip).size(11))
            .padding([6, 10])
            .style(|t: &Theme| theme::tooltip_style(t, theme::shape::XS)),
        widget::tooltip::Position::Top,
    )
    .into()
}
