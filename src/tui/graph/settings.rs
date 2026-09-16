//! The graph view's settings, kept the way Obsidian keeps a graph's: filters,
//! groups, display and forces, with Obsidian's own defaults and ranges, and
//! saved beside the board the way Obsidian saves `graph.json` in a vault.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The file, in the board's `.ekko/` directory.
pub const FILE: &str = "graph.json";

/// A group: the items a search finds, drawn in one colour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub query: String,
    /// A colour of the group palette, by its place in it.
    pub colour: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    // Filters.
    pub search: String,
    /// Boards as nodes, as Obsidian's tags.
    pub tags: bool,
    /// Leaves out the other end of a relation that is not on the board.
    pub existing_only: bool,
    pub orphans: bool,
    // Groups.
    pub groups: Vec<Group>,
    // Display.
    pub arrows: bool,
    pub text_fade: f64,
    pub node_size: f64,
    pub link_thickness: f64,
    // Forces.
    pub center: f64,
    pub repel: f64,
    pub link_force: f64,
    pub link_distance: f64,
    /// How many links out a local graph reaches.
    pub depth: usize,
}

impl Default for Settings {
    /// Obsidian's defaults, with one addition asked for: groups for the states
    /// work can be in, already made -- and as removable as any other group.
    fn default() -> Self {
        Settings {
            search: String::new(),
            tags: false,
            existing_only: false,
            orphans: true,
            groups: vec![
                Group { query: "is:progress".into(), colour: 1 },
                Group { query: "is:paused".into(), colour: 2 },
                Group { query: "is:done".into(), colour: 0 },
                Group { query: "is:notes".into(), colour: 3 },
            ],
            arrows: false,
            text_fade: 0.0,
            node_size: 1.0,
            link_thickness: 1.0,
            center: 0.518713,
            repel: 10.0,
            link_force: 1.0,
            link_distance: 250.0,
            depth: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    Tags,
    ExistingOnly,
    Orphans,
    Arrows,
}

impl Toggle {
    pub fn label(self) -> &'static str {
        match self {
            Toggle::Tags => "Tags",
            Toggle::ExistingOnly => "Existing items only",
            Toggle::Orphans => "Orphans",
            Toggle::Arrows => "Arrows",
        }
    }

    pub fn get(self, settings: &Settings) -> bool {
        match self {
            Toggle::Tags => settings.tags,
            Toggle::ExistingOnly => settings.existing_only,
            Toggle::Orphans => settings.orphans,
            Toggle::Arrows => settings.arrows,
        }
    }

    pub fn flip(self, settings: &mut Settings) {
        let value = match self {
            Toggle::Tags => &mut settings.tags,
            Toggle::ExistingOnly => &mut settings.existing_only,
            Toggle::Orphans => &mut settings.orphans,
            Toggle::Arrows => &mut settings.arrows,
        };
        *value = !*value;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slider {
    TextFade,
    NodeSize,
    LinkThickness,
    Center,
    Repel,
    LinkForce,
    LinkDistance,
    Depth,
}

impl Slider {
    pub const ALL: [Slider; 8] = [
        Slider::TextFade,
        Slider::NodeSize,
        Slider::LinkThickness,
        Slider::Center,
        Slider::Repel,
        Slider::LinkForce,
        Slider::LinkDistance,
        Slider::Depth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Slider::TextFade => "Text fade threshold",
            Slider::NodeSize => "Node size",
            Slider::LinkThickness => "Link thickness",
            Slider::Center => "Center force",
            Slider::Repel => "Repel force",
            Slider::LinkForce => "Link force",
            Slider::LinkDistance => "Link distance",
            Slider::Depth => "Depth",
        }
    }

    /// The least, the most, and one step, as Obsidian's sliders have them.
    pub fn range(self) -> (f64, f64, f64) {
        match self {
            Slider::TextFade => (-3.0, 3.0, 0.1),
            Slider::NodeSize => (0.1, 5.0, 0.1),
            Slider::LinkThickness => (0.1, 5.0, 0.1),
            Slider::Center => (0.0, 1.0, 0.01),
            Slider::Repel => (0.0, 20.0, 0.5),
            Slider::LinkForce => (0.0, 1.0, 0.01),
            Slider::LinkDistance => (30.0, 500.0, 10.0),
            Slider::Depth => (1.0, 5.0, 1.0),
        }
    }

    pub fn get(self, settings: &Settings) -> f64 {
        match self {
            Slider::TextFade => settings.text_fade,
            Slider::NodeSize => settings.node_size,
            Slider::LinkThickness => settings.link_thickness,
            Slider::Center => settings.center,
            Slider::Repel => settings.repel,
            Slider::LinkForce => settings.link_force,
            Slider::LinkDistance => settings.link_distance,
            Slider::Depth => settings.depth as f64,
        }
    }

    /// Sets a value, kept inside the slider's range.
    pub fn set(self, settings: &mut Settings, value: f64) {
        let (least, most, _) = self.range();
        let value = if value.is_finite() { value.clamp(least, most) } else { least };
        match self {
            Slider::TextFade => settings.text_fade = value,
            Slider::NodeSize => settings.node_size = value,
            Slider::LinkThickness => settings.link_thickness = value,
            Slider::Center => settings.center = value,
            Slider::Repel => settings.repel = value,
            Slider::LinkForce => settings.link_force = value,
            Slider::LinkDistance => settings.link_distance = value,
            Slider::Depth => settings.depth = value.round() as usize,
        }
    }

    /// Moves by `steps` of the slider's step.
    pub fn nudge(self, settings: &mut Settings, steps: f64) {
        let (_, _, step) = self.range();
        let moved = self.get(settings) + steps * step;
        self.set(settings, (moved / step).round() * step);
    }

    /// Where the value sits between the least and the most, from 0 to 1.
    pub fn fraction(self, settings: &Settings) -> f64 {
        let (least, most, _) = self.range();
        (self.get(settings) - least) / (most - least)
    }
}

/// The settings panel's sections, closed until opened, as Obsidian's are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Filters,
    Groups,
    Display,
    Forces,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Filters => "Filters",
            Section::Groups => "Groups",
            Section::Display => "Display",
            Section::Forces => "Forces",
        }
    }
}

