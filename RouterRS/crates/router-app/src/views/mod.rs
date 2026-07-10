//! View functions. Layout: header (tabs + quick actions) / center panel +
//! inspector / status bar. State lives in router-core; these are pure
//! projections of `UiSnapshot` + `DiagnosticsSnapshot`.

mod diagnostics;
mod header;
mod inspector;
mod installation;
mod renderer;
mod servers;
mod statusbar;

use std::collections::HashMap;

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};
use router_core::proto::commands::ActionKind;
use router_core::runtime::{PortalSnapshot, Scope, UiSnapshot};
use router_report::{DiagnosticsSnapshot, PortalState};

use crate::message::Message;
use crate::selection::{Selection, TopModule};
use crate::{icons, theme};

/// Aggregate Tx/Rx rates computed by the App on each tick (EMA, per second).
#[derive(Debug, Clone, Copy, Default)]
pub struct Rates {
    pub tx_per_s: f32,
    pub rx_per_s: f32,
}

#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub snap: &'a UiSnapshot,
    pub diag: &'a DiagnosticsSnapshot,
    pub selection: Selection,
    pub center: TopModule,
    pub edits: &'a HashMap<&'static str, String>,
    pub serial_ports: &'a [String],
    pub marker_text: &'a str,
    pub rates: Rates,
    /// (col, portal) -> health state / score, derived from `diag.portals`.
    pub health: &'a HashMap<(u8, u8), PortalState>,
    pub scores: &'a HashMap<(u8, u8), u8>,
    /// Per-column outbox size, held briefly after emptying (anti-strobe).
    pub outbox_display: &'a [usize],
}

impl Ctx<'_> {
    fn portal(&self, col: usize, target: u8) -> Option<&PortalSnapshot> {
        self.snap
            .columns
            .get(col)?
            .portals
            .iter()
            .find(|p| p.target == target)
    }

    fn edit_value(&self, id: &'static str, live: String) -> String {
        self.edits.get(id).cloned().unwrap_or(live)
    }

    fn faulty_units(&self) -> usize {
        self.diag
            .portals
            .iter()
            .filter(|p| matches!(p.state, PortalState::Faulty | PortalState::Silent))
            .count()
    }
}

pub fn root<'a>(ctx: Ctx<'a>) -> Element<'a, Message> {
    let center: Element<Message> = match ctx.center {
        TopModule::Installation => installation::panel(ctx),
        TopModule::Renderer => renderer::panel(ctx),
        TopModule::Servers => servers::panel(ctx),
        TopModule::Diagnostics => diagnostics::panel(ctx),
    };

    column![
        header::view(ctx),
        row![
            container(center)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(14),
            container(scrollable(inspector::view(ctx)).width(Length::Fill))
                .width(Length::Fixed(340.0))
                .height(Length::Fill)
                .padding(12)
                .style(theme::inspector_panel),
        ]
        .width(Length::Fill)
        .height(Length::Fill),
        statusbar::view(ctx),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

// --------------------------------------------------------------- shared bits

/// Section header: icon + small-caps-ish title.
pub fn section_title<'a>(glyph: char, title: &'a str) -> Element<'a, Message> {
    row![
        icons::icon_sized(glyph, 13).color(theme::TEXT_DIM),
        text(title).size(12).color(theme::TEXT_DIM),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// A card with a section header and body content.
pub fn section_card<'a>(
    glyph: char,
    title: &'a str,
    body: Element<'a, Message>,
) -> Element<'a, Message> {
    container(column![section_title(glyph, title), body].spacing(8))
        .padding(12)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

pub fn labeled_input<'a>(
    ctx: Ctx<'a>,
    label: &'static str,
    id: &'static str,
    live: String,
) -> Element<'a, Message> {
    row![
        text(label).size(12).color(theme::TEXT_DIM).width(Length::Fixed(120.0)),
        text_input("", &ctx.edit_value(id, live))
            .on_input(move |s| Message::Edit(id, s))
            .on_submit(Message::Submit(id))
            .size(12)
            .width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// Icon + label tool button.
pub fn tool_button<'a>(glyph: char, label: &'a str, message: Message) -> Element<'a, Message> {
    button(
        row![icons::icon_sized(glyph, 12), text(label).size(11)]
            .spacing(5)
            .align_y(Alignment::Center),
    )
    .padding([5, 9])
    .style(theme::tool)
    .on_press(message)
    .into()
}

