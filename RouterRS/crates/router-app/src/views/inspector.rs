//! Right-hand inspector: breadcrumb navigation + section cards for the
//! selected module / column / portal / portal-submodule.

use iced::widget::{button, checkbox, column, container, pick_list, row, slider, text, Space};
use iced::{Alignment, Element, Length};
use router_core::runtime::{McCommand, Scope};

use crate::message::Message;
use crate::selection::{PortalSub, Selection, TopModule};
use crate::widgets::{self, axis_dial, pilot_disk, status_dot};
use crate::{icons, theme};

use super::{action_buttons, labeled_input, section_card, tool_button, Ctx};

pub fn view<'a>(ctx: Ctx<'a>) -> Element<'a, Message> {
    let body: Element<Message> = match ctx.selection {
        Selection::Module(module) => module_inspector(ctx, module),
        Selection::Column(col) => column_inspector(ctx, col),
        Selection::Portal { col, target } => portal_inspector(ctx, col, target),
        Selection::PortalSub { col, target, sub } => portal_sub_inspector(ctx, col, target, sub),
        Selection::Source(index) => column![text(format!("Source {index}")).size(16)].into(),
    };
    column![breadcrumb(ctx), body].spacing(10).into()
}

// -------------------------------------------------------------- breadcrumb

fn crumb<'a>(label: String, message: Option<Message>) -> Element<'a, Message> {
    let mut b = button(text(label).size(12)).padding([2, 6]).style(theme::ghost);
    if let Some(message) = message {
        b = b.on_press(message);
    }
    b.into()
}

fn crumb_sep<'a>() -> Element<'a, Message> {
    icons::icon_sized(icons::CHEVRON_RIGHT, 10)
        .color(theme::TEXT_MUTED)
        .into()
}

fn breadcrumb<'a>(ctx: Ctx<'a>) -> Element<'a, Message> {
    let mut path = row![].spacing(2).align_y(Alignment::Center);
    match ctx.selection {
        Selection::Module(module) => {
            path = path.push(crumb(module.title().to_string(), None));
        }
        Selection::Column(col) => {
            path = path.push(crumb(
                "Installation".into(),
                Some(Message::Select(Selection::Module(TopModule::Installation))),
            ));
            path = path.push(crumb_sep());
            path = path.push(crumb(format!("Column {col}"), None));
        }
        Selection::Portal { col, target } | Selection::PortalSub { col, target, .. } => {
            path = path.push(crumb(
                "Installation".into(),
                Some(Message::Select(Selection::Module(TopModule::Installation))),
            ));
            path = path.push(crumb_sep());
            path = path.push(crumb(
                format!("Column {col}"),
                Some(Message::Select(Selection::Column(col))),
            ));
            path = path.push(crumb_sep());
            if let Selection::PortalSub { sub, .. } = ctx.selection {
                path = path.push(crumb(
                    format!("Portal {target}"),
                    Some(Message::Select(Selection::Portal { col, target })),
                ));
                path = path.push(crumb_sep());
                let sub_name = match sub {
                    PortalSub::Pilot => "Pilot".to_string(),
                    PortalSub::Axis(axis) => format!("Axis {}", if axis == 0 { "A" } else { "B" }),
                    PortalSub::MotorDriverSettings => "Motor".to_string(),
                    PortalSub::Logger => "Log".to_string(),
                };
                path = path.push(crumb(sub_name, None));
            } else {
                path = path.push(crumb(format!("Portal {target}"), None));
            }
        }
        Selection::Source(index) => {
            path = path.push(crumb(format!("Source {index}"), None));
        }
    }
    path.into()
}

// -------------------------------------------------------- module inspectors

