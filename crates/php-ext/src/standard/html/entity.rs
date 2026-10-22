//! Entity table types of html_tables.h and their lookups.

/// One row of a code point -> entity table (`entity_stage3_row`).
#[derive(Clone, Copy, Debug)]
pub enum EntRow {
    /// No entity for this code point.
    Empty,
    /// The entity name (without `&` and `;`).
    One(&'static str),
    /// A code point that starts HTML5 entities of two code points (`ambiguous` rows).
    Multi(&'static MultiCp),
}

/// Entities starting with one code point (`entity_multicodepoint_row`).
#[derive(Debug)]
pub struct MultiCp {
    /// Entity of the code point on its own, if any.
    pub default: Option<&'static str>,
    /// `(second code point, entity)` pairs.
    pub rest: &'static [(u32, &'static str)],
}

/// Three-stage code point -> entity table (`entity_stage1_row[]`). Stage 1 and 2 hold indices
/// of the next stage's blocks; block 0 of each stage is empty.
#[derive(Debug)]
pub struct MsTable {
    pub stage1: [u8; 0x1E],
    pub stage2: &'static [[u8; 64]],
    pub stage3: &'static [[EntRow; 64]],
}

impl MsTable {
    /// Row of code point `cp` (`table[ENT_STAGE1_INDEX(k)][ENT_STAGE2_INDEX(k)][ENT_STAGE3_INDEX(k)]`),
    /// `None` beyond the table (stage 1 index above 0x1D).
    pub fn row(&self, cp: u32) -> Option<&'static EntRow> {
        let s1 = ((cp & 0xFF_F000) >> 12) as usize;
        let block = *self.stage1.get(s1)?;
        let s3 = self.stage2[usize::from(block)][((cp & 0xFC0) >> 6) as usize];
        Some(&self.stage3[usize::from(s3)][(cp & 0x3F) as usize])
    }

    /// Whether the stage 2 block of stage 1 index `i` is empty (`== empty_stage2_table`).
    pub fn stage1_is_empty(&self, i: usize) -> bool {
        self.stage1[i] == 0
    }

    /// Whether the stage 3 block at `(i, j)` is empty (`== empty_stage3_table`).
    pub fn stage2_is_empty(&self, i: usize, j: usize) -> bool {
        self.stage2[usize::from(self.stage1[i])][j] == 0
    }
}

/// Entity -> code points (`entity_cp_map`): name, first code point and second code point
/// (0 when the entity stands for a single code point).
pub type NamedEntity = (&'static str, u32, u32);

/// Looks an entity name up in a name-sorted map (`resolve_named_entity_html`).
pub fn resolve_named_entity(map: &[NamedEntity], name: &[u8]) -> Option<(u32, u32)> {
    map.binary_search_by(|e| e.0.as_bytes().cmp(name))
        .ok()
        .map(|i| (map[i].1, map[i].2))
}
