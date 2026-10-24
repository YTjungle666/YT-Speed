//! 实时曲线：把 `samples_json()` 的点画成折线。
//!
//! 点数由 core 的定容环决定（上限固定），所以这里不需要再截断。

use leptos::prelude::*;

const W: f64 = 600.0;
const H: f64 = 160.0;
const PAD: f64 = 8.0;

#[component]
pub fn Chart(samples: RwSignal<Vec<[f64; 2]>>) -> impl IntoView {
    let max_y = move || samples.get().iter().map(|p| p[1]).fold(100.0f64, f64::max) * 1.15;

    let points = move || {
        let pts = samples.get();
        if pts.is_empty() {
            return String::new();
        }
        let t_max = pts.last().map(|p| p[0]).unwrap_or(1.0).max(1.0);
        let y_max = max_y();
        pts.iter()
            .map(|p| {
                let x = PAD + (p[0] / t_max) * (W - 2.0 * PAD);
                let y = H - PAD - (p[1] / y_max) * (H - 2.0 * PAD);
                format!("{x:.1},{y:.1}")
            })
            .collect::<Vec<_>>()
            .join(" ")
    };

    let count = move || samples.get().len();

    view! {
        <figure class="chart">
            <figcaption>
                "速率曲线"
                <span class="muted">{move || format!("{} 点", count())}</span>
            </figcaption>
            <svg viewBox=format!("0 0 {W} {H}") preserveAspectRatio="none">
                <line x1=PAD y1=H - PAD x2=W - PAD y2=H - PAD class="axis" />
                <polyline points=points class="curve" />
            </svg>
        </figure>
    }
}