fn module_inspector<'a>(ctx: Ctx<'a>, module: TopModule) -> Element<'a, Message> {
    match module {
        TopModule::Installation => {
            let (columns, rows, width, flipped) = ctx.snap.arrangement;
            column![
                section_card(
                    icons::LAYERS,
                    "ARRANGEMENT",
                    column![
                        labeled_input(ctx, "Columns", "arr.columns", columns.to_string()),
                        labeled_input(ctx, "Rows", "arr.rows", rows.to_string()),
                        labeled_input(ctx, "Column width", "arr.width", width.to_string()),
                        checkbox("Flipped", flipped)
                            .on_toggle(Message::ToggleFlipped)
                            .size(14)
                            .text_size(12),
                        tool_button(icons::REFRESH, "Rebuild columns", Message::RebuildColumns),
                    ]
                    .spacing(8)
                    .into(),
                ),
                section_card(
                    icons::SEND,
                    "MESSAGING",
                    column![
                        row![
                            text("Transmit").size(12).color(theme::TEXT_DIM).width(Length::Fixed(120.0)),
                            pick_list(
                                ["Individual", "Keyframe", "Disabled"].map(String::from).to_vec(),
                                Some(ctx.snap.transmit_mode.to_string()),
                                Message::TransmitModeSelected
                            )
                            .text_size(12)
                            .padding([4, 8]),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                        labeled_input(ctx, "Period [s]", "msg.period", format!("{}", ctx.snap.period_s)),
                        labeled_input(
                            ctx,
                            "Keyframe batch",
                            "msg.batch",
                            ctx.snap.keyframe_batch_size.to_string()
                        ),
                        checkbox("Keyframe velocities", ctx.snap.keyframe_velocities)
                            .on_toggle(Message::ToggleVelocities)
                            .size(14)
                            .text_size(12),
                        checkbox("Image enabled", ctx.snap.image_enabled)
                            .on_toggle(Message::ToggleImageEnabled)
                            .size(14)
                            .text_size(12),
                    ]
                    .spacing(8)
                    .into(),
                ),
                section_card(
                    icons::HDD_UPLOAD,
                    "MASS FIRMWARE UPDATE",
                    column![
                        text("All connected columns").size(11).color(theme::TEXT_MUTED),
                        row![
                            tool_button(icons::UPLOAD, "Upload .bin...", Message::FwUploadDialog(None)),
                            button(
                                row![icons::icon_sized(icons::ERASER, 12), text("Erase").size(11)]
                                    .spacing(5)
                                    .align_y(Alignment::Center)
                            )
                            .padding([5, 9])
                            .style(theme::danger)
                            .on_press(Message::FwErase(None)),
                            tool_button(icons::PLAY, "Run app", Message::FwRun(None)),
                        ]
                        .spacing(6),
                    ]
                    .spacing(8)
                    .into(),
                ),
                section_card(
                    icons::HOUSE,
                    "SYSTEM",
                    row![tool_button(
                        icons::HOUSE,
                        "Home and zero local",
                        Message::HomeAndZero
                    )]
                    .into(),
                ),
            ]
            .spacing(10)
            .into()
        }
        TopModule::Renderer => section_card(
            icons::IMAGE,
            "RENDERER",
            column![
                text(format!("{} sources", ctx.snap.sources.len())).size(12).color(theme::TEXT_DIM),
                checkbox("Image enabled", ctx.snap.image_enabled)
                    .on_toggle(Message::ToggleImageEnabled)
                    .size(14)
                    .text_size(12),
            ]
            .spacing(8)
            .into(),
        ),
        TopModule::Servers => section_card(
            icons::NETWORK,
            "SERVERS",
            column![
                text(format!("OSC UDP :{}", ctx.snap.osc_port)).size(12).color(theme::TEXT_DIM),
                text(format!("REST HTTP :{}", ctx.snap.rest_port)).size(12).color(theme::TEXT_DIM),
            ]
            .spacing(6)
            .into(),
        ),
        TopModule::Diagnostics => section_card(
            icons::HEART_PULSE,
            "DIAGNOSTICS",
            text("Select a unit in the panel to inspect it.")
                .size(12)
                .color(theme::TEXT_DIM)
                .into(),
        ),
    }
}

