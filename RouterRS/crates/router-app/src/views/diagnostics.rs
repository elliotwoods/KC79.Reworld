//! Diagnostics tab: stat tiles, the installation health heatmap, per-column
//! connection table, worst units, and the live fault feed.

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Color, Element, Length};
use router_report::{ColumnState, PortalState};

use crate::message::Message;
use crate::selection::Selection;
use crate::widgets::HealthHeatmap;
use crate::{icons, theme};

use super::{chip, format_count, section_card, state_chip, tool_button, Ctx};

fn stat_tile<'a>(
    glyph: char,
    label: &'a str,
    value: String,
    color: Color,
) -> Element<'a, Message> {
    container(
        column![
            row![
                icons::icon_sized(glyph, 12).color(color),
                text(label).size(10).color(theme::TEXT_MUTED),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
            text(value).size(20).color(color),
        ]
        .spacing(4),
    )
    .padding(12)
    .width(Length::Fill)
    .style(theme::card)
    .into()
}

/// Tiny horizontal bar: `value` against `scale`, colored.
fn mini_bar<'a>(value: f32, scale: f32, width: f32, color: Color) -> Element<'a, Message> {
    let frac = (value / scale).clamp(0.0, 1.0);
    container(
        container(Space::with_height(6))
            .width(Length::Fixed((width * frac).max(2.0)))
            .style(theme::bar_fill(color)),
    )
    .width(Length::Fixed(width))
    .style(theme::bar_track)
    .into()
}

fn column_state_color(state: ColumnState) -> Color {
    match state {
        ColumnState::Connected => theme::OK,
        ColumnState::Disconnected => theme::ERROR,
        ColumnState::Stalled | ColumnState::Noisy => theme::WARN,
    }
}

fn fault_style(kind: &str) -> (char, Color) {
    match kind {
        "ack_timeout" => (icons::CLOCK, theme::WARN),
        "cobs_error" | "msgpack_error" => (icons::BUG, theme::ERROR),
        "device_disconnect" | "disconnect" => (icons::PLUG, theme::ERROR),
        "health_transition" => (icons::HEART_PULSE, theme::WARN),
        _ => (icons::ALERT, theme::WARN),
    }
}

fn epoch_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn age_text(ts_ms: u64, now_ms: u64) -> String {
    let age_s = now_ms.saturating_sub(ts_ms) / 1000;
    if age_s < 60 {
        format!("{age_s}s")
    } else if age_s < 3600 {
        format!("{}m", age_s / 60)
    } else {
        format!("{}h", age_s / 3600)
    }
}

