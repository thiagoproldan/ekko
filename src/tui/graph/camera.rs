//! Where a graph is looked at from: the point of the layout at the middle of
//! the view, and how many braille dots one unit of the layout spans. A dot is
//! about as wide as it is tall, so one scale serves both directions.

pub const MIN_SCALE: f64 = 0.002;
pub const MAX_SCALE: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    /// Framing the whole graph as it moves, until the view is moved or zoomed
    /// by hand.
    pub fitting: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { x: 0.0, y: 0.0, scale: 0.1, fitting: true }
    }
}

impl Camera {
    /// A point of the layout, in dots of a view `size` dots across.
    pub fn to_view(self, point: (f64, f64), size: (f64, f64)) -> (f64, f64) {
        ((point.0 - self.x) * self.scale + size.0 / 2.0, (point.1 - self.y) * self.scale + size.1 / 2.0)
    }

    /// A point of the view, in the layout's units.
    pub fn to_layout(self, point: (f64, f64), size: (f64, f64)) -> (f64, f64) {
        ((point.0 - size.0 / 2.0) / self.scale + self.x, (point.1 - size.1 / 2.0) / self.scale + self.y)
    }

    /// Zooms by `factor`, keeping what is under `point` under it.
    pub fn zoom_at(&mut self, factor: f64, point: (f64, f64), size: (f64, f64)) {
        let before = self.to_layout(point, size);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let after = self.to_layout(point, size);
        self.x += before.0 - after.0;
        self.y += before.1 - after.1;
        self.fitting = false;
    }

    /// Moves the view by `dots`.
    pub fn pan(&mut self, dots: (f64, f64)) {
        self.x -= dots.0 / self.scale;
        self.y -= dots.1 / self.scale;
        self.fitting = false;
    }

    /// Frames every point in a view `size` dots across, with a margin.
    pub fn fit(&mut self, points: impl Iterator<Item = (f64, f64)>, size: (f64, f64)) {
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        for (x, y) in points.filter(|(x, y)| x.is_finite() && y.is_finite()) {
            bounds = Some(match bounds {
                None => (x, y, x, y),
                Some((left, top, right, bottom)) => (left.min(x), top.min(y), right.max(x), bottom.max(y)),
            });
        }
        let Some((left, top, right, bottom)) = bounds else {
            self.x = 0.0;
            self.y = 0.0;
            return;
        };
        self.x = (left + right) / 2.0;
        self.y = (top + bottom) / 2.0;
        let across = (right - left).max(1.0);
        let down = (bottom - top).max(1.0);
        self.scale = (size.0 * 0.8 / across).min(size.1 * 0.8 / down).clamp(MIN_SCALE, MAX_SCALE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    /// Zooming keeps the point under the pointer under it, as a map does.
    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let size = (200.0, 120.0);
        let mut camera = Camera { x: 30.0, y: -10.0, scale: 0.25, fitting: true };
        let pointer = (150.0, 20.0);
        let under = camera.to_layout(pointer, size);
        camera.zoom_at(2.0, pointer, size);
        assert!(near(camera.to_view(under, size), pointer));
        assert!(!camera.fitting);
        camera.zoom_at(1e9, pointer, size);
        assert_eq!(camera.scale, MAX_SCALE);
    }

    #[test]
    fn fitting_frames_every_point_and_panning_moves_by_dots() {
        let size = (100.0, 100.0);
        let points = [(-500.0, 0.0), (500.0, 200.0), (0.0, -300.0)];
        let mut camera = Camera::default();
        camera.fit(points.iter().copied(), size);
        for point in points {
            let (x, y) = camera.to_view(point, size);
            assert!((0.0..=100.0).contains(&x) && (0.0..=100.0).contains(&y), "{point:?} is off the view at {x},{y}");
        }
        let before = camera.to_view((0.0, 0.0), size);
        camera.pan((10.0, -4.0));
        let after = camera.to_view((0.0, 0.0), size);
        assert!(near((after.0 - before.0, after.1 - before.1), (10.0, -4.0)), "the graph did not move with the view");
    }
}
