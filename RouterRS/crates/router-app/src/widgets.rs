//! Custom canvas widgets: the pilot polar disk, axis dials, portal grids,
//! the diagnostics health heatmap, and small indicator helpers.

use std::collections::HashMap;

use glam::vec2;
use iced::mouse;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke, Text as CanvasText};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use router_core::runtime::{ColumnSnapshot, PortalSnapshot};
use router_report::PortalState;

use crate::message::Message;
use crate::selection::Selection;
use crate::theme::{
    self, ACCENT, GRID_LINE, LIVE_BLUE, TARGET_WHITE, TEXT_MUTED,
};

/// The C++ disk warps radius by pow(r, 0.4) for finer center control.
const DISK_WARP: f32 = 0.4;

/// Heartbeat indicators fade out over this long after the last packet.
const HEARTBEAT_FADE_MS: f32 = 300.0;

fn heartbeat_alpha(age_ms: Option<u64>) -> f32 {
    match age_ms {
        Some(age) => (1.0 - age as f32 / HEARTBEAT_FADE_MS).clamp(0.0, 1.0),
        None => 0.0,
    }
}

// ---------------------------------------------------------------- pilot disk

pub struct PilotDisk {
    pub col: usize,
    pub target: u8,
    pub position: glam::Vec2,
    pub live_position: Option<glam::Vec2>,
    pub live_target_position: Option<glam::Vec2>,
}

#[derive(Default)]
pub struct DiskState {
    dragging: bool,
}

impl PilotDisk {
    fn position_message(&self, bounds: Rectangle, cursor_position: Point) -> Message {
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = bounds.width.min(bounds.height) / 2.0 - 14.0;
        let dx = (cursor_position.x - center.x) / radius;
        let dy = (cursor_position.y - center.y) / radius;
        let screen_r = (dx * dx + dy * dy).sqrt();
        let r = screen_r.min(1.0).powf(1.0 / DISK_WARP);
        let scale = if screen_r > 0.0 { r / screen_r } else { 0.0 };
        // screen y is down; installation y is up
        Message::PilotDragTo {
            col: self.col,
            target: self.target,
            position: vec2(dx * scale, -dy * scale),
        }
    }

    fn warped_point(&self, bounds: Rectangle, position: glam::Vec2) -> Point {
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = bounds.width.min(bounds.height) / 2.0 - 14.0;
        let r = position.length();
        let warped = if r > 0.0 { r.powf(DISK_WARP) } else { 0.0 };
        let scale = if r > 0.0 { warped / r } else { 0.0 };
        Point::new(
            center.x + position.x * scale * radius,
            center.y - position.y * scale * radius,
        )
    }
}

