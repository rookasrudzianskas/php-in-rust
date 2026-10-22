//! Port of `ext/standard/libavifinfo/avifinfo.c` (the streamed API) as used by
//! `php_is_image_avif()` and `php_handle_avif()` in `image.c`.
//!
//! libavifinfo (c) 2021 Alliance for Open Media, BSD-2-Clause and the AOM
//! Patent License 1.0. All arithmetic on box sizes is `uint32_t` in the C
//! source and is kept wrapping here, because malformed files can underflow it.

use super::stream::Stream;

/// `AvifInfoStatus` (the public result).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvifInfoStatus {
    Ok,
    NotEnoughData,
    TooComplex,
    InvalidFile,
}

/// `AvifInfoFeatures` (the part getimagesize() uses).
#[derive(Debug, Clone, Copy, Default)]
pub struct AvifInfoFeatures {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u32,
    pub num_channels: u32,
    has_gainmap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    Found,
    NotFound,
    Truncated,
    Aborted,
    Invalid,
}

fn convert_status(s: St) -> AvifInfoStatus {
    match s {
        St::Found => AvifInfoStatus::Ok,
        St::NotFound | St::Truncated => AvifInfoStatus::NotEnoughData,
        St::Aborted => AvifInfoStatus::TooComplex,
        St::Invalid => AvifInfoStatus::InvalidFile,
    }
}

const MAX_SIZE: u32 = u32::MAX;
const MAX_NUM_BOXES: u32 = 4096;
const MAX_VALUE: u32 = u8::MAX as u32;
const MAX_TILES: usize = 16;
const MAX_PROPS: usize = 32;
const MAX_FEATURES: usize = 8;
const MAX_NUM_READ_BYTES: usize = 64;

macro_rules! check {
    ($cond:expr, $status:expr) => {
        if !($cond) {
            return $status;
        }
    };
}
macro_rules! check_found {
    ($e:expr) => {{
        let s = $e;
        if s != St::Found {
            return s;
        }
    }};
}
macro_rules! check_not_found {
    ($e:expr) => {{
        let s = $e;
        if s != St::NotFound {
            return s;
        }
    }};
}

/// `struct php_avif_stream` + `AvifInfoInternalStream`.
struct AvifStream<'a> {
    /// `None` once a read or skip failed ("fail further calls").
    stream: Option<&'a mut Stream>,
    buffer: [u8; MAX_NUM_READ_BYTES],
}

impl AvifStream<'_> {
    /// `AvifInfoInternalRead`: the bytes are available in `self.buffer`.
    fn read(&mut self, num_bytes: usize) -> St {
        let ok = match self.stream.as_deref_mut() {
            None => false,
            Some(s) => s.read(&mut self.buffer[..num_bytes]) == num_bytes as isize,
        };
        if !ok {
            self.stream = None;
            return St::Truncated;
        }
        St::Found
    }

    /// `AvifInfoInternalSkip`.
    fn skip(&mut self, num_bytes: u32) -> St {
        if num_bytes > 0 {
            let failed = self
                .stream
                .as_deref_mut()
                .is_some_and(|s| s.seek_cur(num_bytes as i64) != 0);
            if failed {
                self.stream = None;
            }
        }
        St::Found
    }

    fn data(&self) -> &[u8; MAX_NUM_READ_BYTES] {
        &self.buffer
    }
}

fn read_be(input: &[u8], num_bytes: u32) -> u32 {
    let mut value = 0u32;
    for &b in &input[..num_bytes as usize] {
        value = (value << 8) | b as u32;
    }
    value
}

#[derive(Clone, Copy, Default)]
struct Tile {
    tile_item_id: u8,
    parent_item_id: u8,
    dimg_idx: u8,
}

#[derive(Clone, Copy, Default)]
struct Prop {
    property_index: u8,
    item_id: u8,
}

#[derive(Clone, Copy, Default)]
struct DimProp {
    property_index: u8,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Default)]
struct ChanProp {
    property_index: u8,
    bit_depth: u8,
    num_channels: u8,
}

