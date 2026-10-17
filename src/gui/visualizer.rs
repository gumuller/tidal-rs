//! One real spectrum, drawn on GTK's frame clock with a 60 fps target.
use super::{State, spectrum};
use gtk::{cairo, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Instant,
};
pub(super) fn attach(area: &gtk::DrawingArea, state: Rc<RefCell<State>>) {
    area.set_accessible_role(gtk::AccessibleRole::Img);
    area.update_property(&[gtk::accessible::Property::Label("Audio spectrum analyzer")]);
    let source = spectrum::shared();
    let displayed = RefCell::new([0f32; spectrum::BANDS]);
    let peaks = RefCell::new([0f32; spectrum::BANDS]);
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
        draw(
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
                "{} FFT frames; {:.1} draw fps; {}",
                source.sequence, fps, source.status
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
