//! Installation tab: broadcast action toolbar and the physical column
//! layout — columns side by side, each a card with its portal grid.

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Element, Length};
use router_core::runtime::{ColumnSnapshot, Scope};

use crate::message::Message;
use crate::selection::Selection;
use crate::widgets::PortalGrid;
use crate::{icons, theme};

use super::{action_buttons, chip, format_count, section_card, tool_button, Ctx};

pub fn panel<'a>(ctx: Ctx<'a>) -> Element<'a, Message> {
    let (cols, rows, width, _) = ctx.snap.arrangement;
    let title_block = column![
        row![
            icons::icon_sized(icons::HOUSE, 18).color(theme::ACCENT),
            text("Installation").size(19),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        text(format!(
            "{cols} columns × {rows} rows · column width {width} · portal 1 at the {}",
            if ctx.snap.columns.first().map(|c| c.flipped).unwrap_or(false) {
                "top"
            } else {
                "bottom"
            }
        ))
        .size(11)
        .color(theme::TEXT_MUTED),
    ]
    .spacing(3);

    let header = row![
        title_block,
        Space::with_width(Length::Fill),
        tool_button(icons::HOUSE, "Home & zero local", Message::HomeAndZero),
        tool_button(icons::REFRESH, "Rebuild columns", Message::RebuildColumns),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let mut columns_strip = row![].spacing(10).align_y(Alignment::Start);
    for col in &ctx.snap.columns {
        columns_strip = columns_strip.push(column_card(ctx, col));
    }
    let columns_scroller = scrollable(container(columns_strip).padding(iced::Padding {
        top: 0.0,
        right: 2.0,
        bottom: 10.0,
        left: 2.0,
    }))
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new(),
        ))
        .width(Length::Fill);

    scrollable(
        column![
            header,
            section_card(
                icons::ANTENNA,
                "BROADCAST — ALL COLUMNS",
                action_buttons(Scope::All),
            ),
            columns_scroller,
        ]
        .spacing(12),
    )
    .into()
}

/// One column as a vertical card: header, the portal grid at physical
/// proportions, compact stats below.
fn column_card<'a>(ctx: Ctx<'a>, col: &'a ColumnSnapshot) -> Element<'a, Message> {
    let selected_portal = match ctx.selection {
        Selection::Portal { col: c, target } | Selection::PortalSub { col: c, target, .. }
            if c == col.index =>
        {
            Some(target)
        }
        _ => None,
    };

    // Physical proportions: square-ish cells, sized so tall installations
    // (24 rows) still fit on screen.
    let cell: f32 = if col.count_y > 12 { 22.0 } else { 30.0 };
    let grid_w = col.count_x.max(1) as f32 * cell;
    let grid_h = col.count_y.max(1) as f32 * cell;
    let grid = iced::widget::canvas(PortalGrid::from_snapshot(
        col,
        selected_portal,
        ctx.health,
    ))
    .width(Length::Fixed(grid_w))
    .height(Length::Fixed(grid_h));

    let stats = &col.stats;
    let card_width = grid_w.max(108.0) + 20.0;
    let is_selected_column = matches!(ctx.selection, Selection::Column(c) if c == col.index);

    let plug_color = if stats.connected { theme::OK } else { theme::TEXT_MUTED };
    let header = button(
        row![
            icons::icon_sized(if stats.connected { icons::PLUG_ZAP } else { icons::PLUG }, 12)
                .color(plug_color),
            text(format!("Column {}", col.index)).size(13),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([3, 8])
    .style(move |theme: &iced::Theme, status| {
        if is_selected_column {
            theme::primary(theme, status)
        } else {
            theme::ghost(theme, status)
        }
    })
    .on_press(Message::Select(Selection::Column(col.index)));

    // The alert slot keeps a fixed height whether or not anything shows, so
    // the card never jumps; the outbox value is display-held by the App for
    // ~1.5 s after emptying so it doesn't strobe at the transmit cadence.
    let outbox = ctx.outbox_display.get(col.index).copied().unwrap_or(0);
    let mut alerts = row![].spacing(4).align_y(Alignment::Center);
    if outbox > 0 {
        // a couple of queued packets is normal transmit cadence; amber only
        // when a real backlog builds up
        let color = if outbox > 2 { theme::WARN } else { theme::TEXT_MUTED };
        alerts = alerts.push(chip(Some(icons::CLOCK), format!("outbox {outbox}"), color));
    }
    if stats.ack_timeouts > 0 {
        alerts = alerts.push(
            text(format!("{} timeouts", stats.ack_timeouts))
                .size(10)
                .color(theme::WARN),
        );
    }
    let alert_slot = container(alerts)
        .height(Length::Fixed(20.0))
        .align_y(Alignment::Center);

    let footer = column![
        text(format!(
            "Tx {}  Rx {}",
            format_count(stats.tx_count),
            format_count(stats.rx_count)
        ))
        .size(10)
        .color(theme::TEXT_MUTED),
        alert_slot,
    ]
    .spacing(1)
    .align_x(Alignment::Center);

    container(
        column![header, grid, footer]
            .spacing(6)
            .align_x(Alignment::Center),
    )
    .padding(10)
    .style(if is_selected_column {
        theme::card_selected
    } else {
        theme::card
    })
    .width(Length::Fixed(card_width))
    .into()
}