#[derive(Default)]
struct InternalFeatures {
    has_primary_item: bool,
    has_alpha: bool,
    gainmap_property_index: u8,
    primary_item_id: u8,
    primary_item_features: AvifInfoFeatures,
    data_was_skipped: bool,
    tone_mapped_item_id: u8,
    iinf_parsed: bool,
    iref_parsed: bool,
    num_tiles: usize,
    tiles: [Tile; MAX_TILES],
    num_props: usize,
    props: [Prop; MAX_PROPS],
    num_dim_props: usize,
    dim_props: [DimProp; MAX_FEATURES],
    num_chan_props: usize,
    chan_props: [ChanProp; MAX_FEATURES],
}

const UNDEFINED: u32 = 0;

fn get_item_features(f: &mut InternalFeatures, target_item_id: u32, tile_depth: u32) -> St {
    for prop_item in 0..f.num_props {
        if f.props[prop_item].item_id as u32 != target_item_id {
            continue;
        }
        let property_index = f.props[prop_item].property_index;

        // Retrieve the width and height of the primary item if not already done.
        if target_item_id == f.primary_item_id as u32
            && (f.primary_item_features.width == UNDEFINED
                || f.primary_item_features.height == UNDEFINED)
        {
            for i in 0..f.num_dim_props {
                if f.dim_props[i].property_index != property_index {
                    continue;
                }
                f.primary_item_features.width = f.dim_props[i].width;
                f.primary_item_features.height = f.dim_props[i].height;
                if f.primary_item_features.bit_depth != UNDEFINED
                    && f.primary_item_features.num_channels != UNDEFINED
                {
                    return St::Found;
                }
                break;
            }
        }
        // Retrieve the bit depth and number of channels of the target item.
        if f.primary_item_features.bit_depth == UNDEFINED
            || f.primary_item_features.num_channels == UNDEFINED
        {
            for i in 0..f.num_chan_props {
                if f.chan_props[i].property_index != property_index {
                    continue;
                }
                f.primary_item_features.bit_depth = f.chan_props[i].bit_depth as u32;
                f.primary_item_features.num_channels = f.chan_props[i].num_channels as u32;
                if f.primary_item_features.width != UNDEFINED
                    && f.primary_item_features.height != UNDEFINED
                {
                    return St::Found;
                }
                break;
            }
        }
    }

    // Check for the bit_depth and num_channels in a tile if not yet found.
    let mut tile = 0;
    while tile < f.num_tiles && tile_depth < 3 {
        if f.tiles[tile].parent_item_id as u32 == target_item_id {
            let child = f.tiles[tile].tile_item_id as u32;
            check_not_found!(get_item_features(f, child, tile_depth + 1));
        }
        tile += 1;
    }
    St::NotFound
}

fn get_primary_item_features(f: &mut InternalFeatures) -> St {
    check!(f.has_primary_item, St::NotFound);
    check!(f.num_dim_props > 0 && f.num_chan_props != 0, St::NotFound);

    // Look for a gain map. HEIF scheme: a hidden input of a derived item.
    if f.tone_mapped_item_id != 0 {
        for tile in 0..f.num_tiles {
            if f.tiles[tile].parent_item_id == f.tone_mapped_item_id && f.tiles[tile].dimg_idx == 1
            {
                f.primary_item_features.has_gainmap = true;
                break;
            }
        }
    }
    // Adobe scheme: gain map is an auxiliary item.
    if !f.primary_item_features.has_gainmap && f.gainmap_property_index > 0 {
        for prop_item in 0..f.num_props {
            if f.props[prop_item].property_index == f.gainmap_property_index {
                f.primary_item_features.has_gainmap = true;
                break;
            }
        }
    }
    // Not all metadata read yet: a gain map might still show up later.
    if !f.primary_item_features.has_gainmap
        && (!f.iinf_parsed || (f.tone_mapped_item_id != 0 && !f.iref_parsed))
    {
        return St::NotFound;
    }

    check_found!(get_item_features(f, f.primary_item_id as u32, 0));

    // "auxC" is parsed before the "ipma" properties so it is known now, if any.
    if f.has_alpha {
        f.primary_item_features.num_channels += 1;
    }
    St::Found
}

