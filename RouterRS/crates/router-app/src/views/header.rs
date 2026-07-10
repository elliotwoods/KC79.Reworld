//! Top header bar: wordmark, module tabs, and global quick actions.

use iced::font::Weight;
use iced::widget::{button, container, pick_list, row, text, Space};
use iced::{Alignment, Element, Font, Length};

use crate::message::Message;
use crate::selection::TopModule;
use crate::{icons, theme};

use super::Ctx;

pub fn view<'a>(ctx: Ctx<'a>) -> Element<'a, Message> {
    let bold = Font {
        weight: Weight::Bold,
        ..Font::DEFAULT
    };

    let wordmark = row![
        icons::icon_sized(icons::ROUTE, 18).color(theme::ACCENT),
        text("ROUTER").size(15).font(bold),
        text("RS").size(15).font(bold).color(theme::ACCENT),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let mut tabs = row![].spacing(2).align_y(Alignment::Center);
    for module in TopModule::ALL {
        let selected = ctx.center == module;
        let mut label = row![
            icons::icon_sized(module.icon(), 13),
            text(module.title()).size(13),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        if module == TopModule::Diagnostics {
            let faulty = ctx.faulty_units();
            if faulty > 0 {
                label = label.push(
                    container(text(faulty.to_string()).size(10))
                        .padding([1, 6])
                        .style(theme::badge(theme::ERROR)),
                );
            }
        }

        tabs = tabs.push(
            button(label)
                .padding([6, 12])
                .style(theme::tab(selected))
                .on_press(Message::SelectCenter(module)),
        );
    }

    let transmit = row![
        icons::icon_sized(icons::SEND, 12).color(theme::TEXT_DIM),
        pick_list(
            ["Individual", "Keyframe", "Disabled"].map(String::from).to_vec(),
            Some(ctx.snap.transmit_mode.to_string()),
            Message::TransmitModeSelected,
        )
        .text_size(12)
        .padding([4, 8]),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let image_toggle = button(
        row![
            icons::icon_sized(icons::IMAGE, 12),
            text(if ctx.snap.image_enabled { "Image on" } else { "Image off" }).size(11),
        ]
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .padding([5, 9])
    .style(theme::toggle(ctx.snap.image_enabled, theme::OK))
    .on_press(Message::ToggleImageEnabled(!ctx.snap.image_enabled));

    let save = super::tool_button(icons::SAVE, "Save config", Message::SaveConfig);

    container(
        row![
            wordmark,
            Space::with_width(Length::Fixed(18.0)),
            tabs,
            Space::with_width(Length::Fill),
            transmit,
            image_toggle,
            save,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([8, 14])
    .style(theme::header_bar)
    .into()
}
