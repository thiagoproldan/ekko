//! A selection in a list, and the part of the list on screen.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub selected: usize,
    /// The first line on screen.
    pub offset: usize,
}

impl Cursor {
    /// Pulls the viewport to the selection by the least scrolling that does it.
    pub fn follow(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + height {
            self.offset = self.selected + 1 - height;
        }
    }

    /// Moves by `by` in a list of `len`, stopping at either end.
    pub fn step(&mut self, by: isize, len: usize) {
        if len > 0 {
            self.selected = (self.selected as isize).saturating_add(by).clamp(0, len as isize - 1) as usize;
        }
    }

    /// Keeps the selection inside a list that may have shrunk.
    pub fn clamp(&mut self, len: usize) {
        self.selected = self.selected.min(len.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The viewport moves only as far as it must, in either direction.
    #[test]
    fn the_viewport_follows_the_selection_by_the_least_scrolling() {
        let mut cursor = Cursor { selected: 40, offset: 0 };
        cursor.follow(10);
        assert_eq!(cursor.offset, 31);
        cursor.selected = 35;
        cursor.follow(10);
        assert_eq!(cursor.offset, 31, "scrolled although the selection was on screen");
        cursor.selected = 5;
        cursor.follow(10);
        assert_eq!(cursor.offset, 5);
    }

    #[test]
    fn stepping_stops_at_the_ends_and_an_empty_list_stays_put() {
        let mut cursor = Cursor::default();
        cursor.step(-3, 5);
        assert_eq!(cursor.selected, 0);
        cursor.step(isize::MAX, 5);
        assert_eq!(cursor.selected, 4);
        cursor.step(1, 0);
        assert_eq!(cursor.selected, 4);
        cursor.clamp(2);
        assert_eq!(cursor.selected, 1);
    }
}