#[derive(Default)]
struct BoxHeader {
    size: u32,
    kind: [u8; 4],
    version: u32,
    flags: u32,
    content_size: u32,
}

fn parse_box(
    nesting_level: i32,
    stream: &mut AvifStream<'_>,
    num_remaining_bytes: u32,
    num_parsed_boxes: &mut u32,
    b: &mut BoxHeader,
) -> St {
    let mut box_header_size: u32 = 8;
    check!(box_header_size <= num_remaining_bytes, St::Invalid);
    check_found!(stream.read(8));
    b.size = read_be(stream.data(), 4);
    b.kind.copy_from_slice(&stream.data()[4..8]);
    if b.size == 1 {
        box_header_size += 8;
        check!(box_header_size <= num_remaining_bytes, St::Invalid);
        check_found!(stream.read(8));
        check!(read_be(stream.data(), 4) == 0, St::Aborted);
        b.size = read_be(&stream.data()[4..], 4);
    } else if b.size == 0 {
        check!(nesting_level == 0, St::Invalid);
        b.size = num_remaining_bytes;
    }
    check!(b.size >= box_header_size, St::Invalid);
    check!(b.size <= num_remaining_bytes, St::Invalid);

    let has_fullbox_header = [
        b"meta", b"pitm", b"ipma", b"ispe", b"pixi", b"iref", b"auxC", b"iinf", b"infe",
    ]
    .iter()
    .any(|t| **t == b.kind);
    if has_fullbox_header {
        box_header_size += 4;
    }
    check!(b.size >= box_header_size, St::Invalid);
    b.content_size = b.size - box_header_size;
    if nesting_level != 0 || &b.kind != b"ftyp" {
        *num_parsed_boxes += 1;
        check!(*num_parsed_boxes < MAX_NUM_BOXES, St::Aborted);
    }

    b.version = 0;
    b.flags = 0;
    if has_fullbox_header {
        check_found!(stream.read(4));
        b.version = read_be(stream.data(), 1);
        b.flags = read_be(&stream.data()[1..], 3);
        let k = &b.kind;
        let is_parsable = (k == b"meta" && b.version == 0)
            || (k == b"pitm" && b.version <= 1)
            || (k == b"ipma" && b.version <= 1)
            || (k == b"ispe" && b.version == 0)
            || (k == b"pixi" && b.version == 0)
            || (k == b"iref" && b.version <= 1)
            || (k == b"auxC" && b.version == 0)
            || (k == b"iinf" && b.version <= 1)
            || (k == b"infe" && b.version >= 2 && b.version <= 3);
        // Instead of considering this file as invalid, skip unparsable boxes.
        if !is_parsable {
            b.kind = *b"skip";
        }
    }
    St::Found
}

