use std::time::{Duration, Instant};

use atoll_engine::MouseButton;

const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DISTANCE: f32 = 4.0;

/// Derives click counts for double and triple clicks, which Chromium expects from the embedder.
#[derive(Default)]
pub struct ClickTracker {
    last: Option<(MouseButton, Instant, f32, f32)>,
    count: i32,
}

impl ClickTracker {
    pub fn press(&mut self, button: MouseButton, x: f32, y: f32) -> i32 {
        let now = Instant::now();
        let continues = self
            .last
            .is_some_and(|(last_button, time, last_x, last_y)| {
                last_button == button
                    && now.duration_since(time) <= DOUBLE_CLICK_TIME
                    && (x - last_x).abs() <= DOUBLE_CLICK_DISTANCE
                    && (y - last_y).abs() <= DOUBLE_CLICK_DISTANCE
            });
        self.count = if continues { (self.count % 3) + 1 } else { 1 };
        self.last = Some((button, now, x, y));
        self.count
    }

    pub fn current(&self) -> i32 {
        self.count.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_clicks_on_the_same_spot_count_up() {
        let mut tracker = ClickTracker::default();
        assert_eq!(tracker.press(MouseButton::Left, 10.0, 10.0), 1);
        assert_eq!(tracker.press(MouseButton::Left, 11.0, 10.0), 2);
        assert_eq!(tracker.press(MouseButton::Left, 11.0, 11.0), 3);
        assert_eq!(tracker.press(MouseButton::Left, 11.0, 11.0), 1);
    }

    #[test]
    fn distant_or_different_clicks_restart() {
        let mut tracker = ClickTracker::default();
        tracker.press(MouseButton::Left, 10.0, 10.0);
        assert_eq!(tracker.press(MouseButton::Left, 40.0, 10.0), 1);
        assert_eq!(tracker.press(MouseButton::Right, 40.0, 10.0), 1);
    }
}