impl canvas::Program<Message> for PilotDisk {
    type State = DiskState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        let canvas::Event::Mouse(mouse_event) = event else {
            return (canvas::event::Status::Ignored, None);
        };
        match mouse_event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                if let Some(position) = cursor.position_in(bounds) {
                    state.dragging = true;
                    return (
                        canvas::event::Status::Captured,
                        Some(self.position_message(bounds, position)),
                    );
                }
            }
            mouse::Event::CursorMoved { .. } if state.dragging => {
                if let Some(position) = cursor.position_in(bounds) {
                    return (
                        canvas::event::Status::Captured,
                        Some(self.position_message(bounds, position)),
                    );
                }
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                state.dragging = false;
            }
            _ => {}
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = bounds.width.min(bounds.height) / 2.0 - 14.0;

        // background disc
        frame.fill(
            &Path::circle(center, radius),
            Color::from_rgba(1.0, 1.0, 1.0, 0.030),
        );

        // rings at r = 0.25, 0.5, 0.75, 1 (warped)
        for ring in [0.25f32, 0.5, 0.75, 1.0] {
            let rr = ring.powf(DISK_WARP) * radius;
            frame.stroke(
                &Path::circle(center, rr),
                Stroke::default().with_width(1.0).with_color(GRID_LINE),
            );
        }
        // crosshair
        frame.stroke(
            &Path::line(
                Point::new(center.x - radius, center.y),
                Point::new(center.x + radius, center.y),
            ),
            Stroke::default().with_width(1.0).with_color(GRID_LINE),
        );
        frame.stroke(
            &Path::line(
                Point::new(center.x, center.y - radius),
                Point::new(center.x, center.y + radius),
            ),
            Stroke::default().with_width(1.0).with_color(GRID_LINE),
        );
        // minor angle ticks every 30 degrees
        for i in 0..12 {
            let angle = i as f32 * std::f32::consts::TAU / 12.0;
            let (sin, cos) = angle.sin_cos();
            frame.stroke(
                &Path::line(
                    Point::new(center.x + cos * (radius - 4.0), center.y + sin * (radius - 4.0)),
                    Point::new(center.x + cos * radius, center.y + sin * radius),
                ),
                Stroke::default().with_width(1.0).with_color(GRID_LINE),
            );
        }
        // axis labels (installation space: x right, y up)
        let axis_label = |content: &str, position: Point| CanvasText {
            content: content.to_string(),
            position,
            color: TEXT_MUTED,
            size: iced::Pixels(10.0),
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..CanvasText::default()
        };
        frame.fill_text(axis_label("+y", Point::new(center.x, center.y - radius - 8.0)));
        frame.fill_text(axis_label("+x", Point::new(center.x + radius + 9.0, center.y)));

        let target_point = self.warped_point(bounds, self.position);

        // dashed leader from live to local target while they disagree
        if let Some(live) = self.live_position {
            let live_point = self.warped_point(bounds, live);
            let dx = live_point.x - target_point.x;
            let dy = live_point.y - target_point.y;
            if (dx * dx + dy * dy).sqrt() > 4.0 {
                frame.stroke(
                    &Path::line(live_point, target_point),
                    Stroke {
                        line_dash: canvas::LineDash {
                            segments: &[3.0, 4.0],
                            offset: 0,
                        },
                        ..Stroke::default()
                            .with_width(1.0)
                            .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.35))
                    },
                );
            }
        }

        // live position (blue, filled)
        if let Some(live) = self.live_position {
            let p = self.warped_point(bounds, live);
            frame.fill(&Path::circle(p, 7.0), LIVE_BLUE);
        }
        // live target (blue ring)
        if let Some(live_target) = self.live_target_position {
            let p = self.warped_point(bounds, live_target);
            frame.stroke(
                &Path::circle(p, 9.0),
                Stroke::default().with_width(2.0).with_color(LIVE_BLUE),
            );
        }
        // local target (white ring + dot)
        frame.stroke(
            &Path::circle(target_point, 11.0),
            Stroke::default().with_width(2.0).with_color(TARGET_WHITE),
        );
        frame.fill(&Path::circle(target_point, 3.0), TARGET_WHITE);

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.dragging {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}

pub fn pilot_disk(
    col: usize,
    target: u8,
    portal: &PortalSnapshot,
    size: f32,
) -> Element<'static, Message> {
    Canvas::new(PilotDisk {
        col,
        target,
        position: portal.position,
        live_position: portal.live_position,
        live_target_position: portal.live_target_position,
    })
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .into()
}

// ----------------------------------------------------------------- axis dial

pub struct AxisDial {
    pub col: usize,
    pub target: u8,
    pub axis: usize,
    pub value: f32,
    pub live: Option<f32>,
}

#[derive(Default)]
pub struct DialState {
    dragging: bool,
}

impl AxisDial {
    fn value_message(&self, bounds: Rectangle, cursor_position: Point) -> Message {
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let dx = cursor_position.x - center.x;
        let dy = cursor_position.y - center.y;
        // axes: 0 = left, values increase clockwise on screen
        let angle = dy.atan2(dx); // radians, 0 = +x (right), y down
        let value = (angle + std::f32::consts::PI) / std::f32::consts::TAU;
        Message::PilotSetAxis {
            col: self.col,
            target: self.target,
            axis: self.axis,
            value: value.rem_euclid(1.0),
        }
    }

