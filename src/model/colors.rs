//! Which colour a block takes: from its task's tags, its task's list or its
//! calendar, in the order the person put them, as one colour or as a
//! gradient of them all.

use crate::strings::tr;
use super::*;
use serde::{Deserialize, Serialize};

/// Where a block's colour may come from.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Hash)]
pub enum ColorSource {
    Tags,
    Lists,
    Calendars,
}

impl ColorSource {
    pub const ALL: [ColorSource; 3] = [ColorSource::Lists, ColorSource::Tags, ColorSource::Calendars];
    pub fn label(self) -> &'static str {
        match self {
            ColorSource::Tags => tr("Tags"),
            ColorSource::Lists => tr("Task lists"),
            ColorSource::Calendars => tr("Calendars"),
        }
    }
}

/// One source in the priority list, and whether its colour is used.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug)]
pub struct SourcePref {
    pub source: ColorSource,
    pub on: bool,
}

/// How a block is painted.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug, Default)]
pub enum BlockScheme {
    /// the colour of the first source in the priority list that has one
    #[default]
    Primary,
    /// a gradient of every source that has one: the list's along the top,
    /// the tag's from the bottom left, the calendar's from the bottom right
    Dynamic,
}

/// The colours a block could take, one from each source that is on.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct BlockColors {
    pub list: Option<[u8; 3]>,
    pub tag: Option<[u8; 3]>,
    pub cal: Option<[u8; 3]>,
}

impl BlockColors {
    fn get(&self, s: ColorSource) -> Option<[u8; 3]> {
        match s {
            ColorSource::Tags => self.tag,
            ColorSource::Lists => self.list,
            ColorSource::Calendars => self.cal,
        }
    }
    fn count(&self) -> usize {
        [self.list, self.tag, self.cal].iter().filter(|c| c.is_some()).count()
    }
    /// The four corners of the gradient: top left, top right, bottom left,
    /// bottom right. The list's colour runs along the top, the tag's comes
    /// from the bottom left and the calendar's from the bottom right; a
    /// missing one leaves its corners to the others.
    pub fn corners(&self) -> Option<[[u8; 3]; 4]> {
        let any = self.list.or(self.tag).or(self.cal)?;
        let bl = self.tag.or(self.cal).unwrap_or(any);
        let br = self.cal.or(self.tag).unwrap_or(any);
        let tl = self.list.unwrap_or(bl);
        let tr = self.list.unwrap_or(br);
        Some([tl, tr, bl, br])
    }
}

/// What a block is painted with.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BlockPaint {
    Solid([u8; 3]),
    /// top left, top right, bottom left, bottom right
    Gradient([[u8; 3]; 4]),
}

impl BlockPaint {
    pub fn gradient(&self) -> Option<[[u8; 3]; 4]> {
        match self {
            BlockPaint::Solid(_) => None,
            BlockPaint::Gradient(g) => Some(*g),
        }
    }
}

/// A tag's colour when none is picked: the same for the same name, whatever
/// its case, from the palette without its grey.
pub fn auto_tag_color(name: &str) -> [u8; 3] {
    // FNV-1a, so it stays put from one run and one version to the next
    let mut h: u32 = 0x811c_9dc5;
    for b in name.trim().to_lowercase().bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    // and mixed, as FNV's low bits alone repeat too often
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    let n = EVENT_COLORS.len() - 1;
    EVENT_COLORS[h as usize % n].0
}

impl Prefs {
    /// The sources in the order the person put them, each once, any missing
    /// one added at the end, switched on.
    pub fn color_order(&self) -> Vec<SourcePref> {
        let mut out: Vec<SourcePref> = Vec::new();
        for p in &self.color_order {
            if !out.iter().any(|o| o.source == p.source) {
                out.push(*p);
            }
        }
        for s in ColorSource::ALL {
            if !out.iter().any(|o| o.source == s) {
                out.push(SourcePref { source: s, on: true });
            }
        }
        out
    }
    fn source_on(&self, s: ColorSource) -> bool {
        self.color_order().iter().any(|p| p.source == s && p.on)
    }
    /// The colours left once the sources switched off are taken out.
    pub fn colors_on(&self, c: BlockColors) -> BlockColors {
        BlockColors {
            list: c.list.filter(|_| self.source_on(ColorSource::Lists)),
            tag: c.tag.filter(|_| self.source_on(ColorSource::Tags)),
            cal: c.cal.filter(|_| self.source_on(ColorSource::Calendars)),
        }
    }
    /// The colour of the first source in the priority list that has one.
    pub fn first_color(&self, c: &BlockColors) -> Option<[u8; 3]> {
        self.color_order().iter().filter(|p| p.on).find_map(|p| c.get(p.source))
    }
    /// How a block with these colours is painted: as the scheme says, with
    /// `fallback` when no source gives one.
    pub fn paint_of(&self, c: &BlockColors, fallback: [u8; 3]) -> BlockPaint {
        let c = self.colors_on(*c);
        match (self.block_scheme, c.corners()) {
            (BlockScheme::Dynamic, Some(g)) if c.count() > 1 => BlockPaint::Gradient(g),
            _ => BlockPaint::Solid(self.first_color(&c).unwrap_or(fallback)),
        }
    }
}

impl Data {
    /// A tag's colour: the one picked for it, else its own automatic one.
    pub fn tag_color(&self, name: &str) -> [u8; 3] {
        self.tag_picked(name).unwrap_or_else(|| auto_tag_color(name))
    }
    /// The colour picked for a tag, if one was.
    pub fn tag_picked(&self, name: &str) -> Option<[u8; 3]> {
        self.prefs.tag_colors.get(&name.trim().to_lowercase()).copied()
    }
    /// Picks a tag's colour; None goes back to its automatic one.
    pub fn set_tag_color(&mut self, name: &str, color: Option<[u8; 3]>) {
        let key = name.trim().to_lowercase();
        match color {
            Some(c) => self.prefs.tag_colors.insert(key, c),
            None => self.prefs.tag_colors.remove(&key),
        };
    }

    /// The colours a block of `task` (or of no task) in calendar `cal` could
    /// take, one from each source, switched on or not.
    pub fn block_colors(&self, task: Option<&Task>, cal: u64) -> BlockColors {
        BlockColors {
            list: task.map(|t| self.color_of_list(t.list)),
            tag: task.and_then(|t| t.tags.iter().find(|g| !g.trim().is_empty())).map(|g| self.tag_color(g)),
            cal: Some(self.calendar_color(cal)),
        }
    }

    /// The colour of the first source switched on in the priority list that
    /// has one, else the main colour.
    pub fn primary_color(&self, c: &BlockColors) -> [u8; 3] {
        self.prefs.first_color(&self.prefs.colors_on(*c)).unwrap_or_else(|| self.main_color())
    }

    /// How a block is painted: a colour picked for it (or for its task)
    /// alone; else as the colour scheme says.
    pub fn block_paint(&self, own: Option<[u8; 3]>, task: Option<&Task>, cal: u64) -> BlockPaint {
        match own.or_else(|| task.and_then(|t| t.color)) {
            Some(c) => BlockPaint::Solid(c),
            None => self.prefs.paint_of(&self.block_colors(task, cal), self.main_color()),
        }
    }

    /// How an event's block is painted.
    pub fn event_paint(&self, e: &Event) -> BlockPaint {
        self.block_paint(e.color, e.task_id.and_then(|t| self.task(t)), e.calendar)
    }
}
