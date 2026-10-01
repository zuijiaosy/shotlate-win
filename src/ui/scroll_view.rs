//! Shared rendering for the live long-capture panel and its finished-image viewer.
use tiny_skia::{FilterQuality, Pixmap, Transform};
use crate::kit::geom::{Point, Rect};
use crate::kit::color::{Color, SELECTION_BLUE};
use crate::render::{canvas::Canvas, text::Weight};
use super::chrome::Theme;

pub fn panel_rect(region: Rect, monitor: Rect, scale: f32) -> Rect {
    let gap = (8.0 * scale).min(monitor.height / 8.0).min(monitor.width / 8.0);
    let w = (260.0 * scale).min((monitor.width - 2.0 * gap).max(1.0));
    let h = (460.0 * scale).min((monitor.height - 2.0 * gap).max(1.0));
    let x = if region.max_x() + gap + w <= monitor.max_x() { region.max_x() + gap }
        else if region.x - gap - w >= monitor.x { region.x - gap - w }
        else { monitor.max_x() - gap - w };
    let y = if region.y - gap - h >= monitor.y { region.y - gap - h }
        else if region.max_y() + gap + h <= monitor.max_y() { region.max_y() + gap }
        else { region.y.clamp(monitor.y + gap, (monitor.max_y() - h - gap).max(monitor.y + gap)) };
    Rect::new(x, y, w, h)
}

pub fn buttons(width: f32, height: f32, finished: bool) -> Vec<(Rect, &'static str)> {
    let labels: &[&str] = if finished { &["复制", "保存", "贴图", "关闭"] } else { &["自动滚动", "完成", "取消"] };
    let w = (width - 24.0 - (labels.len() - 1) as f32 * 8.0) / labels.len() as f32;
    labels.iter().enumerate().map(|(i, &label)| (Rect::new(12.0 + i as f32 * (w + 8.0), height - 42.0, w, 30.0), label)).collect()
}

pub fn render(width: f32, height: f32, scale: f32, image: Option<&Pixmap>, status: &str, running: bool, finished: bool, offset: f32, dark: bool) -> Option<Pixmap> {
    let theme = Theme { dark };
    let mut pix = Pixmap::new((width * scale).round() as u32, (height * scale).round() as u32)?;
    let mut c = Canvas::new(pix.as_mut(), Transform::from_scale(scale, scale));
    c.fill_rect(&Rect::new(0.0, 0.0, width, height), theme.panel());
    c.text(if finished { "长截图" } else { "滚动截图" }, Point::new(14.0, 12.0), 15.0, Weight::Bold, theme.label());
    let viewport = Rect::new(12.0, 44.0, width - 24.0, height - 144.0);
    c.fill_rect(&viewport, if dark { Color::gray(0.08, 1.0) } else { Color::gray(0.92, 1.0) });
    if let Some(image) = image {
        let w = if finished { viewport.width } else { viewport.width.min(image.width() as f32 / scale) };
        let h = w * image.height() as f32 / image.width() as f32;
        c.save();
        c.clip_rect(&viewport);
        c.draw_image(image.as_ref(), &Rect::new(viewport.mid_x() - w / 2.0, viewport.y - offset, w, h), FilterQuality::Bilinear, 1.0);
        c.restore();
    }
    let status_rect = Rect::new(14.0, height - 90.0, width - 28.0, 42.0);
    let layout = crate::render::text::layout(status, 12.0, Weight::Regular, Some(status_rect.width));
    c.text_layout(&layout, status_rect.origin(), theme.secondary());
    for (i, (r, label)) in buttons(width, height, finished).into_iter().enumerate() {
        c.fill_rounded(&r, 5.0, if i == 0 { SELECTION_BLUE } else { theme.hover() });
        c.text_centered(if !finished && i == 0 && running { "暂停" } else { label }, &r, 12.0, Weight::Regular,
            if i == 0 { Color::white(1.0) } else { theme.label() });
    }
    Some(pix)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn wide_regions_and_high_dpi_keep_panels_on_screen() {
        for (width, height, scale) in [(1024.0, 768.0, 1.0), (1920.0, 1080.0, 2.5), (1024.0, 768.0, 2.0)] {
            let monitor = Rect::new(-width, -height, width, height);
            for region in [monitor, monitor.inset(40.0, 40.0), Rect::new(-width + 20.0, -height + 20.0, 500.0, 600.0)] {
                let panel = panel_rect(region, monitor, scale);
                assert!(panel.x >= monitor.x && panel.y >= monitor.y);
                assert!(panel.max_x() <= monitor.max_x() && panel.max_y() <= monitor.max_y());
                assert!(panel.width > 0.0 && panel.height > 0.0);
            }
        }
    }
    #[test] fn panels_fit_in_both_themes_and_scales() {
        for scale in [1.0, 1.5, 2.0] { for dark in [false, true] { for finished in [false, true] {
            let p = render(260.0, 460.0, scale, None, "60,000 px · 已暂停", false, finished, 0.0, dark).unwrap();
            assert_eq!(p.width(), (260.0 * scale) as u32);
            assert!(buttons(260.0, 460.0, finished).iter().all(|(r, _)| r.max_x() <= 248.0 && r.max_y() <= 448.0));
        } } }
    }
}