    fn dial_point(&self, bounds: Rectangle, value: f32, radius_scale: f32) -> Point {
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = (bounds.width.min(bounds.height) / 2.0 - 12.0) * radius_scale;
        let angle = value * std::f32::consts::TAU - std::f32::consts::PI;
        Point::new(
            center.x + angle.cos() * radius,
            center.y + angle.sin() * radius,
        )
    }
}

impl canvas::Program<Message> for AxisDial {
    type State = DialState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        let canvas::Event::Mouse(mouse_event) = event else {
            return (canvas::event::Status::Ignored, None);
        };
        match mouse_event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                if let Some(position) = cursor.position_in(bounds) {
                    state.dragging = true;
                    return (
                        canvas::event::Status::Captured,
                        Some(self.value_message(bounds, position)),
                    );
                }
            }
            mouse::Event::CursorMoved { .. } if state.dragging => {
                if let Some(position) = cursor.position_in(bounds) {
                    return (
                        canvas::event::Status::Captured,
                        Some(self.value_message(bounds, position)),
                    );
                }
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                state.dragging = false;
            }
            _ => {}
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = bounds.width.min(bounds.height) / 2.0 - 12.0;

        // background disc + outer ring
        frame.fill(
            &Path::circle(center, radius),
            Color::from_rgba(1.0, 1.0, 1.0, 0.030),
        );
        frame.stroke(
            &Path::circle(center, radius),
            Stroke::default().with_width(1.5).with_color(GRID_LINE),
        );

        // accent arc from 0 to the current value
        if self.value > 0.001 {
            let start = iced::Radians(-std::f32::consts::PI);
            let end = iced::Radians(self.value * std::f32::consts::TAU - std::f32::consts::PI);
            let mut builder = canvas::path::Builder::new();
            builder.arc(canvas::path::Arc {
                center,
                radius: radius - 5.0,
                start_angle: start,
                end_angle: end,
            });
            frame.stroke(
                &builder.build(),
                Stroke::default()
                    .with_width(3.0)
                    .with_color(theme::with_alpha(ACCENT, 0.65)),
            );
        }

        // quadrant tick labels (0 = left, 0.25 = up, 0.5 = right, 0.75 = down)
        for (value, label) in [(0.0, "0"), (0.25, ".25"), (0.5, ".5"), (0.75, ".75")] {
            let tick = self.dial_point(bounds, value, 1.0);
            frame.fill(&Path::circle(tick, 2.0), GRID_LINE);
            let out = self.dial_point(bounds, value, 1.22);
            frame.fill_text(CanvasText {
                content: label.to_string(),
                position: out,
                color: TEXT_MUTED,
                size: iced::Pixels(9.0),
                horizontal_alignment: iced::alignment::Horizontal::Center,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..CanvasText::default()
            });
        }

        // live value (blue line)
        if let Some(live) = self.live {
            let p = self.dial_point(bounds, live, 0.88);
            frame.stroke(
                &Path::line(center, p),
                Stroke::default().with_width(3.0).with_color(LIVE_BLUE),
            );
        }
        // target value (white line + knob)
        let p = self.dial_point(bounds, self.value, 1.0);
        frame.stroke(
            &Path::line(center, p),
            Stroke::default().with_width(2.0).with_color(TARGET_WHITE),
        );
        frame.fill(&Path::circle(p, 4.5), TARGET_WHITE);

        // value text
        frame.fill_text(CanvasText {
            content: format!("{:.4}", self.value),
            position: center,
            color: TARGET_WHITE,
            size: iced::Pixels(12.0),
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..CanvasText::default()
        });

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.dragging {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

pub fn axis_dial(
    col: usize,
    target: u8,
    axis: usize,
    portal: &PortalSnapshot,
    size: f32,
) -> Element<'static, Message> {
    Canvas::new(AxisDial {
        col,
        target,
        axis,
        value: if axis == 0 { portal.axes.x } else { portal.axes.y },
        live: portal.live_axes.map(|a| if axis == 0 { a.x } else { a.y }),
    })
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .into()
}

