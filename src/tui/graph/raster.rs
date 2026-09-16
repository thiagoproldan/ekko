//! Drawing in braille. A terminal cell holds a braille glyph of two columns
//! and four rows of dots, which gives a graph eight points for every cell --
//! enough for lines at any angle and round nodes of more than one size.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// The bit each dot sets in a braille glyph, by row and column.
const BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

pub struct Raster {
    width: usize,
    height: usize,
    dots: Vec<u8>,
    /// Each cell's colour, and the priority it was drawn at: one cell has one
    /// colour, so a node drawn over a link keeps the node's.
    ink: Vec<Option<(Color, u8)>>,
}

impl Raster {
    /// A raster for `width` by `height` cells.
    pub fn new(width: u16, height: u16) -> Raster {
        let cells = width as usize * height as usize;
        Raster { width: width as usize, height: height as usize, dots: vec![0; cells], ink: vec![None; cells] }
    }

    /// The raster's width and height in dots.
    pub fn size(&self) -> (f64, f64) {
        ((self.width * 2) as f64, (self.height * 4) as f64)
    }

    pub fn dot(&mut self, x: i64, y: i64, colour: Color, priority: u8) {
        if x < 0 || y < 0 || x >= (self.width * 2) as i64 || y >= (self.height * 4) as i64 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let cell = (y / 4) * self.width + x / 2;
        self.dots[cell] |= BITS[y % 4][x % 2];
        if self.ink[cell].is_none_or(|(_, drawn)| drawn <= priority) {
            self.ink[cell] = Some((colour, priority));
        }
    }

    /// A straight line between two points in dots. It is clipped to the
    /// raster before it is walked, so a link that runs far outside a zoomed-in
    /// view costs only the part that is on screen.
    pub fn line(&mut self, from: (f64, f64), to: (f64, f64), colour: Color, priority: u8) {
        let (width, height) = self.size();
        let Some((from, to)) = clip(from, to, width, height) else { return };
        let (mut x, mut y) = (from.0.round() as i64, from.1.round() as i64);
        let (end_x, end_y) = (to.0.round() as i64, to.1.round() as i64);
        let dx = (end_x - x).abs();
        let dy = -(end_y - y).abs();
        let step_x = if x < end_x { 1 } else { -1 };
        let step_y = if y < end_y { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            self.dot(x, y, colour, priority);
            if x == end_x && y == end_y {
                break;
            }
            let doubled = 2 * error;
            if doubled >= dy {
                error += dy;
                x += step_x;
            }
            if doubled <= dx {
                error += dx;
                y += step_y;
            }
        }
    }

    /// A filled circle of `radius` dots around a point.
    pub fn disk(&mut self, centre: (f64, f64), radius: f64, colour: Color, priority: u8) {
        let (width, height) = self.size();
        let radius = radius.max(0.5);
        let (cx, cy) = centre;
        if !cx.is_finite() || !cy.is_finite() || cx + radius < 0.0 || cy + radius < 0.0 || cx - radius > width || cy - radius > height {
            return;
        }
        for y in (cy - radius).floor() as i64..=(cy + radius).ceil() as i64 {
            for x in (cx - radius).floor() as i64..=(cx + radius).ceil() as i64 {
                let (dx, dy) = (x as f64 - cx, y as f64 - cy);
                if dx * dx + dy * dy <= radius * radius + 0.25 {
                    self.dot(x, y, colour, priority);
                }
            }
        }
    }

    /// Writes the raster into `area`, leaving cells without a dot as they are.
    pub fn blit(&self, buf: &mut Buffer, area: Rect) {
        for row in 0..self.height.min(area.height as usize) {
            for column in 0..self.width.min(area.width as usize) {
                let at = row * self.width + column;
                if self.dots[at] == 0 {
                    continue;
                }
                let Some(glyph) = char::from_u32(0x2800 + u32::from(self.dots[at])) else { continue };
                if let Some(cell) = buf.cell_mut((area.x + column as u16, area.y + row as u16)) {
                    cell.set_char(glyph);
                    if let Some((colour, _)) = self.ink[at] {
                        cell.set_fg(colour);
                    }
                }
            }
        }
    }
}