// --------------------------------------------------------- column inspector

fn column_inspector<'a>(ctx: Ctx<'a>, col: usize) -> Element<'a, Message> {
    let Some(column_snap) = ctx.snap.columns.get(col) else {
        return text("Column not found").into();
    };
    let stats = &column_snap.stats;

    let connection: Element<Message> = if stats.connected {
        column![
            row![
                icons::icon_sized(icons::PLUG_ZAP, 13).color(theme::OK),
                text("Connected").size(12).color(theme::OK),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            text(stats.device_description.clone()).size(11).color(theme::TEXT_DIM),
            tool_button(icons::PLUG, "Disconnect", Message::Disconnect(col)),
        ]
        .spacing(6)
        .into()
    } else {
        let mut list = column![
            row![
                icons::icon_sized(icons::PLUG, 13).color(theme::ERROR),
                text("Disconnected").size(12).color(theme::ERROR),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            text("Serial ports").size(11).color(theme::TEXT_MUTED),
        ]
        .spacing(6);
        for port in ctx.serial_ports {
            list = list.push(tool_button(
                icons::CABLE,
                port,
                Message::ConnectSerial(col, port.clone()),
            ));
        }
        list = list.push(tool_button(icons::REFRESH, "Refresh ports", Message::RefreshPorts));
        list = list.push(text("TCP gateways").size(11).color(theme::TEXT_MUTED));
        for host in ["192.168.1.201", "192.168.1.202"] {
            list = list.push(tool_button(
                icons::NETWORK,
                host,
                Message::ConnectTcp(col, host.to_string()),
            ));
        }
        list = list.push(labeled_input(ctx, "Custom TCP host", "tcp.host", String::new()));
        list.into()
    };

    column![
        section_card(icons::CABLE, "CONNECTION", connection),
        section_card(
            icons::ACTIVITY,
            "TRAFFIC",
            column![
                text(format!("Tx {}   Rx {}", stats.tx_count, stats.rx_count)).size(12),
                text(format!(
                    "ACK timeouts {}   decode errors {}",
                    stats.ack_timeouts, stats.decode_errors
                ))
                .size(11)
                .color(theme::TEXT_DIM),
                text(format!("Outbox {}", stats.outbox_size)).size(11).color(theme::TEXT_DIM),
                row![
                    tool_button(icons::TRASH, "Clear outbox", Message::ClearOutbox(col)),
                    tool_button(icons::ERASER, "Clear counters", Message::ClearCounters(col)),
                ]
                .spacing(6),
            ]
            .spacing(6)
            .into(),
        ),
        section_card(
            icons::CLOCK,
            "SCHEDULED POLL",
            column![
                checkbox("Scheduled poll", column_snap.scheduled_poll_enabled)
                    .on_toggle(move |v| Message::ToggleScheduledPoll(col, v))
                    .size(14)
                    .text_size(12),
                labeled_input(
                    ctx,
                    "Poll period [s]",
                    "col.poll.period",
                    format!("{}", column_snap.scheduled_poll_period_s)
                ),
            ]
            .spacing(8)
            .into(),
        ),
        section_card(
            icons::ANTENNA,
            "BROADCAST — THIS COLUMN",
            action_buttons(Scope::Column(col)),
        ),
        section_card(
            icons::HDD_UPLOAD,
            "FIRMWARE UPDATE",
            row![
                tool_button(icons::UPLOAD, "Upload .bin...", Message::FwUploadDialog(Some(col))),
                button(
                    row![icons::icon_sized(icons::ERASER, 12), text("Erase").size(11)]
                        .spacing(5)
                        .align_y(Alignment::Center)
                )
                .padding([5, 9])
                .style(theme::danger)
                .on_press(Message::FwErase(Some(col))),
                tool_button(icons::PLAY, "Run app", Message::FwRun(Some(col))),
            ]
            .spacing(6)
            .into(),
        ),
    ]
    .spacing(10)
    .into()
}

// --------------------------------------------------------- portal inspector

fn portal_header<'a>(ctx: Ctx<'a>, col: usize, target: u8) -> Element<'a, Message> {
    let Some(portal) = ctx.portal(col, target) else {
        return text("Portal not found").into();
    };
    let uptime = portal
        .up_time_ms
        .map(|ms| format!("{}s", ms / 1000))
        .unwrap_or_else(|| "—".into());
    let last_log = portal
        .last_log
        .as_ref()
        .map(|(level, message, count)| {
            let repeat = if *count > 1 { format!(" x{count}") } else { String::new() };
            (*level, format!("{message}{repeat}"))
        })
        .unwrap_or((0, "—".into()));

    let health = ctx.health.get(&(col as u8, target)).copied();
    let health_chip: Element<Message> = match health {
        Some(state) => super::state_chip(state),
        None => Space::with_width(0).into(),
    };

    column![
        row![
            text(format!("Portal {target}")).size(16),
            health_chip,
            Space::with_width(Length::Fill),
            status_dot(portal.last_rx_age_ms.map(|a| a < 200), "Rx"),
            status_dot(portal.last_tx_age_ms.map(|a| a < 200), "Tx"),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        text(format!(
            "Up {uptime} · FW {} · {}",
            portal.version.clone().unwrap_or_else(|| "—".into()),
            if portal.in_target_position { "in position" } else { "moving / unknown" }
        ))
        .size(11)
        .color(theme::TEXT_DIM),
        text(last_log.1).size(11).color(widgets::level_color(last_log.0)),
    ]
    .spacing(3)
    .into()
}

