use ratatui::style::Color;

pub fn completion_color(progress: f32) -> Color {
    let strength = (1.0 - progress.clamp(0.0, 1.0)).powi(3);
    let channel = |base: f32, peak: f32| (base + (peak - base) * strength).round() as u8;
    Color::Rgb(
        channel(15.0, 27.0),
        channel(19.0, 64.0),
        channel(24.0, 55.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_feedback_settles_without_overshooting() {
        assert_eq!(completion_color(0.0), Color::Rgb(27, 64, 55));
        assert_eq!(completion_color(1.0), crate::view::BG);
        assert_eq!(completion_color(3.0), crate::view::BG);
    }
}