fn danger_button<'a>(glyph: char, label: &'a str, message: Message) -> Element<'a, Message> {
    button(
        row![icons::icon_sized(glyph, 12), text(label).size(11)]
            .spacing(5)
            .align_y(Alignment::Center),
    )
    .padding([5, 9])
    .style(theme::danger)
    .on_press(message)
    .into()
}

fn action_icon(action: ActionKind) -> char {
    match action {
        ActionKind::Ping => icons::LOCATE_FIXED,
        ActionKind::Init => icons::SETTINGS,
        ActionKind::Calibrate => icons::CIRCLE_GAUGE,
        ActionKind::Home => icons::HOUSE,
        ActionKind::FlashLeds => icons::SIREN,
        ActionKind::GoHome => icons::TARGET,
        ActionKind::SeeThrough => icons::EYE,
        ActionKind::DisableDebugLights => icons::LIGHTBULB_OFF,
        ActionKind::EnableDebugLights => icons::LIGHTBULB,
        ActionKind::Unjam => icons::WRENCH,
        ActionKind::EscapeFromRoutine => icons::UNLOCK,
        ActionKind::Reboot => icons::ROTATE_CW,
    }
}

fn action_label(action: ActionKind) -> &'static str {
    match action {
        ActionKind::Ping => "Ping",
        ActionKind::Init => "Initialise",
        ActionKind::Calibrate => "Calibrate",
        ActionKind::Home => "Home",
        ActionKind::FlashLeds => "Flash LEDs",
        ActionKind::GoHome => "Go home",
        ActionKind::SeeThrough => "See through",
        ActionKind::DisableDebugLights => "Lights off",
        ActionKind::EnableDebugLights => "Lights on",
        ActionKind::Unjam => "Unjam",
        ActionKind::EscapeFromRoutine => "Escape",
        ActionKind::Reboot => "Reboot",
    }
}

/// The action toolbar, grouped semantically (Poll / Identify / Motion /
/// Setup / Danger) instead of a flat grid.
pub fn action_buttons<'a>(scope: Scope) -> Element<'a, Message> {
    let action = |kind: ActionKind| {
        tool_button(action_icon(kind), action_label(kind), Message::Action(scope, kind))
    };

    let group = |label: &'static str, buttons: Vec<Element<'a, Message>>| {
        column![
            text(label).size(9).color(theme::TEXT_MUTED),
            row(buttons).spacing(4).wrap(),
        ]
        .spacing(3)
    };

    row![
        group("STATUS", vec![tool_button(icons::REFRESH, "Poll", Message::Poll(scope))]),
        group(
            "IDENTIFY",
            vec![
                action(ActionKind::Ping),
                action(ActionKind::FlashLeds),
                action(ActionKind::EnableDebugLights),
                action(ActionKind::DisableDebugLights),
            ],
        ),
        group(
            "MOTION",
            vec![
                action(ActionKind::Home),
                action(ActionKind::GoHome),
                action(ActionKind::SeeThrough),
                action(ActionKind::Unjam),
                action(ActionKind::EscapeFromRoutine),
            ],
        ),
        group(
            "SETUP",
            vec![action(ActionKind::Init), action(ActionKind::Calibrate)],
        ),
        group(
            "DANGER",
            vec![danger_button(
                icons::ROTATE_CW,
                "Reboot",
                Message::Action(scope, ActionKind::Reboot),
            )],
        ),
    ]
    .spacing(16)
    .wrap()
    .into()
}

/// Small translucent pill with icon + text.
pub fn chip<'a>(glyph: Option<char>, label: String, color: iced::Color) -> Element<'a, Message> {
    let mut content = row![].spacing(4).align_y(Alignment::Center);
    if let Some(glyph) = glyph {
        content = content.push(icons::icon_sized(glyph, 10).color(color));
    }
    content = content.push(text(label).size(10).color(color));
    container(content)
        .padding([2, 8])
        .style(theme::chip(color))
        .into()
}

pub fn state_chip<'a>(state: PortalState) -> Element<'a, Message> {
    chip(None, state.as_str().to_string(), theme::state_color(state))
}

/// Format a per-second rate compactly.
pub fn format_rate(v: f32) -> String {
    if v >= 1000.0 {
        format!("{:.1}k/s", v / 1000.0)
    } else {
        format!("{v:.0}/s")
    }
}

pub fn format_count(v: u64) -> String {
    if v >= 1_000_000 {
        format!("{:.1}M", v as f64 / 1_000_000.0)
    } else if v >= 10_000 {
        format!("{:.1}k", v as f64 / 1000.0)
    } else {
        v.to_string()
    }
}