// --------------------------------------------------------------- portal grid

/// Clickable grid of a column's portals with live/target markers and
/// health-state cell tints.
pub struct PortalGrid {
    pub col: usize,
    pub count_x: usize,
    pub count_y: usize,
    pub flipped: bool,
    pub portals: Vec<GridPortal>,
    pub selected: Option<u8>,
}

pub struct GridPortal {
    pub target: u8,
    pub position: glam::Vec2,
    pub live_position: Option<glam::Vec2>,
    pub rx_age_ms: Option<u64>,
    pub tx_age_ms: Option<u64>,
    pub health: Option<PortalState>,
}

impl PortalGrid {
    pub fn from_snapshot(
        column: &ColumnSnapshot,
        selected: Option<u8>,
        health: &HashMap<(u8, u8), PortalState>,
    ) -> Self {
        Self {
            col: column.index,
            count_x: column.count_x.max(1),
            count_y: column.count_y.max(1),
            flipped: column.flipped,
            portals: column
                .portals
                .iter()
                .map(|p| GridPortal {
                    target: p.target,
                    position: p.position,
                    live_position: p.live_position,
                    rx_age_ms: p.last_rx_age_ms,
                    tx_age_ms: p.last_tx_age_ms,
                    health: health.get(&(column.index as u8, p.target)).copied(),
                })
                .collect(),
            selected,
        }
    }

    fn cell(&self, index: usize, bounds: Rectangle) -> Rectangle {
        let cw = bounds.width / self.count_x as f32;
        let ch = bounds.height / self.count_y as f32;
        let gx = index % self.count_x;
        let row = index / self.count_x;
        // when NOT flipped, portal 1 is at the bottom (image bottom-to-top)
        let gy = if self.flipped { row } else { self.count_y - 1 - row };
        Rectangle {
            x: gx as f32 * cw,
            y: gy as f32 * ch,
            width: cw,
            height: ch,
        }
    }
}

/// Cell tint for a portal's health state (None = no data yet).
fn health_fill(state: Option<PortalState>) -> Option<Color> {
    match state {
        Some(PortalState::Ok) => Some(theme::with_alpha(theme::OK, 0.07)),
        Some(PortalState::Degraded) => Some(theme::with_alpha(theme::WARN, 0.16)),
        Some(PortalState::Faulty) => Some(theme::with_alpha(theme::ERROR, 0.20)),
        Some(PortalState::Silent) => Some(theme::with_alpha(theme::ERROR, 0.30)),
        Some(PortalState::Unknown) | None => None,
    }
}

