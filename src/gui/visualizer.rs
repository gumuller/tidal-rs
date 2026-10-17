use super::State;
use gtk::{cairo, prelude::*};
use std::f64::consts::TAU;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

pub const MODES: [&str; 6] = [
    "Vines",
    "Neon Tunnel",
    "Warp Drive",
    "Plasma",
    "Orbital",
    "Laser Bloom",
];

fn color(cr: &cairo::Context, base: (f64, f64, f64), shift: f64, alpha: f64) {
    let glow = |offset: f64| 0.5 + 0.5 * (shift + offset).sin();
    cr.set_source_rgba(
        base.0 * 0.55 + glow(0.0) * 0.45,
        base.1 * 0.55 + glow(2.1) * 0.45,
        base.2 * 0.55 + glow(4.2) * 0.45,
        alpha,
    );
}

pub fn draw(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    mode: usize,
    t: f64,
    level: f64,
    base: (f64, f64, f64),
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let cx = w / 2.0;
    let cy = h / 2.0;
    let size = w.min(h);
    let energy = level.clamp(0.0, 1.0);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_cap(cairo::LineCap::Round);
    match mode % MODES.len() {
        0 => {
            cr.set_line_width((size / 150.0).max(1.0));
            for vine in 0..12 {
                color(cr, base, vine as f64 * 0.2 + t * 0.1, 0.75);
                for x in 0..=180 {
                    let xf = x as f64 / 180.0;
                    let y = cy
                        + (xf * 14.0 + t * 2.0 + vine as f64 * 0.28).sin()
                            * (xf * std::f64::consts::PI).sin()
                            * h
                            * (0.06 + energy * 0.32)
                        + (vine as f64 - 5.5) * h * 0.025;
                    if x == 0 {
                        cr.move_to(xf * w, y);
                    } else {
                        cr.line_to(xf * w, y);
                    }
                }
                let _ = cr.stroke();
            }
        }
        1 => {
            for ring in (0..24).rev() {
                let z = (ring as f64 / 24.0 + t * 0.20).fract();
                let radius = z.powf(2.2) * size * 0.95;
                color(cr, base, z * 5.0 + t * 0.3, (1.0 - z) * 0.8 + 0.15);
                cr.set_line_width(1.0 + z * 2.0 + energy * 2.0);
                for side in 0..=8 {
                    let a = side as f64 / 8.0 * TAU + t * 0.22 + z * 0.7;
                    let x = cx + a.cos() * radius * (1.0 + energy * 0.16);
                    let y = cy + a.sin() * radius;
                    if side == 0 {
                        cr.move_to(x, y);
                    } else {
                        cr.line_to(x, y);
                    }
                }
                cr.close_path();
                let _ = cr.stroke();
            }
        }
        2 => {
            for star in 0..220 {
                let seed = star as f64;
                let a = seed * 2.399963;
                let z = (seed * 0.618034 + t * (0.12 + energy * 0.26))
                    .fract()
                    .max(0.005);
                let r = z * z * size * 0.9;
                let old = (r - size * (0.008 + energy * 0.04) * z).max(0.0);
                color(cr, base, seed * 0.08, z.min(0.95));
                cr.set_line_width((z * 2.5).max(0.6));
                cr.move_to(cx + a.cos() * old, cy + a.sin() * old);
                cr.line_to(cx + a.cos() * r, cy + a.sin() * r);
                let _ = cr.stroke();
            }
        }
        3 => {
            for y in 0..28 {
                for x in 0..40 {
                    let xf = x as f64 / 40.0;
                    let yf = y as f64 / 28.0;
                    let d = ((xf - 0.5).powi(2) + (yf - 0.5).powi(2)).sqrt();
                    let v = (xf * 13.0 + t).sin()
                        + (yf * 10.0 - t * 0.8).sin()
                        + (d * 24.0 - t * (1.5 + energy)).sin();
                    color(cr, base, v + t * 0.3, 0.12 + energy * 0.55);
                    cr.rectangle(xf * w, yf * h, w / 40.0 + 0.5, h / 28.0 + 0.5);
                    let _ = cr.fill();
                }
            }
        }
        4 => {
            for orbit in 0..8 {
                let r = size * (0.10 + orbit as f64 * 0.045) * (1.0 + energy * 0.2);
                color(cr, base, orbit as f64 * 0.5 + t * 0.2, 0.8);
                cr.set_line_width(1.2);
                let _ = cr.save();
                cr.translate(cx, cy);
                cr.rotate(t * 0.13 + orbit as f64 * 0.39);
                cr.scale(1.0, 0.35 + energy * 0.25);
                cr.arc(0.0, 0.0, r, 0.0, TAU);
                let _ = cr.stroke();
                let a = t * (0.6 + orbit as f64 * 0.10) + orbit as f64;
                cr.arc(a.cos() * r, a.sin() * r, 3.0 + energy * 4.0, 0.0, TAU);
                let _ = cr.fill();
                let _ = cr.restore();
            }
        }
        _ => {
            for trail in (0..6).rev() {
                color(
                    cr,
                    base,
                    trail as f64 * 0.35 + t * 0.18,
                    0.15 + 0.12 * (6 - trail) as f64,
                );
                cr.set_line_width(1.0 + energy * 1.5);
                for i in 0..=480 {
                    let a = i as f64 / 480.0 * TAU;
                    let phase = t - trail as f64 * 0.05;
                    let radius =
                        size * (0.22 + energy * 0.16) * (1.0 + 0.18 * (a * 5.0 + phase).sin());
                    let x = cx + (a * 3.0 + phase * 0.34).sin() * radius;
                    let y = cy + (a * 2.0 - phase * 0.28).sin() * radius;
                    if i == 0 {
                        cr.move_to(x, y);
                    } else {
                        cr.line_to(x, y);
                    }
                }
                let _ = cr.stroke();
            }
        }
    }
}