fn parse_ipco(
    nesting_level: i32,
    stream: &mut AvifStream<'_>,
    mut num_remaining_bytes: u32,
    num_parsed_boxes: &mut u32,
    features: &mut InternalFeatures,
) -> St {
    let mut box_index: u32 = 1;
    loop {
        let mut b = BoxHeader::default();
        check_found!(parse_box(
            nesting_level,
            stream,
            num_remaining_bytes,
            num_parsed_boxes,
            &mut b
        ));

        if &b.kind == b"ispe" {
            check!(b.content_size >= 8, St::Invalid);
            check_found!(stream.read(8));
            let width = read_be(stream.data(), 4);
            let height = read_be(&stream.data()[4..], 4);
            check!(width != 0 && height != 0, St::Invalid);
            if features.num_dim_props < MAX_FEATURES && box_index <= MAX_VALUE {
                features.dim_props[features.num_dim_props] = DimProp {
                    property_index: box_index as u8,
                    width,
                    height,
                };
                features.num_dim_props += 1;
            } else {
                features.data_was_skipped = true;
            }
            check_found!(stream.skip(b.content_size - 8));
        } else if &b.kind == b"pixi" {
            check!(b.content_size >= 1, St::Invalid);
            check_found!(stream.read(1));
            let num_channels = read_be(stream.data(), 1);
            check!(num_channels >= 1, St::Invalid);
            check!(b.content_size > num_channels, St::Invalid);
            check_found!(stream.read(1));
            let bit_depth = read_be(stream.data(), 1);
            check!(bit_depth >= 1, St::Invalid);
            for i in 1..num_channels {
                check_found!(stream.read(1));
                // Bit depth should be the same for all channels.
                check!(read_be(stream.data(), 1) == bit_depth, St::Invalid);
                check!(i <= 32, St::Aborted);
            }
            if features.num_chan_props < MAX_FEATURES
                && box_index <= MAX_VALUE
                && bit_depth <= MAX_VALUE
                && num_channels <= MAX_VALUE
            {
                features.chan_props[features.num_chan_props] = ChanProp {
                    property_index: box_index as u8,
                    bit_depth: bit_depth as u8,
                    num_channels: num_channels as u8,
                };
                features.num_chan_props += 1;
            } else {
                features.data_was_skipped = true;
            }
            check_found!(stream.skip(b.content_size.wrapping_sub(1 + num_channels)));
        } else if &b.kind == b"av1C" {
            // Only the necessary third byte is parsed; the others are assumed valid.
            check!(b.content_size >= 3, St::Invalid);
            check_found!(stream.read(3));
            let byte = stream.data()[2];
            let high_bitdepth = byte & 0x40 != 0;
            let twelve_bit = byte & 0x20 != 0;
            let monochrome = byte & 0x10 != 0;
            if twelve_bit {
                check!(high_bitdepth, St::Invalid);
            }
            if features.num_chan_props < MAX_FEATURES && box_index <= MAX_VALUE {
                features.chan_props[features.num_chan_props] = ChanProp {
                    property_index: box_index as u8,
                    bit_depth: if high_bitdepth {
                        if twelve_bit { 12 } else { 10 }
                    } else {
                        8
                    },
                    num_channels: if monochrome { 1 } else { 3 },
                };
                features.num_chan_props += 1;
            } else {
                features.data_was_skipped = true;
            }
            check_found!(stream.skip(b.content_size - 3));
        } else if &b.kind == b"auxC" {
            const ALPHA: &[u8; 44] = b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0";
            const ALPHA_LEN: u32 = 44;
            const GAINMAP: &[u8; 29] = b"urn:com:photo:aux:hdrgainmap\0";
            const GAINMAP_LEN: u32 = 29;
            let mut num_read_bytes: u32 = 0;
            // Check for a gain map or for an alpha plane. Start with the gain
            // map since the identifier is shorter.
            if b.content_size >= GAINMAP_LEN {
                check_found!(stream.read(GAINMAP_LEN as usize));
                num_read_bytes = GAINMAP_LEN;
                let aux_type = &stream.data()[..GAINMAP_LEN as usize];
                if aux_type == &GAINMAP[..] {
                    if box_index <= MAX_VALUE {
                        features.gainmap_property_index = box_index as u8;
                    } else {
                        features.data_was_skipped = true;
                    }
                } else if b.content_size >= ALPHA_LEN && aux_type == &ALPHA[..GAINMAP_LEN as usize]
                {
                    // The beginning of the aux type matches the alpha string; check the end too.
                    let end_length = ALPHA_LEN - GAINMAP_LEN;
                    check_found!(stream.read(end_length as usize));
                    num_read_bytes = ALPHA_LEN;
                    if stream.data()[..end_length as usize] == ALPHA[GAINMAP_LEN as usize..] {
                        features.has_alpha = true;
                    }
                }
            }
            check_found!(stream.skip(b.content_size.wrapping_sub(num_read_bytes)));
        } else {
            check_found!(stream.skip(b.content_size));
        }
        box_index += 1;
        num_remaining_bytes = num_remaining_bytes.wrapping_sub(b.size);
        if num_remaining_bytes == 0 {
            break;
        }
    }
    St::NotFound
}

