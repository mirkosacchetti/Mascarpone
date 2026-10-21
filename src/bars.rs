use crate::rl;
use crate::ui::{self, Ui, R};

const BARS: usize = 4;
const STEP: f64 = 0.35;
const TICK: f64 = 0.09;

pub fn draw(ui: &mut Ui, r: R, playing: bool) {
    let phase = (ui.seconds() / TICK).floor() * STEP;
    let gap = r.w / 10.0;
    let bar = (r.w - gap * (BARS as f32 - 1.0)) / BARS as f32;
    for i in 0..BARS {
        let level = if playing { 0.25 + 0.75 * (phase + i as f64 * 1.3).sin().abs() as f32 } else { 0.3 };
        let h = (level * r.h).round();
        ui.rect(R::new(r.x + i as f32 * (bar + gap), r.bottom() - h, bar, h), ui::ACCENT);
    }
    if playing && unsafe { rl::IsWindowFocused() } {
        ui.wake(TICK - ui.seconds() % TICK);
    }
}