pub(super) fn attach(area: &gtk::DrawingArea, state: Rc<RefCell<State>>, mode: Rc<Cell<usize>>) {
    let phase = Cell::new(0.0);
    let last = Cell::new(std::time::Instant::now());
    area.set_draw_func(move |area, cr, w, h| {
        let now = std::time::Instant::now();
        let dt = now.duration_since(last.replace(now)).as_secs_f64().min(0.1);
        let state = state.borrow();
        let playing = state.last_playback.as_ref().filter(|p| !p.paused);
        let level = playing.map(|p| p.level).unwrap_or(0.0);
        if playing.is_some() {
            phase.set(phase.get() + dt);
        }
        let c = area.color();
        draw(
            cr,
            w as f64,
            h as f64,
            mode.get(),
            phase.get(),
            level,
            (c.red() as f64, c.green() as f64, c.blue() as f64),
        );
    });
    let area = area.downgrade();
    gtk::glib::timeout_add_local(Duration::from_millis(33), move || {
        let Some(area) = area.upgrade() else {
            return gtk::glib::ControlFlow::Break;
        };
        if area.is_mapped() {
            area.queue_draw();
        }
        gtk::glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modes_render_distinct_frames_and_react_to_audio() {
        fn render(mode: usize, level: f64) -> Vec<u8> {
            let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 320, 220).unwrap();
            {
                let cr = cairo::Context::new(&surface).unwrap();
                draw(&cr, 320.0, 220.0, mode, 2.3, level, (0.4, 0.5, 0.9));
            }
            surface.flush();
            surface.data().unwrap().to_vec()
        }
        let frames: Vec<_> = (0..MODES.len()).map(|m| render(m, 0.8)).collect();
        for (i, frame) in frames.iter().enumerate() {
            assert!(frame.iter().any(|b| *b != 0), "{} rendered blank", MODES[i]);
            assert_ne!(
                *frame,
                render(i, 0.1),
                "{} did not react to audio",
                MODES[i]
            );
            for other in frames.iter().skip(i + 1) {
                assert_ne!(frame, other);
            }
        }
    }
}