fn parse_iprp(
    nesting_level: i32,
    stream: &mut AvifStream<'_>,
    mut num_remaining_bytes: u32,
    num_parsed_boxes: &mut u32,
    features: &mut InternalFeatures,
) -> St {
    loop {
        let mut b = BoxHeader::default();
        check_found!(parse_box(
            nesting_level,
            stream,
            num_remaining_bytes,
            num_parsed_boxes,
            &mut b
        ));

        if &b.kind == b"ipco" {
            check_not_found!(parse_ipco(
                nesting_level + 1,
                stream,
                b.content_size,
                num_parsed_boxes,
                features
            ));
        } else if &b.kind == b"ipma" {
            let mut num_read_bytes: u32 = 4;
            check!(b.content_size >= num_read_bytes, St::Invalid);
            check_found!(stream.read(4));
            let entry_count = read_be(stream.data(), 4);
            let id_num_bytes: u32 = if b.version < 1 { 2 } else { 4 };
            let index_num_bytes: u32 = if b.flags & 1 != 0 { 2 } else { 1 };
            let essential_bit_mask: u32 = if b.flags & 1 != 0 { 0x8000 } else { 0x80 };

            let mut entry: u32 = 0;
            while entry < entry_count {
                if entry as usize >= MAX_PROPS || features.num_props >= MAX_PROPS {
                    features.data_was_skipped = true;
                    break;
                }
                num_read_bytes = num_read_bytes.wrapping_add(id_num_bytes + 1);
                check!(b.content_size >= num_read_bytes, St::Invalid);
                check_found!(stream.read((id_num_bytes + 1) as usize));
                let item_id = read_be(stream.data(), id_num_bytes);
                let association_count = read_be(&stream.data()[id_num_bytes as usize..], 1);

                let mut property: u32 = 0;
                while property < association_count {
                    if property as usize >= MAX_PROPS || features.num_props >= MAX_PROPS {
                        features.data_was_skipped = true;
                        break;
                    }
                    num_read_bytes = num_read_bytes.wrapping_add(index_num_bytes);
                    check!(b.content_size >= num_read_bytes, St::Invalid);
                    check_found!(stream.read(index_num_bytes as usize));
                    let value = read_be(stream.data(), index_num_bytes);
                    let property_index = value & !essential_bit_mask;
                    if property_index <= MAX_VALUE && item_id <= MAX_VALUE {
                        features.props[features.num_props] = Prop {
                            property_index: property_index as u8,
                            item_id: item_id as u8,
                        };
                        features.num_props += 1;
                    } else {
                        features.data_was_skipped = true;
                    }
                    property += 1;
                }
                if property < association_count {
                    break; // Do not read garbage.
                }
                entry += 1;
            }

            // If all features are available now, do not look further.
            check_not_found!(get_primary_item_features(features));

            // Mostly if 'data_was_skipped'.
            check_found!(stream.skip(b.content_size.wrapping_sub(num_read_bytes)));
        } else {
            check_found!(stream.skip(b.content_size));
        }
        num_remaining_bytes = num_remaining_bytes.wrapping_sub(b.size);
        if num_remaining_bytes == 0 {
            break;
        }
    }
    St::NotFound
}

