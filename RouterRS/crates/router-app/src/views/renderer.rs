//! Renderer tab: the composited preview and the image-source stack.

use iced::widget::{
    button, checkbox, column, container, pick_list, row, scrollable, slider, text, text_input,
    Space,
};
use iced::{Alignment, Color, Element, Length};

use crate::message::Message;
use crate::widgets::preview_handle;
use crate::{icons, theme};

use super::{section_title, tool_button, Ctx};

fn source_type_style(type_name: &str) -> (char, Color) {
    match type_name {
        "Gradient" => (icons::BLEND, theme::SRC_GRADIENT),
        "Text" => (icons::TYPE, theme::SRC_TEXT),
        "FilePlayer" => (icons::FILM, theme::SRC_FILE),
        "Spout" => (icons::CAST, theme::SRC_SPOUT),
        _ => (icons::LAYERS, theme::TEXT_DIM),
    }
}

pub fn panel<'a>(ctx: Ctx<'a>) -> Element<'a, Message> {
    let header = row![
        column![
            row![
                icons::icon_sized(icons::IMAGE, 18).color(theme::ACCENT),
                text("Renderer").size(19),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text(format!(
                "{}×{} px — one pixel per portal, sampled into aim vectors",
                ctx.snap.resolution.0, ctx.snap.resolution.1
            ))
            .size(11)
            .color(theme::TEXT_MUTED),
        ]
        .spacing(3),
        Space::with_width(Length::Fill),
        checkbox("Image sampling", ctx.snap.image_enabled)
            .on_toggle(Message::ToggleImageEnabled)
            .size(14)
            .text_size(12),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let preview: Element<Message> = match preview_handle(&ctx.snap.preview) {
        Some(handle) => iced::widget::image(handle)
            .filter_method(iced::widget::image::FilterMethod::Nearest)
            .width(Length::Fill)
            .height(Length::Fixed(280.0))
            .content_fit(iced::ContentFit::Contain)
            .into(),
        None => container(text("No image").color(theme::TEXT_MUTED))
            .width(Length::Fill)
            .height(Length::Fixed(120.0))
            .center_x(Length::Fill)
            .center_y(Length::Fixed(120.0))
            .into(),
    };
    let preview_card = container(preview)
        .padding(10)
        .width(Length::Fill)
        .style(theme::card);

    let mut sources_list = column![].spacing(8);
    if ctx.snap.sources.is_empty() {
        sources_list = sources_list.push(
            text("No sources — add one below.")
                .size(12)
                .color(theme::TEXT_MUTED),
        );
    }
    for (index, source) in ctx.snap.sources.iter().enumerate() {
        sources_list = sources_list.push(source_card(index, source));
    }

    let mut add_row = row![text("Add").size(11).color(theme::TEXT_MUTED)]
        .spacing(6)
        .align_y(Alignment::Center);
    for type_name in router_core::image::sources::SOURCE_TYPES {
        let (glyph, _) = source_type_style(type_name);
        add_row = add_row.push(tool_button(
            glyph,
            type_name,
            Message::SourceAdd(type_name.to_string()),
        ));
    }

    scrollable(
        column![
            header,
            preview_card,
            row![
                section_title(icons::LAYERS, "SOURCES"),
                Space::with_width(Length::Fill),
                add_row,
            ]
            .align_y(Alignment::Center),
            sources_list,
        ]
        .spacing(12),
    )
    .into()
}

fn source_card<'a>(index: usize, source: &'a serde_json::Value) -> Element<'a, Message> {
    let get_str = |key: &str| source.get(key).and_then(|v| v.as_str()).unwrap_or("");
    let get_bool = |key: &str| source.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
    let get_f32 = |key: &str| source.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let type_name = get_str("type").to_string();
    let (glyph, accent) = source_type_style(&type_name);

    let header = row![
        icons::icon_sized(glyph, 14).color(accent),
        text(type_name.clone()).size(14),
        Space::with_width(Length::Fixed(10.0)),
        checkbox("Visible", get_bool("visible"))
            .on_toggle(move |v| Message::SourceParam(index, "visible", serde_json::json!(v)))
            .size(14)
            .text_size(12),
        checkbox("Render", get_bool("renderEnabled"))
            .on_toggle(move |v| Message::SourceParam(index, "renderEnabled", serde_json::json!(v)))
            .size(14)
            .text_size(12),
        pick_list(
            ["Direct", "HV_ThetaR", "Centered"].map(String::from).to_vec(),
            Some(get_str("style").to_string()),
            move |style| Message::SourceParam(index, "style", serde_json::json!(style))
        )
        .text_size(12)
        .padding([4, 8]),
        Space::with_width(Length::Fill),
        button(icons::icon_sized(icons::TRASH, 12))
            .padding([5, 8])
            .style(theme::danger)
            .on_press(Message::SourceRemove(index)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let mut controls = column![
        header,
        row![
            text(format!("Alpha {:.2}", get_f32("alpha")))
                .size(12)
                .color(theme::TEXT_DIM)
                .width(Length::Fixed(80.0)),
            slider(0.0..=1.0, get_f32("alpha"), move |alpha| Message::SourceParam(
                index,
                "alpha",
                serde_json::json!(alpha)
            ))
            .step(0.01),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(8);

    controls = match type_name.as_str() {
        "Gradient" => controls.push(
            row![
                pick_list(
                    ["Radial", "Horizontal", "Vertical"].map(String::from).to_vec(),
                    Some(get_str("gradientType").to_string()),
                    move |v| Message::SourceParam(index, "gradientType", serde_json::json!(v))
                )
                .text_size(12)
                .padding([4, 8]),
                pick_list(
                    ["Triangle", "Sine", "Sawtooth"].map(String::from).to_vec(),
                    Some(get_str("wave").to_string()),
                    move |v| Message::SourceParam(index, "wave", serde_json::json!(v))
                )
                .text_size(12)
                .padding([4, 8]),
                text(format!("freq {:.2}", get_f32("frequency"))).size(12).color(theme::TEXT_DIM),
                slider(0.0..=8.0, get_f32("frequency"), move |v| Message::SourceParam(
                    index,
                    "frequency",
                    serde_json::json!(v)
                ))
                .step(0.05)
                .width(Length::Fixed(120.0)),
                text(format!("speed {:.2}", get_f32("speed"))).size(12).color(theme::TEXT_DIM),
                slider(-2.0..=2.0, get_f32("speed"), move |v| Message::SourceParam(
                    index,
                    "speed",
                    serde_json::json!(v)
                ))
                .step(0.01)
                .width(Length::Fixed(120.0)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ),
        "FilePlayer" => {
            let file = get_str("file").to_string();
            let error = get_str("error").to_string();
            controls
                .push(
                    row![
                        super::tool_button(
                            icons::UPLOAD,
                            "Pick file...",
                            Message::SourceFileDialog(index)
                        ),
                        text(if file.is_empty() { "(no file)".to_string() } else { file })
                            .size(11)
                            .color(theme::TEXT_DIM),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .push(
                    row![
                        checkbox("Play", get_bool("play"))
                            .on_toggle(move |v| Message::SourceParam(index, "play", serde_json::json!(v)))
                            .size(14)
                            .text_size(12),
                        pick_list(
                            ["Loop", "Ping Pong", "None"].map(String::from).to_vec(),
                            Some(get_str("loopMode").to_string()),
                            move |v| Message::SourceParam(index, "loopMode", serde_json::json!(v))
                        )
                        .text_size(12)
                        .padding([4, 8]),
                        text(format!("speed {:.2}", get_f32("speed"))).size(12).color(theme::TEXT_DIM),
                        slider(-4.0..=4.0, get_f32("speed"), move |v| Message::SourceParam(
                            index,
                            "speed",
                            serde_json::json!(v)
                        ))
                        .step(0.05)
                        .width(Length::Fixed(120.0)),
                        text(format!("pos {:.2}", get_f32("position"))).size(12).color(theme::TEXT_DIM),
                        slider(0.0..=1.0, get_f32("position"), move |v| Message::SourceParam(
                            index,
                            "position",
                            serde_json::json!(v)
                        ))
                        .step(0.001)
                        .width(Length::Fixed(160.0)),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .push(if error.is_empty() {
                    Element::from(Space::with_height(0))
                } else {
                    row![
                        icons::icon_sized(icons::ALERT, 11).color(theme::ERROR),
                        text(error).size(11).color(theme::ERROR),
                    ]
                    .spacing(5)
                    .align_y(Alignment::Center)
                    .into()
                })
        }
        "Text" => controls.push(
            row![
                text_input("text...", get_str("text"))
                    .on_input(move |v| Message::SourceParam(index, "text", serde_json::json!(v)))
                    .size(12)
                    .width(Length::Fixed(200.0)),
                checkbox("Inverse", get_bool("inverse"))
                    .on_toggle(move |v| Message::SourceParam(index, "inverse", serde_json::json!(v)))
                    .size(14)
                    .text_size(12),
                text(format!(
                    "border {}",
                    source.get("border").and_then(|v| v.as_i64()).unwrap_or(0)
                ))
                .size(12)
                .color(theme::TEXT_DIM),
                slider(
                    0.0..=8.0,
                    source.get("border").and_then(|v| v.as_i64()).unwrap_or(0) as f32,
                    move |v| Message::SourceParam(index, "border", serde_json::json!(v as i64))
                )
                .step(1.0)
                .width(Length::Fixed(100.0)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ),
        "Spout" => controls.push(
            row![
                text_input("sender name (empty = active)", get_str("senderName"))
                    .on_input(move |v| Message::SourceParam(index, "senderName", serde_json::json!(v)))
                    .size(12)
                    .width(Length::Fixed(220.0)),
                text(get_str("status").to_string()).size(11).color(theme::WARN),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ),
        _ => controls,
    };

    // colored accent strip on the left edge, matching the source type
    container(
        container(controls)
            .padding(10)
            .width(Length::Fill)
            .style(theme::card),
    )
    .padding(iced::Padding {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 3.0,
    })
    .width(Length::Fill)
    .style(theme::bar_fill(accent))
    .into()
}