/// One row of the settings panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Section(Section),
    Search,
    Toggle(Toggle),
    Group(usize),
    NewGroup,
    Slider(Slider),
    Animate,
    Restore,
}

/// The panel's rows, with the sections in `open` opened. A local graph has a
/// depth; the whole board's does not.
pub fn controls(settings: &Settings, open: &HashSet<Section>, local: bool) -> Vec<Control> {
    let mut rows = Vec::new();
    for section in [Section::Filters, Section::Groups, Section::Display, Section::Forces] {
        rows.push(Control::Section(section));
        if !open.contains(&section) {
            continue;
        }
        match section {
            Section::Filters => {
                if local {
                    rows.push(Control::Slider(Slider::Depth));
                }
                rows.extend([
                    Control::Search,
                    Control::Toggle(Toggle::Tags),
                    Control::Toggle(Toggle::ExistingOnly),
                    Control::Toggle(Toggle::Orphans),
                ]);
            }
            Section::Groups => {
                rows.extend((0..settings.groups.len()).map(Control::Group));
                rows.push(Control::NewGroup);
            }
            Section::Display => rows.extend([
                Control::Toggle(Toggle::Arrows),
                Control::Slider(Slider::TextFade),
                Control::Slider(Slider::NodeSize),
                Control::Slider(Slider::LinkThickness),
                Control::Animate,
            ]),
            Section::Forces => rows.extend([
                Control::Slider(Slider::Center),
                Control::Slider(Slider::Repel),
                Control::Slider(Slider::LinkForce),
                Control::Slider(Slider::LinkDistance),
            ]),
        }
    }
    rows.push(Control::Restore);
    rows
}

impl Settings {
    /// The settings saved beside the board, or the defaults when there are
    /// none or they do not read. Values out of range are brought back in.
    pub fn load(dir: &Path) -> Settings {
        let read = std::fs::read_to_string(dir.join(FILE)).ok().and_then(|text| serde_json::from_str::<Settings>(&text).ok());
        let mut settings = read.unwrap_or_default();
        for slider in Slider::ALL {
            let value = slider.get(&settings);
            slider.set(&mut settings, value);
        }
        settings
    }

    /// Saves the settings beside the board, whole, through a rename, so a
    /// reader never sees half a file.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let temp = dir.join(format!("{FILE}.{}", std::process::id()));
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, dir.join(FILE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults are Obsidian's, and the preset groups put work in
    /// progress and paused ahead of `done`.
    #[test]
    fn the_defaults_are_obsidians() {
        let settings = Settings::default();
        assert!(!settings.tags && !settings.existing_only && settings.orphans && !settings.arrows);
        assert_eq!((settings.repel, settings.link_force, settings.link_distance), (10.0, 1.0, 250.0));
        assert!((settings.center - 0.518713).abs() < 1e-9);
        assert_eq!(settings.groups[0].query, "is:progress");
    }

    #[test]
    fn sliders_stay_in_range_and_move_by_their_steps() {
        let mut settings = Settings::default();
        Slider::Repel.set(&mut settings, 99.0);
        assert_eq!(settings.repel, 20.0);
        Slider::LinkDistance.nudge(&mut settings, -3.0);
        assert_eq!(settings.link_distance, 220.0);
        Slider::Depth.nudge(&mut settings, 10.0);
        assert_eq!(settings.depth, 5);
        assert_eq!(Slider::Depth.fraction(&settings), 1.0);
        Slider::NodeSize.set(&mut settings, f64::NAN);
        assert_eq!(settings.node_size, 0.1);
    }

    /// Closed sections show their heading alone; an opened one shows its
    /// controls, and a local graph's filters begin with its depth.
    #[test]
    fn the_panel_opens_section_by_section() {
        let settings = Settings::default();
        let closed = controls(&settings, &HashSet::new(), false);
        assert_eq!(closed.len(), 5);
        let open = controls(&settings, &HashSet::from([Section::Groups, Section::Filters]), true);
        assert_eq!(open[1], Control::Slider(Slider::Depth));
        assert!(open.contains(&Control::Group(3)) && open.contains(&Control::NewGroup));
        assert_eq!(open.last(), Some(&Control::Restore));
    }

    /// Saved settings read back as they were, and a file that does not read,
    /// or holds values out of range, gives settings that are safe to use.
    #[test]
    fn settings_are_saved_and_read_back_safely() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ekko-graph-settings-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(Settings::load(&dir), Settings::default());
        let changed = Settings { tags: true, repel: 4.5, search: "is:pending".into(), ..Settings::default() };
        changed.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), changed);

        std::fs::write(dir.join(FILE), r#"{"repel": 400, "linkDistance": -1}"#).unwrap();
        let clamped = Settings::load(&dir);
        assert_eq!((clamped.repel, clamped.link_distance), (20.0, 30.0));
        std::fs::write(dir.join(FILE), "not json").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        std::fs::remove_dir_all(&dir).ok();
    }
}