fn parse_iref(
    nesting_level: i32,
    stream: &mut AvifStream<'_>,
    mut num_remaining_bytes: u32,
    num_parsed_boxes: &mut u32,
    features: &mut InternalFeatures,
) -> St {
    features.iref_parsed = true;

    while num_remaining_bytes > 0 {
        let mut b = BoxHeader::default();
        check_found!(parse_box(
            nesting_level,
            stream,
            num_remaining_bytes,
            num_parsed_boxes,
            &mut b
        ));

        if &b.kind == b"dimg" {
            let num_bytes_per_id: u32 = if b.version == 0 { 2 } else { 4 };
            let mut num_read_bytes = num_bytes_per_id + 2;
            check!(b.content_size >= num_read_bytes, St::Invalid);
            check_found!(stream.read((num_bytes_per_id + 2) as usize));
            let from_item_id = read_be(stream.data(), num_bytes_per_id);
            let reference_count = read_be(&stream.data()[num_bytes_per_id as usize..], 2);

            let mut i: u32 = 0;
            while i < reference_count {
                if i as usize >= MAX_TILES {
                    features.data_was_skipped = true;
                    break;
                }
                num_read_bytes = num_read_bytes.wrapping_add(num_bytes_per_id);
                check!(b.content_size >= num_read_bytes, St::Invalid);
                check_found!(stream.read(num_bytes_per_id as usize));
                let to_item_id = read_be(stream.data(), num_bytes_per_id);
                if from_item_id <= MAX_VALUE
                    && to_item_id <= MAX_VALUE
                    && features.num_tiles < MAX_TILES
                {
                    features.tiles[features.num_tiles] = Tile {
                        tile_item_id: to_item_id as u8,
                        parent_item_id: from_item_id as u8,
                        dimg_idx: i as u8,
                    };
                    features.num_tiles += 1;
                } else {
                    features.data_was_skipped = true;
                }
                i += 1;
            }

            // If all features are available now, do not look further.
            check_not_found!(get_primary_item_features(features));

            // Mostly if 'data_was_skipped'.
            check_found!(stream.skip(b.content_size.wrapping_sub(num_read_bytes)));
        } else {
            check_found!(stream.skip(b.content_size));
        }
        num_remaining_bytes = num_remaining_bytes.wrapping_sub(b.size);
    }
    St::NotFound
}

fn parse_iinf(
    nesting_level: i32,
    stream: &mut AvifStream<'_>,
    mut num_remaining_bytes: u32,
    box_version: u32,
    num_parsed_boxes: &mut u32,
    features: &mut InternalFeatures,
) -> St {
    features.iinf_parsed = true;

    let num_bytes_per_entry_count: u32 = if box_version == 0 { 2 } else { 4 };
    check!(
        num_bytes_per_entry_count <= num_remaining_bytes,
        St::Invalid
    );
    check_found!(stream.read(num_bytes_per_entry_count as usize));
    num_remaining_bytes -= num_bytes_per_entry_count;
    let entry_count = read_be(stream.data(), num_bytes_per_entry_count);

    // `for (int i = 0; i < entry_count; ++i)`: `i` is converted to unsigned.
    let mut i: u32 = 0;
    while i < entry_count {
        let mut b = BoxHeader::default();
        check_found!(parse_box(
            nesting_level,
            stream,
            num_remaining_bytes,
            num_parsed_boxes,
            &mut b
        ));

        if &b.kind == b"infe" {
            let num_bytes_per_id: u32 = if b.version == 2 { 2 } else { 4 };
            // item_ID (16 or 32) + item_protection_index (16) + item_type (32).
            check!(num_bytes_per_id + 2 + 4 <= b.content_size, St::Invalid);
            check_found!(stream.read(num_bytes_per_id as usize));
            let item_id = read_be(stream.data(), num_bytes_per_id);

            // Skip item_protection_index.
            check_found!(stream.skip(2));

            check_found!(stream.read(4));
            if &stream.data()[..4] == b"tmap" {
                // Tone Mapped Image: indicates the presence of a gain map.
                if item_id <= MAX_VALUE {
                    features.tone_mapped_item_id = item_id as u8;
                } else {
                    features.data_was_skipped = true;
                }
            }

            check_found!(stream.skip(b.content_size - (num_bytes_per_id + 2 + 4)));
        } else {
            check_found!(stream.skip(b.content_size));
        }

        num_remaining_bytes = num_remaining_bytes.wrapping_sub(b.size);
        if num_remaining_bytes == 0 {
            break; // Ignore entry_count bigger than box.
        }
        i += 1;
    }
    St::NotFound
}

