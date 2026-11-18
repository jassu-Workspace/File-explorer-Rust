//! Motion curves for transfer animations (pure, so they can be tested without a window).

/// Point on the curved flight from `from` to `to` at `t` in 0..=1, with the item's scale (it shrinks as it lands).
pub fn flight_point(from: (f32, f32), to: (f32, f32), t: f32) -> (f32, f32, f32) {
    let t = t.clamp(0.0, 1.0);
    let dist = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
    // A low hop: items near the top edge must not fly up into the toolbar.
    let lift = (dist * 0.25).clamp(60.0, 140.0);
    let c = ((from.0 + to.0) / 2.0, from.1.min(to.1) - lift);
    let u = 1.0 - t;
    let x = u * u * from.0 + 2.0 * u * t * c.0 + t * t * to.0;
    let y = u * u * from.1 + 2.0 * u * t * c.1 + t * t * to.1;
    (x, y, 1.0 - 0.6 * t)
}

/// Spring settle from 0 to 1 with a small overshoot (pop-ins and slide-ups).
pub fn spring(t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    1.0 - (-5.0 * t).exp() * (9.0 * t).cos()
}

/// Horizontal offset (DIPs) of a damped error shake at `t` in 0..=1.
pub fn shake(t: f32) -> f32 {
    if !(0.0..1.0).contains(&t) {
        return 0.0;
    }
    7.0 * (t * 4.0 * std::f32::consts::TAU).sin() * (1.0 - t)
}

/// Laps per second of the drive-to-drive particles for a throughput in bytes/s (log scale, capped).
pub fn particle_speed(bytes_per_sec: f64) -> f32 {
    ((0.15 + (1.0 + bytes_per_sec / 1e6).ln() / 3.0) as f32).min(3.0)
}

/// Particle position along the strip (0..1) for a seed, time in seconds and throughput.
pub fn particle_x(seed: f32, t: f32, bytes_per_sec: f64) -> f32 {
    (seed + t * particle_speed(bytes_per_sec)).fract()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flight_arcs_above_and_lands() {
        let (from, to) = ((100.0, 300.0), (500.0, 280.0));
        let (x0, y0, s0) = flight_point(from, to, 0.0);
        let (x1, y1, s1) = flight_point(from, to, 1.0);
        assert_eq!((x0, y0), from);
        assert_eq!((x1, y1), to);
        assert!(s0 > s1, "items shrink as they arrive");
        let (_, ymid, _) = flight_point(from, to, 0.5);
        assert!(ymid < 280.0 - 40.0, "the path arcs upward, {ymid}");
    }

    #[test]
    fn spring_overshoots_then_settles() {
        assert_eq!(spring(0.0), 0.0);
        assert!((spring(1.0) - 1.0).abs() < 0.02);
        assert!((0..100).map(|i| spring(i as f32 / 100.0)).any(|v| v > 1.03), "a little overshoot");
        assert_eq!(spring(2.0), 1.0);
    }

    #[test]
    fn shake_is_damped_and_ends_at_zero() {
        assert_eq!(shake(0.0), 0.0);
        assert_eq!(shake(1.0), 0.0);
        let peak = (1..100).map(|i| shake(i as f32 / 100.0).abs()).fold(0.0, f32::max);
        assert!((5.0..=8.0).contains(&peak), "{peak}");
    }

    #[test]
    fn particles_move_faster_with_throughput() {
        let slow = particle_x(0.3, 1.0, 10e6);
        let fast = particle_x(0.3, 1.0, 400e6);
        assert!((0.0..1.0).contains(&slow) && (0.0..1.0).contains(&fast));
        assert!(particle_speed(400e6) > particle_speed(10e6));
        assert!(particle_speed(1e12) <= 3.0, "capped so it stays readable");
    }
}