fn portal_nav<'a>(col: usize, target: u8, current: Option<PortalSub>) -> Element<'a, Message> {
    let nav_button = |glyph: char, label: &'static str, sub: PortalSub| {
        let is_current = current == Some(sub);
        button(
            row![icons::icon_sized(glyph, 11), text(label).size(11)]
                .spacing(4)
                .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(theme::segmented(is_current))
        .on_press(Message::Select(Selection::PortalSub { col, target, sub }))
    };
    container(
        row![
            nav_button(icons::CROSSHAIR, "Pilot", PortalSub::Pilot),
            nav_button(icons::CIRCLE_GAUGE, "Axis A", PortalSub::Axis(0)),
            nav_button(icons::CIRCLE_GAUGE, "Axis B", PortalSub::Axis(1)),
            nav_button(icons::CPU, "Motor", PortalSub::MotorDriverSettings),
            nav_button(icons::SCROLL_TEXT, "Log", PortalSub::Logger),
        ]
        .spacing(2),
    )
    .padding(3)
    .style(theme::card_inner)
    .into()
}

fn portal_inspector<'a>(ctx: Ctx<'a>, col: usize, target: u8) -> Element<'a, Message> {
    let Some(portal) = ctx.portal(col, target) else {
        return text("Portal not found").into();
    };
    column![
        portal_header(ctx, col, target),
        portal_nav(col, target, None),
        pilot_disk(col, target, portal, 300.0),
        row![
            tool_button(icons::REFRESH, "Poll", Message::Poll(Scope::Portal(col, target))),
            tool_button(icons::LOCATE_FIXED, "Poll position", Message::PilotPollPosition),
            tool_button(icons::SEND, "Push", Message::PilotPush),
        ]
        .spacing(4),
        section_card(icons::ANTENNA, "ACTIONS", action_buttons(Scope::Portal(col, target))),
        section_card(
            icons::CLOCK,
            "POLLING",
            column![
                checkbox("Poll regularly", portal.poll_regularly)
                    .on_toggle(Message::TogglePollRegularly)
                    .size(14)
                    .text_size(12),
                labeled_input(
                    ctx,
                    "Poll interval [s]",
                    "poll.interval",
                    format!("{}", portal.poll_interval_s)
                ),
            ]
            .spacing(8)
            .into(),
        ),
    ]
    .spacing(10)
    .into()
}

