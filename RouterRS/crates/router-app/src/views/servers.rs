//! Servers tab: OSC receiver + REST server status and route reference.

use iced::widget::{column, container, row, scrollable, text, Space};
use iced::{Alignment, Element, Font, Length};

use crate::message::Message;
use crate::{icons, theme};

use super::{chip, Ctx};

fn server_card<'a>(
    glyph: char,
    title: &'a str,
    running: bool,
    subtitle: String,
    routes: &'a [&'a str],
) -> Element<'a, Message> {
    let status = if running {
        chip(None, "listening".into(), theme::OK)
    } else {
        chip(None, "disabled".into(), theme::ERROR)
    };

    let mut route_list = column![].spacing(3);
    for route in routes {
        route_list = route_list.push(
            text(*route)
                .size(11)
                .font(Font::MONOSPACE)
                .color(theme::TEXT_DIM),
        );
    }

    container(
        column![
            row![
                icons::icon_sized(glyph, 15).color(theme::ACCENT),
                text(title).size(15),
                Space::with_width(Length::Fill),
                status,
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text(subtitle).size(12).color(theme::TEXT_DIM),
            container(route_list)
                .padding(10)
                .width(Length::Fill)
                .style(theme::card_inner),
        ]
        .spacing(10),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::card)
    .into()
}

pub fn panel<'a>(ctx: Ctx<'a>) -> Element<'a, Message> {
    const OSC_ROUTES: [&str; 10] = [
        "/move <col> <portal> <x> <y>",
        "/unwind",
        "/motionProfile <maxV> <acc> <minV>",
        "/setCurrent <amps>",
        "/homeAndZeroLocal",
        "/disableLights",
        "/axesMoveBlock <col> <a> <b> ...",
        "/axesMoveByInidices ...   (sic)",
        "/[col]/<action>",
        "/[col]/[portal]/<action>",
    ];
    const REST_ROUTES: [&str; 7] = [
        "GET /                              -> \"true\"",
        "GET /<col>/<portal>/setPosition/<x>,<y>",
        "GET /<col>/<portal>/getPosition",
        "GET /<col>/<portal>/getTargetPosition",
        "GET /<col>/<portal>/isInPosition",
        "GET /<col>/<portal>/pollPosition",
        "GET /<col>/<portal>/push",
    ];

    let header = column![
        row![
            icons::icon_sized(icons::NETWORK, 18).color(theme::ACCENT),
            text("Servers").size(19),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        text("External control interfaces — same routes and semantics as the C++ app")
            .size(11)
            .color(theme::TEXT_MUTED),
    ]
    .spacing(3);

    scrollable(
        column![
            header,
            row![
                server_card(
                    icons::RADIO,
                    "OSC Receiver",
                    ctx.snap.osc_running,
                    format!(
                        "UDP port {} · receive only · {} msg/frame",
                        ctx.snap.osc_port, ctx.snap.osc_messages_per_tick
                    ),
                    &OSC_ROUTES,
                ),
                server_card(
                    icons::SERVER,
                    "REST Server",
                    ctx.snap.rest_running,
                    format!("HTTP port {} · GET only", ctx.snap.rest_port),
                    &REST_ROUTES,
                ),
            ]
            .spacing(12)
            .align_y(Alignment::Start),
        ]
        .spacing(12),
    )
    .into()
}