/// The part of the segment inside a `width` by `height` rectangle, by
/// Liang-Barsky, or `None` when none of it is.
fn clip(from: (f64, f64), to: (f64, f64), width: f64, height: f64) -> Option<((f64, f64), (f64, f64))> {
    if ![from.0, from.1, to.0, to.1].iter().all(|value| value.is_finite()) {
        return None;
    }
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let (mut enter, mut leave) = (0.0f64, 1.0f64);
    let edges = [(-dx, from.0), (dx, width - 1.0 - from.0), (-dy, from.1), (dy, height - 1.0 - from.1)];
    for (p, q) in edges {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > leave {
                return None;
            }
            enter = enter.max(r);
        } else {
            if r < enter {
                return None;
            }
            leave = leave.min(r);
        }
    }
    Some(((from.0 + enter * dx, from.1 + enter * dy), (from.0 + leave * dx, from.1 + leave * dy)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyphs(raster: &Raster, row: usize) -> String {
        (0..raster.width).map(|column| char::from_u32(0x2800 + u32::from(raster.dots[row * raster.width + column])).unwrap()).collect()
    }

    #[test]
    fn a_dot_sets_its_bit_in_its_cell() {
        let mut raster = Raster::new(2, 1);
        raster.dot(0, 0, Color::White, 0);
        raster.dot(3, 3, Color::White, 0);
        assert_eq!(glyphs(&raster, 0), "\u{2801}\u{2880}");
        raster.dot(-1, 0, Color::White, 0);
        raster.dot(4, 0, Color::White, 0);
        assert_eq!(glyphs(&raster, 0), "\u{2801}\u{2880}", "a dot outside the raster was drawn");
    }

    /// A line reaches both of its ends, and one that runs far past the raster
    /// is cut to it rather than walked dot by dot.
    #[test]
    fn a_line_reaches_its_ends_and_is_clipped_to_the_raster() {
        let mut raster = Raster::new(4, 1);
        raster.line((0.0, 0.0), (7.0, 0.0), Color::White, 0);
        assert_eq!(glyphs(&raster, 0), "\u{2809}\u{2809}\u{2809}\u{2809}");

        let mut far = Raster::new(4, 2);
        let started = std::time::Instant::now();
        far.line((-1e12, 5.0), (1e12, 5.0), Color::White, 0);
        assert!(started.elapsed().as_millis() < 50, "a clipped line was walked");
        assert_eq!(glyphs(&far, 1), "\u{2812}\u{2812}\u{2812}\u{2812}");
        far.line((f64::NAN, 0.0), (3.0, 3.0), Color::White, 0);
    }

    /// A node drawn over a link keeps its own colour in the cell they share.
    #[test]
    fn the_higher_priority_keeps_its_colour() {
        let mut raster = Raster::new(1, 1);
        raster.dot(0, 0, Color::Red, 2);
        raster.dot(1, 1, Color::Blue, 1);
        assert_eq!(raster.ink[0], Some((Color::Red, 2)));
        raster.dot(1, 2, Color::Green, 2);
        assert_eq!(raster.ink[0], Some((Color::Green, 2)));
    }

    #[test]
    fn a_disk_fills_around_its_centre() {
        let mut raster = Raster::new(3, 2);
        raster.disk((3.0, 4.0), 1.5, Color::White, 0);
        let filled: u32 = raster.dots.iter().map(|bits| bits.count_ones()).sum();
        assert!((5..=13).contains(&filled), "{filled} dots");
        let mut buffer = Buffer::empty(Rect::new(0, 0, 3, 2));
        raster.blit(&mut buffer, Rect::new(0, 0, 3, 2));
        assert!((0..3).any(|x| buffer[(x, 0)].symbol() != " "));
    }
}