pub fn panel<'a>(ctx: Ctx<'a>) -> Element<'a, Message> {
    let diag = ctx.diag;

    // ---------------------------------------------------------- header row
    let header = row![
        column![
            row![
                icons::icon_sized(icons::HEART_PULSE, 18).color(theme::ACCENT),
                text("Diagnostics").size(19),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text(format!(
                "session {} · {} KB{}",
                diag.session_file,
                diag.file_bytes / 1024,
                if diag.dropped_events > 0 {
                    format!(" · {} dropped events", diag.dropped_events)
                } else {
                    String::new()
                }
            ))
            .size(11)
            .color(theme::TEXT_MUTED),
        ]
        .spacing(3),
        Space::with_width(Length::Fill),
        tool_button(icons::FILE_TEXT, "Write summary", Message::WriteSummaryNow),
        button(
            row![
                icons::icon_sized(icons::TERMINAL, 12),
                text(if diag.verbose { "Verbose on" } else { "Verbose off" }).size(11),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding([5, 9])
        .style(theme::toggle(diag.verbose, theme::WARN))
        .on_press(Message::ToggleVerbose),
        text_input("marker label...", ctx.marker_text)
            .on_input(Message::MarkerText)
            .on_submit(Message::AddMarker)
            .width(Length::Fixed(170.0))
            .size(12),
        tool_button(icons::FLAG, "Mark", Message::AddMarker),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    // ----------------------------------------------------------- stat tiles
    let total_tx: u64 = diag.columns.iter().map(|c| c.tx).sum();
    let total_rx: u64 = diag.columns.iter().map(|c| c.rx).sum();
    let total_timeouts: u64 = diag.columns.iter().map(|c| c.timeouts).sum();
    let total_decode: u64 = diag
        .columns
        .iter()
        .map(|c| c.cobs_errors + c.msgpack_errors)
        .sum();
    let faulty_units = ctx.faulty_units();

    let nonzero = |v: u64, color: Color| if v > 0 { color } else { theme::TEXT_DIM };
    let tiles = row![
        stat_tile(icons::SEND, "PACKETS TX", format_count(total_tx), theme::TEXT),
        stat_tile(icons::ACTIVITY, "PACKETS RX", format_count(total_rx), theme::TEXT),
        stat_tile(
            icons::CLOCK,
            "ACK TIMEOUTS",
            format_count(total_timeouts),
            nonzero(total_timeouts, theme::WARN),
        ),
        stat_tile(
            icons::BUG,
            "DECODE ERRORS",
            format_count(total_decode),
            nonzero(total_decode, theme::ERROR),
        ),
        stat_tile(
            icons::ALERT,
            "FAULTY UNITS",
            faulty_units.to_string(),
            nonzero(faulty_units as u64, theme::ERROR),
        ),
    ]
    .spacing(10);

    // -------------------------------------------------------------- heatmap
    let heatmap_h = 120.0_f32.max(ctx.snap.columns.iter().map(|c| c.count_y).max().unwrap_or(6) as f32 * 9.0);
    let heatmap: Element<Message> = if ctx.snap.columns.is_empty() {
        text("No columns").size(12).color(theme::TEXT_MUTED).into()
    } else {
        iced::widget::canvas(HealthHeatmap::from_snapshots(
            &ctx.snap.columns,
            ctx.health,
            ctx.scores,
        ))
        .width(Length::Fill)
        .height(Length::Fixed(heatmap_h))
        .into()
    };
    let legend = row![
        text("click a cell to inspect").size(10).color(theme::TEXT_MUTED),
        Space::with_width(Length::Fill),
        state_chip(PortalState::Ok),
        state_chip(PortalState::Degraded),
        state_chip(PortalState::Faulty),
        state_chip(PortalState::Silent),
        state_chip(PortalState::Unknown),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let heatmap_card = section_card(
        icons::LAYERS,
        "UNIT HEALTH",
        column![heatmap, legend].spacing(8).into(),
    );

    // ---------------------------------------------------- connections table
    let head = |label: &'static str, width: f32| {
        text(label)
            .size(10)
            .color(theme::TEXT_MUTED)
            .width(Length::Fixed(width))
    };
    let mut table = column![row![
        head("COL", 36.0),
        head("STATE", 96.0),
        head("ENDPOINT", 150.0),
        head("TX", 64.0),
        head("RX", 64.0),
        head("TIMEOUT", 62.0),
        head("COBS", 48.0),
        head("MSGPK", 52.0),
        head("LATENCY p50/p90/p99", 190.0),
    ]
    .spacing(6)]
    .spacing(0);

    for (i, col) in diag.columns.iter().enumerate() {
        let cell = |content: String, width: f32, color: Color| {
            text(content).size(12).color(color).width(Length::Fixed(width))
        };
        let latency = row![
            mini_bar(col.latency_p90_ms, 300.0, 60.0, column_state_color(col.state)),
            text(format!(
                "{:.0}/{:.0}/{:.0} ms",
                col.latency_p50_ms, col.latency_p90_ms, col.latency_p99_ms
            ))
            .size(11)
            .color(theme::TEXT_DIM),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        table = table.push(
            container(
                row![
                    cell(col.col.to_string(), 36.0, theme::TEXT),
                    container(chip(None, col.state.as_str().into(), column_state_color(col.state)))
                        .width(Length::Fixed(96.0)),
                    cell(col.endpoint.clone(), 150.0, theme::TEXT_DIM),
                    cell(format_count(col.tx), 64.0, theme::TEXT),
                    cell(format_count(col.rx), 64.0, theme::TEXT),
                    cell(col.timeouts.to_string(), 62.0, nonzero(col.timeouts, theme::WARN)),
                    cell(col.cobs_errors.to_string(), 48.0, nonzero(col.cobs_errors, theme::ERROR)),
                    cell(col.msgpack_errors.to_string(), 52.0, nonzero(col.msgpack_errors, theme::ERROR)),
                    latency,
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([4, 6])
            .style(theme::zebra(i)),
        );
    }
    let table_card = section_card(icons::CABLE, "CONNECTIONS", table.into());

    // ----------------------------------------------------------- worst units
    let mut worst: Vec<_> = diag.portals.iter().collect();
    worst.sort_by_key(|p| p.score);
    let mut worst_list = column![].spacing(2);
    if worst.is_empty() {
        worst_list = worst_list.push(
            text("No unit data yet — enable scheduled polling to score units.")
                .size(11)
                .color(theme::TEXT_MUTED),
        );
    }
    for portal in worst.iter().take(12) {
        let color = theme::state_color(portal.state);
        worst_list = worst_list.push(
            button(
                row![
                    text(format!("col {} · portal {}", portal.col, portal.portal))
                        .size(12)
                        .width(Length::Fixed(130.0)),
                    container(state_chip(portal.state)).width(Length::Fixed(90.0)),
                    mini_bar(portal.score as f32, 100.0, 70.0, color),
                    text(format!("{}", portal.score)).size(11).color(color).width(Length::Fixed(30.0)),
                    text(format!("ack {:.0}%", portal.ack_rate * 100.0))
                        .size(11)
                        .color(theme::TEXT_DIM)
                        .width(Length::Fixed(66.0)),
                    text(format!("{} err logs", portal.error_logs))
                        .size(11)
                        .color(nonzero(portal.error_logs, theme::WARN)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .style(theme::ghost)
            .on_press(Message::Select(Selection::Portal {
                col: portal.col as usize,
                target: portal.portal,
            })),
        );
    }
    let worst_card = section_card(icons::ALERT, "WORST UNITS", worst_list.into());

    // ------------------------------------------------------------ fault feed
    let now_ms = epoch_ms_now();
    let mut fault_feed = column![].spacing(3);
    if diag.recent_faults.is_empty() {
        fault_feed = fault_feed.push(
            text("No faults recorded — all quiet.")
                .size(11)
                .color(theme::TEXT_MUTED),
        );
    }
    for fault in diag.recent_faults.iter().rev().take(30) {
        let (glyph, color) = fault_style(&fault.kind);
        let place = match fault.portal {
            Some(p) => format!("col {} · p{}", fault.col, p),
            None => format!("col {}", fault.col),
        };
        let mut line = row![
            text(age_text(fault.ts_ms, now_ms))
                .size(10)
                .color(theme::TEXT_MUTED)
                .width(Length::Fixed(34.0)),
            icons::icon_sized(glyph, 11).color(color),
            text(fault.kind.clone()).size(11).color(color).width(Length::Fixed(110.0)),
            text(place).size(11).color(theme::TEXT_DIM).width(Length::Fixed(84.0)),
            text(fault.detail.clone()).size(11).color(theme::TEXT_DIM),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if fault.repeat > 1 {
            line = line.push(chip(None, format!("×{}", fault.repeat), theme::WARN));
        }
        fault_feed = fault_feed.push(line);
    }
    let feed_card = section_card(icons::SCROLL_TEXT, "RECENT FAULTS", fault_feed.into());

    scrollable(
        column![header, tiles, heatmap_card, table_card, worst_card, feed_card].spacing(12),
    )
    .into()
}