impl canvas::Program<Message> for PortalGrid {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if let Some(position) = cursor.position_in(bounds) {
                let local = Rectangle::new(Point::ORIGIN, bounds.size());
                for (i, portal) in self.portals.iter().enumerate() {
                    let cell = self.cell(i, local);
                    if cell.contains(position) {
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::Select(Selection::Portal {
                                col: self.col,
                                target: portal.target,
                            })),
                        );
                    }
                }
            }
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let local = Rectangle::new(Point::ORIGIN, bounds.size());

        for (i, portal) in self.portals.iter().enumerate() {
            let cell = self.cell(i, local);
            let inner = Rectangle {
                x: cell.x + 1.0,
                y: cell.y + 1.0,
                width: cell.width - 2.0,
                height: cell.height - 2.0,
            };
            let selected = self.selected == Some(portal.target);
            let shape = Path::rounded_rectangle(
                Point::new(inner.x, inner.y),
                Size::new(inner.width, inner.height),
                3.0.into(),
            );

            if selected {
                frame.fill(&shape, theme::with_alpha(ACCENT, 0.16));
            } else if let Some(fill) = health_fill(portal.health) {
                frame.fill(&shape, fill);
            }
            frame.stroke(
                &shape,
                Stroke::default()
                    .with_width(if selected { 1.5 } else { 1.0 })
                    .with_color(if selected { ACCENT } else { GRID_LINE }),
            );

            let center = Point::new(inner.x + inner.width / 2.0, inner.y + inner.height / 2.0);
            let radius = inner.width.min(inner.height) / 2.0 - 3.0;
            let to_point = |p: glam::Vec2| {
                let r = p.length().min(1.0);
                let scale = if p.length() > 0.0 { r / p.length() } else { 0.0 };
                Point::new(
                    center.x + p.x * scale * radius,
                    center.y - p.y * scale * radius,
                )
            };

            // leader line between live and target, then live (blue filled),
            // then target (white ring)
            let target_point = to_point(portal.position);
            if let Some(live) = portal.live_position {
                let live_point = to_point(live);
                let dx = live_point.x - target_point.x;
                let dy = live_point.y - target_point.y;
                if (dx * dx + dy * dy).sqrt() > 3.0 {
                    frame.stroke(
                        &Path::line(live_point, target_point),
                        Stroke::default()
                            .with_width(1.0)
                            .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.25)),
                    );
                }
                frame.fill(&Path::circle(live_point, 2.5), LIVE_BLUE);
            }
            frame.stroke(
                &Path::circle(target_point, 3.5),
                Stroke::default().with_width(1.5).with_color(TARGET_WHITE),
            );

            // target ID
            frame.fill_text(CanvasText {
                content: portal.target.to_string(),
                position: Point::new(inner.x + 3.0, inner.y + 2.0),
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.55),
                size: iced::Pixels(9.0),
                ..CanvasText::default()
            });

            // rx/tx heartbeat ticks (top-right corner), fading with age
            let rx_alpha = heartbeat_alpha(portal.rx_age_ms);
            if rx_alpha > 0.0 {
                frame.fill(
                    &Path::circle(Point::new(inner.x + inner.width - 5.0, inner.y + 5.0), 2.0),
                    theme::with_alpha(theme::OK, rx_alpha),
                );
            }
            let tx_alpha = heartbeat_alpha(portal.tx_age_ms);
            if tx_alpha > 0.0 {
                frame.fill(
                    &Path::circle(Point::new(inner.x + inner.width - 11.0, inner.y + 5.0), 2.0),
                    theme::with_alpha(ACCENT, tx_alpha),
                );
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

// ------------------------------------------------------------ health heatmap

/// Installation-shaped health heatmap for the Diagnostics panel: one slot per
/// column, cells colored by health state, click to inspect the portal.
pub struct HealthHeatmap {
    pub columns: Vec<HeatColumn>,
}

pub struct HeatColumn {
    pub col: usize,
    pub count_x: usize,
    pub count_y: usize,
    pub flipped: bool,
    /// (target, health) in portal order 1..=N.
    pub cells: Vec<(u8, Option<(PortalState, u8)>)>,
}

const HEAT_GAP: f32 = 8.0;

impl HealthHeatmap {
    pub fn from_snapshots(
        columns: &[ColumnSnapshot],
        health: &HashMap<(u8, u8), PortalState>,
        scores: &HashMap<(u8, u8), u8>,
    ) -> Self {
        Self {
            columns: columns
                .iter()
                .map(|c| HeatColumn {
                    col: c.index,
                    count_x: c.count_x.max(1),
                    count_y: c.count_y.max(1),
                    flipped: c.flipped,
                    cells: c
                        .portals
                        .iter()
                        .map(|p| {
                            let key = (c.index as u8, p.target);
                            let state = health.get(&key).copied();
                            let score = scores.get(&key).copied().unwrap_or(0);
                            (p.target, state.map(|s| (s, score)))
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn column_rect(&self, index: usize, bounds: Rectangle) -> Rectangle {
        let count = self.columns.len().max(1) as f32;
        let width = (bounds.width - HEAT_GAP * (count - 1.0)) / count;
        Rectangle {
            x: index as f32 * (width + HEAT_GAP),
            y: 0.0,
            width,
            height: bounds.height,
        }
    }

    fn cell_rect(&self, column: &HeatColumn, cell_index: usize, area: Rectangle) -> Rectangle {
        let cw = area.width / column.count_x as f32;
        let ch = area.height / column.count_y as f32;
        let gx = cell_index % column.count_x;
        let row = cell_index / column.count_x;
        let gy = if column.flipped { row } else { column.count_y - 1 - row };
        Rectangle {
            x: area.x + gx as f32 * cw,
            y: area.y + gy as f32 * ch,
            width: cw,
            height: ch,
        }
    }
}

impl canvas::Program<Message> for HealthHeatmap {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if let Some(position) = cursor.position_in(bounds) {
                let local = Rectangle::new(Point::ORIGIN, bounds.size());
                for (ci, column) in self.columns.iter().enumerate() {
                    let area = self.column_rect(ci, local);
                    for (i, (target, _)) in column.cells.iter().enumerate() {
                        if self.cell_rect(column, i, area).contains(position) {
                            return (
                                canvas::event::Status::Captured,
                                Some(Message::Select(Selection::Portal {
                                    col: column.col,
                                    target: *target,
                                })),
                            );
                        }
                    }
                }
            }
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let local = Rectangle::new(Point::ORIGIN, bounds.size());

        for (ci, column) in self.columns.iter().enumerate() {
            let area = self.column_rect(ci, local);
            for (i, (_, health)) in column.cells.iter().enumerate() {
                let cell = self.cell_rect(column, i, area);
                let inner = Rectangle {
                    x: cell.x + 1.0,
                    y: cell.y + 1.0,
                    width: (cell.width - 2.0).max(1.0),
                    height: (cell.height - 2.0).max(1.0),
                };
                let color = match health {
                    Some((state, score)) => {
                        let base = theme::state_color(*state);
                        // fade Ok cells by score so a 100 reads brighter
                        let alpha = match state {
                            PortalState::Ok => 0.25 + 0.45 * (*score as f32 / 100.0),
                            PortalState::Unknown => 0.25,
                            _ => 0.85,
                        };
                        theme::with_alpha(base, alpha)
                    }
                    None => Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                };
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(inner.x, inner.y),
                        Size::new(inner.width, inner.height),
                        2.0.into(),
                    ),
                    color,
                );
            }
            // column index below is drawn by the surrounding view; here just
            // a faint outline per column
            frame.stroke(
                &Path::rectangle(Point::new(area.x, area.y), Size::new(area.width, area.height)),
                Stroke::default().with_width(0.5).with_color(GRID_LINE),
            );
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

// ------------------------------------------------------------------- helpers

/// Convert the renderer's float-RGB pixels into an image handle (clamped to
/// 0..1 like the GL preview in the C++ app).
pub fn preview_handle(pixels: &router_core::image::PixelsF32) -> Option<iced::widget::image::Handle> {
    if pixels.width == 0 || pixels.height == 0 {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels.width * pixels.height * 4);
    for chunk in pixels.data.chunks_exact(3) {
        for value in chunk {
            rgba.push((value.clamp(0.0, 1.0) * 255.0) as u8);
        }
        rgba.push(255);
    }
    Some(iced::widget::image::Handle::from_rgba(
        pixels.width as u32,
        pixels.height as u32,
        rgba,
    ))
}

/// A small colored status dot with a label.
pub fn status_dot(fresh: Option<bool>, label: &str) -> Element<'static, Message> {
    let color = match fresh {
        Some(true) => theme::OK,
        Some(false) => Color::from_rgba(1.0, 1.0, 1.0, 0.22),
        None => Color::from_rgba(1.0, 1.0, 1.0, 0.10),
    };
    iced::widget::row![
        iced::widget::text("●").color(color).size(11),
        iced::widget::text(label.to_string()).size(11).color(theme::TEXT_DIM),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center)
    .into()
}

pub fn level_color(level: u8) -> Color {
    match level {
        20.. => theme::ERROR,
        10.. => theme::WARN,
        _ => theme::TEXT_DIM,
    }
}
