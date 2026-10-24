//! 仪表盘：SVG 圆弧 + 指针 + 大字读数。
//!
//! 刻度按 `peak` 线性映射（默认 10 Gbit/s）。仪表盘只画**原始口径**；
//! ×1.06 的上游口径在结果面板里单独列，免得一个数两种含义。

use leptos::prelude::*;

use crate::components::format::{phase_color_class, phase_label};

const R: f64 = 96.0;
const CX: f64 = 120.0;
const CY: f64 = 116.0;
/// 圆弧张角：-120° ~ +120°
const SPAN: f64 = 240.0;

fn polar(deg: f64, r: f64) -> (f64, f64) {
    let rad = deg.to_radians();
    (CX + r * rad.sin(), CY - r * rad.cos())
}

fn arc_path(from: f64, to: f64) -> String {
    let (x1, y1) = polar(from, R);
    let (x2, y2) = polar(to, R);
    let large = if (to - from).abs() > 180.0 { 1 } else { 0 };
    let sweep = if to >= from { 1 } else { 0 };
    format!("M {x1:.2} {y1:.2} A {R} {R} 0 {large} {sweep} {x2:.2} {y2:.2}")
}

#[component]
pub fn Gauge(value: RwSignal<f64>, phase: RwSignal<String>, peak: f64) -> impl IntoView {
    let frac = move || (value.get() / peak).clamp(0.0, 1.0);
    let needle = move || {
        let deg = -SPAN / 2.0 + frac() * SPAN;
        let (x, y) = polar(deg, R - 16.0);
        (x, y)
    };
    let track = arc_path(-SPAN / 2.0, SPAN / 2.0);
    let class = move || format!("gauge {}", phase_color_class(&phase.get()));
    let ticks = move || {
        (0..=10)
            .map(|i| {
                let f = i as f64 / 10.0;
                let deg = -SPAN / 2.0 + f * SPAN;
                let (x1, y1) = polar(deg, R - 4.0);
                let (x2, y2) = polar(deg, R + 6.0);
                let (tx, ty) = polar(deg, R + 20.0);
                let label = if i % 5 == 0 {
                    format!("{}", (peak * f / 1000.0).round() as i64)
                } else {
                    String::new()
                };
                view! {
                    <line x1=x1 y1=y1 x2=x2 y2=y2 class="tick" />
                    <text x=tx y=ty class="tick-label" text-anchor="middle">{label}</text>
                }
            })
            .collect_view()
    };

    view! {
        <div class=class>
            <svg viewBox="0 0 240 200" aria-hidden="true">
                <path d=track class="track" />
                <path d=move || arc_path(-SPAN / 2.0, -SPAN / 2.0 + frac() * SPAN) class="fill" />
                {ticks}
                <line
                    x1=CX y1=CY
                    x2=move || needle().0
                    y2=move || needle().1
                    class="needle"
                />
                <circle cx=CX cy=CY r="5" class="hub" />
            </svg>
            <div class="gauge-read" aria-live="polite" aria-atomic="true">
                <span class="num">{move || format!("{:.2}", value.get())}</span>
                <span class="unit">"Mbit/s"</span>
                <span class="phase">{move || phase_label(&phase.get())}</span>
            </div>
        </div>
    }
}