fn parse_meta(
    nesting_level: i32,
    stream: &mut AvifStream<'_>,
    mut num_remaining_bytes: u32,
    num_parsed_boxes: &mut u32,
    features: &mut InternalFeatures,
) -> St {
    loop {
        let mut b = BoxHeader::default();
        check_found!(parse_box(
            nesting_level,
            stream,
            num_remaining_bytes,
            num_parsed_boxes,
            &mut b
        ));
        if &b.kind == b"pitm" {
            let num_bytes_per_id: u32 = if b.version == 0 { 2 } else { 4 };
            check!(num_bytes_per_id <= num_remaining_bytes, St::Invalid);
            check_found!(stream.read(num_bytes_per_id as usize));
            let primary_item_id = read_be(stream.data(), num_bytes_per_id);
            check!(primary_item_id <= MAX_VALUE, St::Aborted);
            features.has_primary_item = true;
            features.primary_item_id = primary_item_id as u8;
            check_found!(stream.skip(b.content_size.wrapping_sub(num_bytes_per_id)));
        } else if &b.kind == b"iprp" {
            check_not_found!(parse_iprp(
                nesting_level + 1,
                stream,
                b.content_size,
                num_parsed_boxes,
                features
            ));
        } else if &b.kind == b"iref" {
            check_not_found!(parse_iref(
                nesting_level + 1,
                stream,
                b.content_size,
                num_parsed_boxes,
                features
            ));
        } else if &b.kind == b"iinf" {
            check_not_found!(parse_iinf(
                nesting_level + 1,
                stream,
                b.content_size,
                b.version,
                num_parsed_boxes,
                features
            ));
        } else {
            check_found!(stream.skip(b.content_size));
        }
        num_remaining_bytes = num_remaining_bytes.wrapping_sub(b.size);
        if num_remaining_bytes == 0 {
            break;
        }
    }
    // According to ISO/IEC 14496-12:2012(E) 8.11.1.1 there is at most one "meta".
    if features.data_was_skipped {
        St::Aborted
    } else {
        St::Invalid
    }
}

fn parse_ftyp(stream: &mut AvifStream<'_>) -> St {
    let mut b = BoxHeader::default();
    let mut num_parsed_boxes = 0u32;
    check_found!(parse_box(
        0,
        stream,
        MAX_SIZE,
        &mut num_parsed_boxes,
        &mut b
    ));
    check!(&b.kind == b"ftyp", St::Invalid);
    // Iterate over brands. See ISO/IEC 14496-12:2012(E) 4.3.1
    check!(b.content_size >= 8, St::Invalid); // major_brand, minor_version
    let mut i: u32 = 0;
    while i + 4 <= b.content_size {
        check_found!(stream.read(4));
        if i != 4 {
            let brand = &stream.data()[..4];
            if brand == b"avif" || brand == b"avis" {
                check_found!(stream.skip(b.content_size - (i + 4)));
                return St::Found;
            }
            check!(i <= 32 * 4, St::Aborted); // Be reasonable.
        }
        i += 4;
    }
    St::Invalid // No AVIF brand no good.
}

fn parse_file(
    stream: &mut AvifStream<'_>,
    num_parsed_boxes: &mut u32,
    features: &mut InternalFeatures,
) -> St {
    loop {
        let mut b = BoxHeader::default();
        check_found!(parse_box(0, stream, MAX_SIZE, num_parsed_boxes, &mut b));
        if &b.kind == b"meta" {
            return parse_meta(1, stream, b.content_size, num_parsed_boxes, features);
        }
        check_found!(stream.skip(b.content_size));
    }
}

/// `AvifInfoIdentifyStream` over a php stream (`php_is_image_avif`).
pub fn identify(stream: &mut Stream) -> AvifInfoStatus {
    let mut s = AvifStream {
        stream: Some(stream),
        buffer: [0; MAX_NUM_READ_BYTES],
    };
    convert_status(parse_ftyp(&mut s))
}

/// `AvifInfoGetFeaturesStream` over a php stream (`php_handle_avif`).
pub fn get_features(stream: &mut Stream) -> Result<AvifInfoFeatures, AvifInfoStatus> {
    let mut s = AvifStream {
        stream: Some(stream),
        buffer: [0; MAX_NUM_READ_BYTES],
    };
    let mut num_parsed_boxes = 0u32;
    let mut internal = InternalFeatures::default();
    let status = parse_file(&mut s, &mut num_parsed_boxes, &mut internal);
    if status == St::Found {
        Ok(internal.primary_item_features)
    } else {
        Err(convert_status(status))
    }
}
