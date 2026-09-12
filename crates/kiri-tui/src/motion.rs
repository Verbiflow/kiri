use ratatui::style::Color;

pub fn completion_color(progress: f32, theme: &crate::theme::Theme) -> Color {
    let strength = (1.0 - progress.clamp(0.0, 1.0)).powi(3);
    match (theme.bg, theme.pulse) {
        (Color::Rgb(r, g, b), Color::Rgb(pr, pg, pb)) => {
            let channel = |base: u8, peak: u8| {
                (base as f32 + (peak as f32 - base as f32) * strength).round() as u8
            };
            Color::Rgb(channel(r, pr), channel(g, pg), channel(b, pb))
        }
        _ => theme.bg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_feedback_settles_without_overshooting() {
        let theme = &crate::theme::KIRI;
        assert_eq!(completion_color(0.0, theme), Color::Rgb(27, 64, 55));
        assert_eq!(completion_color(1.0, theme), theme.bg);
        assert_eq!(completion_color(3.0, theme), theme.bg);
    }
}
