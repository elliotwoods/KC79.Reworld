//! Bottom status bar: connection totals, traffic rates, fault counts,
//! server status, and the report session file.

use iced::widget::{container, row, text, Space};
use iced::{Alignment, Color, Element, Length};

use crate::message::Message;
use crate::{icons, theme};

use super::{format_rate, Ctx};

fn item<'a>(glyph: char, color: Color, label: String) -> Element<'a, Message> {
    row![
        icons::icon_sized(glyph, 11).color(color),
        text(label).size(11).color(theme::TEXT_DIM),
    ]
    .spacing(5)
    .align_y(Alignment::Center)
    .into()
}

fn dot_item<'a>(on: bool, label: String) -> Element<'a, Message> {
    row![
        text("●")
            .size(9)
            .color(if on { theme::OK } else { theme::TEXT_MUTED }),
        text(label).size(11).color(theme::TEXT_DIM),
    ]
    .spacing(5)
    .align_y(Alignment::Center)
    .into()
}

pub fn view<'a>(ctx: Ctx<'a>) -> Element<'a, Message> {
    let total = ctx.snap.columns.len();
    let connected = ctx.snap.columns.iter().filter(|c| c.stats.connected).count();
    let conn_color = if total == 0 || connected == 0 {
        theme::ERROR
    } else if connected < total {
        theme::WARN
    } else {
        theme::OK
    };

    let faults = ctx.diag.recent_faults.len();
    let faulty_units = ctx.faulty_units();
    let fault_color = if faulty_units > 0 {
        theme::ERROR
    } else if faults > 0 {
        theme::WARN
    } else {
        theme::TEXT_MUTED
    };

    let mut bar = row![
        item(icons::CABLE, conn_color, format!("{connected}/{total} columns")),
        item(
            icons::ARROW_UP_DOWN,
            theme::TEXT_MUTED,
            format!(
                "Tx {}  Rx {}",
                format_rate(ctx.rates.tx_per_s),
                format_rate(ctx.rates.rx_per_s)
            ),
        ),
        item(
            icons::ALERT,
            fault_color,
            format!("{faults} recent faults · {faulty_units} faulty units"),
        ),
        Space::with_width(Length::Fill),
        dot_item(
            ctx.snap.osc_running,
            format!("OSC :{}", ctx.snap.osc_port),
        ),
        dot_item(
            ctx.snap.rest_running,
            format!("REST :{}", ctx.snap.rest_port),
        ),
        item(
            icons::FILE_TEXT,
            theme::TEXT_MUTED,
            format!(
                "{} · {} KB",
                ctx.diag.session_file,
                ctx.diag.file_bytes / 1024
            ),
        ),
    ]
    .spacing(18)
    .align_y(Alignment::Center);

    if ctx.diag.dropped_events > 0 {
        bar = bar.push(item(
            icons::BUG,
            theme::WARN,
            format!("{} dropped events", ctx.diag.dropped_events),
        ));
    }
    if ctx.diag.verbose {
        bar = bar.push(super::chip(
            Some(icons::TERMINAL),
            "VERBOSE".into(),
            theme::WARN,
        ));
    }

    container(bar)
        .width(Length::Fill)
        .padding([6, 14])
        .style(theme::status_bar)
        .into()
}