fn portal_sub_inspector<'a>(
    ctx: Ctx<'a>,
    col: usize,
    target: u8,
    sub: PortalSub,
) -> Element<'a, Message> {
    let Some(portal) = ctx.portal(col, target) else {
        return text("Portal not found").into();
    };
    let header = portal_header(ctx, col, target);
    let nav = portal_nav(col, target, Some(sub));

    let body: Element<Message> = match sub {
        PortalSub::Pilot => {
            let preset_row = |axis: usize| {
                row![
                    text(if axis == 0 { "A" } else { "B" }).size(12).color(theme::TEXT_DIM),
                    tool_button(icons::CHEVRON_RIGHT, "Left", Message::PilotSetAxis { col, target, axis, value: 0.0 }),
                    tool_button(icons::CHEVRON_RIGHT, "Up", Message::PilotSetAxis { col, target, axis, value: 0.25 }),
                    tool_button(icons::CHEVRON_RIGHT, "Right", Message::PilotSetAxis { col, target, axis, value: 0.5 }),
                    tool_button(icons::CHEVRON_RIGHT, "Down", Message::PilotSetAxis { col, target, axis, value: 0.75 }),
                ]
                .spacing(3)
                .align_y(Alignment::Center)
            };
            column![
                pilot_disk(col, target, portal, 300.0),
                row![
                    text(format!("Offset {:+.3}", portal.offset))
                        .size(12)
                        .color(theme::TEXT_DIM)
                        .width(Length::Fixed(90.0)),
                    slider(-0.25..=0.25, portal.offset, move |offset| Message::PilotOffset {
                        col,
                        target,
                        offset
                    })
                    .step(0.001),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                text(format!(
                    "position ({:+.3}, {:+.3})   polar r {:.3} θ {:+.3}   leading: {}",
                    portal.position.x, portal.position.y, portal.polar.x, portal.polar.y,
                    portal.leading_control
                ))
                .size(11)
                .color(theme::TEXT_DIM),
                row![
                    axis_dial(col, target, 0, portal, 130.0),
                    axis_dial(col, target, 1, portal, 130.0),
                ]
                .spacing(10),
                preset_row(0),
                preset_row(1),
                row![
                    tool_button(icons::ERASER, "Reset local (r)", Message::PilotResetLocal),
                    tool_button(icons::ROTATE_CW, "Unwind (u)", Message::PilotUnwind),
                    tool_button(icons::SEND, "Push (m)", Message::PilotPush),
                ]
                .spacing(4),
                row![
                    tool_button(icons::LOCATE_FIXED, "Poll position", Message::PilotPollPosition),
                    tool_button(icons::EYE, "See through", Message::PilotSeeThrough),
                    tool_button(icons::TARGET, "Take current", Message::PilotTakeCurrent),
                ]
                .spacing(4),
                checkbox("Send periodically", portal.send_periodically)
                    .on_toggle(Message::ToggleSendPeriodically)
                    .size(14)
                    .text_size(12),
            ]
            .spacing(8)
            .into()
        }
        PortalSub::Axis(axis) => {
            let mc = &portal.mc[axis.min(1)];
            let letter = if axis == 0 { "A" } else { "B" };
            column![
                section_card(
                    icons::ACTIVITY,
                    "STATUS",
                    column![
                        text(format!("Axis {letter} — MotionControl")).size(13),
                        text(format!(
                            "position {}   target {}",
                            mc.reported_position.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
                            mc.reported_target.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
                        ))
                        .size(12)
                        .color(theme::TEXT_DIM),
                        row![
                            icons::icon_sized(
                                if mc.health_ok == Some(false) { icons::ALERT } else { icons::CHECK_CIRCLE },
                                11
                            )
                            .color(match mc.health_ok {
                                Some(true) => theme::OK,
                                Some(false) => theme::ERROR,
                                None => theme::TEXT_MUTED,
                            }),
                            text(format!(
                                "calibration: {}",
                                match mc.health_ok {
                                    Some(true) => "OK",
                                    Some(false) => "NOT OK",
                                    None => "unreported",
                                }
                            ))
                            .size(12)
                            .color(match mc.health_ok {
                                Some(true) => theme::OK,
                                Some(false) => theme::ERROR,
                                None => theme::TEXT_MUTED,
                            }),
                        ]
                        .spacing(5)
                        .align_y(Alignment::Center),
                    ]
                    .spacing(6)
                    .into(),
                ),
                section_card(
                    icons::CIRCLE_GAUGE,
                    "MOTION PROFILE",
                    column![
                        labeled_input(ctx, "Max velocity", "mc.maxv", mc.max_velocity.to_string()),
                        labeled_input(ctx, "Acceleration", "mc.acc", mc.acceleration.to_string()),
                        labeled_input(ctx, "Min velocity", "mc.minv", mc.min_velocity.to_string()),
                        tool_button(
                            icons::SEND,
                            "Push motion profile",
                            Message::Mc { axis, kind: McCommand::PushMotionProfile }
                        ),
                    ]
                    .spacing(8)
                    .into(),
                ),
                section_card(
                    icons::WRENCH,
                    "ROUTINES",
                    column![
                        row![
                            tool_button(icons::TARGET, "Zero position", Message::Mc { axis, kind: McCommand::ZeroCurrentPosition }),
                            tool_button(icons::CIRCLE_GAUGE, "Measure backlash", Message::Mc { axis, kind: McCommand::MeasureBacklash }),
                        ]
                        .spacing(4),
                        row![
                            tool_button(icons::HOUSE, "Home routine", Message::Mc { axis, kind: McCommand::HomeRoutine }),
                            tool_button(icons::CLOCK, "Init timer", Message::Mc { axis, kind: McCommand::InitTimer }),
                            tool_button(icons::CLOCK, "Deinit timer", Message::Mc { axis, kind: McCommand::DeinitTimer }),
                        ]
                        .spacing(4),
                        row![
                            tool_button(icons::CLOCK, "Test timer", Message::Mc { axis, kind: McCommand::TestTimer }),
                            tool_button(icons::CPU, "MD test routine", Message::MdTestRoutine { axis }),
                            tool_button(icons::CPU, "MD test timer", Message::MdTestTimer { axis }),
                        ]
                        .spacing(4),
                    ]
                    .spacing(6)
                    .into(),
                ),
            ]
            .spacing(10)
            .into()
        }
        PortalSub::MotorDriverSettings => section_card(
            icons::CPU,
            "MOTOR DRIVER SETTINGS",
            column![
                labeled_input(ctx, "Current [A]", "mds.current", format!("{}", portal.mds_current_amps)),
                labeled_input(
                    ctx,
                    "Microstep res.",
                    "mds.microstep",
                    portal.mds_microstep_resolution.to_string()
                ),
                text("Sent to hardware on submit; microstep is transmitted as log2.")
                    .size(11)
                    .color(theme::TEXT_MUTED),
            ]
            .spacing(8)
            .into(),
        ),
        PortalSub::Logger => {
            let mut log_list = column![].spacing(2);
            if portal.logs.is_empty() {
                log_list = log_list.push(
                    text("No firmware log lines received.").size(11).color(theme::TEXT_MUTED),
                );
            }
            for (level, message, count) in portal.logs.iter().rev() {
                let repeat = if *count > 1 { format!("  x{count}") } else { String::new() };
                log_list = log_list.push(
                    text(format!("{message}{repeat}"))
                        .size(11)
                        .color(widgets::level_color(*level)),
                );
            }
            section_card(icons::SCROLL_TEXT, "FIRMWARE LOG (NEWEST FIRST)", log_list.into())
        }
    };

    column![header, nav, body].spacing(10).into()
}
