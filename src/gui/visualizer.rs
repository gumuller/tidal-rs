//! FFT-driven views drawn on GTK's frame clock with a 60 fps target.
use super::{State, spectrum};
use gtk::{cairo, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    time::Instant,
};
pub const MODES: [&str; 4] = [
    "Spectrum",
    "Radial spectrum",
    "Spectral ribbons",
    "Waterfall",
];
pub(super) fn attach(area: &gtk::DrawingArea, state: Rc<RefCell<State>>) {
    area.set_accessible_role(gtk::AccessibleRole::Img);
    area.update_property(&[gtk::accessible::Property::Label("Audio spectrum analyzer")]);
    let source = spectrum::shared();
    let displayed = RefCell::new([0f32; spectrum::BANDS]);
    let peaks = RefCell::new([0f32; spectrum::BANDS]);
    let history = RefCell::new(VecDeque::<[f32; spectrum::BANDS]>::new());
    let sequence = Cell::new(0);
    let last = Cell::new(Instant::now());
    let frames = Cell::new(0u64);
    let announced = Cell::new(Instant::now());
    area.set_draw_func(move |area, cr, w, h| {
        let now = Instant::now();
        let dt = now.duration_since(last.replace(now)).as_secs_f32().min(0.1);
        let source = source.lock().unwrap().clone();
        let playing = state
            .borrow()
            .last_playback
            .as_ref()
            .is_some_and(|p| !p.paused);
        let active = playing
            && source
                .at
                .is_some_and(|at| at.elapsed().as_secs_f32() < 0.25);
        let c = area.color();
        let base = (c.red() as f64, c.green() as f64, c.blue() as f64);
        let mut bars = displayed.borrow_mut();
        let mut held = peaks.borrow_mut();
        if active && sequence.get() != source.sequence {
            sequence.set(source.sequence);
            let mut history = history.borrow_mut();
            history.push_front(source.bins);
            history.truncate(120);
        }
        let mode = state.borrow().visual_mode.get();
        render(
            mode,
            &history.borrow(),
            cr,
            w as f64,
            h as f64,
            &source.bins,
            &mut bars,
            &mut held,
            active,
            dt,
            base,
        );
        if active {
            frames.set(frames.get() + 1);
        }
        if announced.get().elapsed().as_secs_f32() > 0.5 {
            let elapsed = announced.replace(now).elapsed().as_secs_f64();
            let fps = frames.replace(0) as f64 / elapsed.max(0.001);
            area.update_property(&[gtk::accessible::Property::Description(&format!(
                "{} FFT frames; {:.1} draw fps; {}; {}",
                source.sequence,
                fps,
                source.status,
                MODES[(mode as usize).min(3)]
            ))]);
        }
    });
    let last = Cell::new(0i64);
    area.add_tick_callback(move |area, clock| {
        let now = clock.frame_time();
        if area.is_mapped() && now >= last.get() - 1_000 {
            let next = if now - last.get() > 50_000 {
                now + 16_667
            } else {
                last.get() + 16_667
            };
            last.set(next);
            area.queue_draw();
        }
        gtk::glib::ControlFlow::Continue
    });
}
fn render(
    mode: u32,
    history: &VecDeque<[f32; spectrum::BANDS]>,
    cr: &cairo::Context,
    w: f64,
    h: f64,
    input: &[f32; spectrum::BANDS],
    bars: &mut [f32; spectrum::BANDS],
    peaks: &mut [f32; spectrum::BANDS],
    active: bool,
    dt: f32,
    base: (f64, f64, f64),
) {
    if mode == 0 {
        draw(cr, w, h, input, bars, peaks, active, dt, base);
        return;
    }
    for i in 0..spectrum::BANDS {
        let target = if active { input[i] } else { 0.0 };
        let rate = if target > bars[i] { 28.0 } else { 8.0 };
        bars[i] += (target - bars[i]) * (1.0 - (-rate * dt).exp());
        peaks[i] = bars[i].max(peaks[i] - dt * 0.28);
    }
    match mode {
        1 => {
            let cx = w * 0.5;
            let cy = h * 0.5;
            let size = w.min(h);
            let radius = size * 0.20;
            cr.set_line_width(1.0);
            cr.set_source_rgba(base.0, base.1, base.2, 0.2);
            cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
            let _ = cr.stroke();
            for (i, value) in bars.iter().enumerate() {
                let a = i as f64 / spectrum::BANDS as f64 * std::f64::consts::TAU
                    - std::f64::consts::FRAC_PI_2;
                let r = radius + *value as f64 * size * 0.28;
                let peak = radius + peaks[i] as f64 * size * 0.28;
                cr.set_source_rgba(base.0, base.1, base.2, 0.85);
                cr.set_line_width((size * 0.012).max(1.0));
                cr.move_to(cx + a.cos() * radius, cy + a.sin() * radius);
                cr.line_to(cx + a.cos() * r, cy + a.sin() * r);
                let _ = cr.stroke();
                cr.set_source_rgba(
                    1.0 - base.0 * 0.3,
                    1.0 - base.1 * 0.3,
                    1.0 - base.2 * 0.3,
                    0.7,
                );
                cr.arc(
                    cx + a.cos() * peak,
                    cy + a.sin() * peak,
                    1.0 + size * 0.002,
                    0.0,
                    std::f64::consts::TAU,
                );
                let _ = cr.fill();
            }
        }
        2 => {
            for layer in (0..4).rev() {
                let scale = 1.0 - layer as f64 * 0.12;
                let gradient = cairo::LinearGradient::new(0.0, 0.0, w, h);
                gradient.add_color_stop_rgba(
                    0.0,
                    base.0,
                    base.1,
                    base.2,
                    0.05 + layer as f64 * 0.035,
                );
                gradient.add_color_stop_rgba(1.0, base.2, base.0, base.1, 0.22);
                for side in [-1.0, 1.0] {
                    cr.move_to(0.0, h * 0.5);
                    for (i, value) in bars.iter().enumerate() {
                        let x = i as f64 / (spectrum::BANDS - 1) as f64 * w;
                        let y = h * 0.5
                            + side * (*value as f64 * h * 0.46 * scale + layer as f64 * 2.0);
                        cr.line_to(x, y);
                    }
                    cr.line_to(w, h * 0.5);
                    cr.close_path();
                    let _ = cr.set_source(&gradient);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgba(base.0, base.1, base.2, 0.35 + scale * 0.4);
                    cr.set_line_width(1.0);
                    let _ = cr.stroke();
                }
            }
        }
        _ => {
            let mut surface = match cairo::ImageSurface::create(
                cairo::Format::ARgb32,
                spectrum::BANDS as i32,
                120,
            ) {
                Ok(s) => s,
                Err(_) => return,
            };
            let stride = surface.stride() as usize;
            if let Ok(mut data) = surface.data() {
                for (y, row) in history.iter().enumerate() {
                    for (x, value) in row.iter().enumerate() {
                        let energy = ((*value as f64) * 1.7).min(1.0).powf(0.7);
                        let r = (base.0 * energy + energy.powi(4) * 0.35).min(1.0);
                        let g = (base.1 * energy + energy.powi(3) * 0.25).min(1.0);
                        let b = (base.2 * energy + energy.powi(2) * 0.25).min(1.0);
                        let pixel = 0xff000000u32
                            | ((r * 255.0) as u32) << 16
                            | ((g * 255.0) as u32) << 8
                            | (b * 255.0) as u32;
                        let at = y * stride + x * 4;
                        data[at..at + 4].copy_from_slice(&pixel.to_ne_bytes());
                    }
                }
            }
            surface.mark_dirty();
            let _ = cr.save();
            cr.scale(w / spectrum::BANDS as f64, h / 120.0);
            if cr.set_source_surface(&surface, 0.0, 0.0).is_ok() {
                cr.source().set_filter(cairo::Filter::Bilinear);
                let _ = cr.paint();
            }
            let _ = cr.restore();
        }
    }
}
fn draw(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    input: &[f32; spectrum::BANDS],
    bars: &mut [f32; spectrum::BANDS],
    peaks: &mut [f32; spectrum::BANDS],
    active: bool,
    dt: f32,
    base: (f64, f64, f64),
) {
    let top = 8.0;
    let bottom = 20.0;
    let height = (h - top - bottom).max(1.0);
    let width = w / spectrum::BANDS as f64;
    cr.set_line_width(1.0);
    cr.set_source_rgba(base.0, base.1, base.2, 0.12);
    for step in 0..=3 {
        let y = top + height * step as f64 / 3.0;
        cr.move_to(0.0, y);
        cr.line_to(w, y);
        let _ = cr.stroke();
    }
    for i in 0..spectrum::BANDS {
        let target = if active { input[i] } else { 0.0 };
        let rate = if target > bars[i] { 28.0 } else { 8.0 };
        bars[i] += (target - bars[i]) * (1.0 - (-rate * dt).exp());
        peaks[i] = bars[i].max(peaks[i] - dt * 0.28);
        let x = i as f64 * width;
        let y = top + height * (1.0 - bars[i] as f64);
        cr.set_source_rgba(base.0, base.1, base.2, 0.85);
        cr.rectangle(x, y, (width - 2.0).max(1.0), height * bars[i] as f64);
        let _ = cr.fill();
        cr.set_source_rgba(base.0, base.1, base.2, 1.0);
        cr.rectangle(
            x,
            top + height * (1.0 - peaks[i] as f64),
            (width - 2.0).max(1.0),
            1.5,
        );
        let _ = cr.fill();
    }
    cr.set_source_rgba(base.0, base.1, base.2, 0.65);
    cr.select_font_face(
        "JetBrains Mono",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    cr.set_font_size(9.0);
    for (fraction, label) in [
        (0.0, "24 Hz"),
        (0.29, "160"),
        (0.55, "1k"),
        (0.80, "5k"),
        (0.93, "18k"),
    ] {
        cr.move_to(w * fraction, h - 4.0);
        let _ = cr.show_text(label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fft_modes_are_distinct_and_frequency_sensitive() {
        fn frame(mode: u32, band: usize) -> Vec<u8> {
            let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 400, 240).unwrap();
            {
                let cr = cairo::Context::new(&surface).unwrap();
                let mut input = [0.0; spectrum::BANDS];
                input[band] = 0.9;
                let mut history = VecDeque::new();
                for _ in 0..120 {
                    history.push_back(input);
                }
                render(
                    mode,
                    &history,
                    &cr,
                    400.0,
                    240.0,
                    &input,
                    &mut [0.0; spectrum::BANDS],
                    &mut [0.0; spectrum::BANDS],
                    true,
                    1.0,
                    (0.4, 0.5, 0.8),
                );
            }
            surface.flush();
            let bytes = surface.data().unwrap().to_vec();
            bytes
        }
        let frames: Vec<_> = (0..4).map(|m| frame(m, 12)).collect();
        for mode in 0..4 {
            assert_ne!(frames[mode], frame(mode as u32, 50));
            for other in mode + 1..4 {
                assert_ne!(frames[mode], frames[other]);
            }
        }
    }
}
