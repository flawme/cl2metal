//! Cell-aligned drawing for box-drawing and block characters.
//!
//! The font's glyphs for these characters carry different ink offsets: the
//! rounded corner hugs the left of its em box while the bar sits at its
//! center, so blitting them ink-centered (as the glyph path does) lands each
//! glyph at a different sub-cell position and the strokes step and notch
//! where glyphs hand off. Drawing them as geometry that spans the full cell
//! instead makes the strokes tile by construction — the same approach
//! Alacritty and Kitty take.
//!
//! Straight shapes go through the renderer's flat-rect pass; the four
//! rounded corners are anti-aliased coverage bitmaps that the glyph atlas
//! synthesizes (see `GlyphAtlas::synthetic`), blitted cell-aligned so they
//! meet the straight strokes exactly.

/// Stroke weight of a box-drawing arm.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Weight {
    Light,
    Heavy,
    Double,
}

/// Which corner an arc glyph rounds. Named after the Unicode names
/// ("light arc down and right" opens down and right), so the bulge points
/// the opposite way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ArcKind {
    DownRight,
    DownLeft,
    UpRight,
    UpLeft,
}

/// Dash pattern of a dashed line glyph: how many dashes fit in one cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dash {
    Double,
    Triple,
    Quadruple,
}

/// The shape a character draws, in fractions of the cell.
enum Shape {
    /// (left, right, up, down) arms.
    Arms {
        l: Option<Weight>,
        r: Option<Weight>,
        u: Option<Weight>,
        d: Option<Weight>,
        /// Present only on the pure dashed lines, where it replaces the arms
        /// along the line's axis.
        dash: Option<Dash>,
    },
    /// Filled rects [x, y, w, h] in cell fractions.
    Block {
        rects: [(f32, f32, f32, f32); 2],
        n: usize,
    },
}

use Weight::{Double as Db, Heavy as Hv, Light as Lt};

fn shape(c: char) -> Option<Shape> {
    use Shape::Arms;
    let s = match c {
        // -- Horizontal lines ------------------------------------------------
        '\u{2500}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: None,
            d: None,
            dash: None,
        },
        '\u{2501}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: None,
            d: None,
            dash: None,
        },
        '\u{2550}' => Arms {
            l: Some(Db),
            r: Some(Db),
            u: None,
            d: None,
            dash: None,
        },
        '\u{2504}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: None,
            d: None,
            dash: Some(Dash::Triple),
        },
        '\u{2505}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: None,
            d: None,
            dash: Some(Dash::Triple),
        },
        '\u{2508}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: None,
            d: None,
            dash: Some(Dash::Quadruple),
        },
        '\u{2509}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: None,
            d: None,
            dash: Some(Dash::Quadruple),
        },
        '\u{254C}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: None,
            d: None,
            dash: Some(Dash::Double),
        },
        '\u{254D}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: None,
            d: None,
            dash: Some(Dash::Double),
        },
        // -- Vertical lines --------------------------------------------------
        '\u{2502}' => Arms {
            l: None,
            r: None,
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{2503}' => Arms {
            l: None,
            r: None,
            u: Some(Hv),
            d: Some(Hv),
            dash: None,
        },
        '\u{2551}' => Arms {
            l: None,
            r: None,
            u: Some(Db),
            d: Some(Db),
            dash: None,
        },
        '\u{2506}' => Arms {
            l: None,
            r: None,
            u: Some(Lt),
            d: Some(Lt),
            dash: Some(Dash::Triple),
        },
        '\u{2507}' => Arms {
            l: None,
            r: None,
            u: Some(Hv),
            d: Some(Hv),
            dash: Some(Dash::Triple),
        },
        '\u{250A}' => Arms {
            l: None,
            r: None,
            u: Some(Lt),
            d: Some(Lt),
            dash: Some(Dash::Quadruple),
        },
        '\u{250B}' => Arms {
            l: None,
            r: None,
            u: Some(Hv),
            d: Some(Hv),
            dash: Some(Dash::Quadruple),
        },
        '\u{254E}' => Arms {
            l: None,
            r: None,
            u: Some(Lt),
            d: Some(Lt),
            dash: Some(Dash::Double),
        },
        '\u{254F}' => Arms {
            l: None,
            r: None,
            u: Some(Hv),
            d: Some(Hv),
            dash: Some(Dash::Double),
        },
        // -- Corners: down and right ----------------------------------------
        '\u{250C}' => Arms {
            l: None,
            r: Some(Lt),
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{250D}' => Arms {
            l: None,
            r: Some(Hv),
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{250E}' => Arms {
            l: None,
            r: Some(Lt),
            u: None,
            d: Some(Hv),
            dash: None,
        },
        '\u{250F}' => Arms {
            l: None,
            r: Some(Hv),
            u: None,
            d: Some(Hv),
            dash: None,
        },
        '\u{2552}' => Arms {
            l: None,
            r: Some(Db),
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{2553}' => Arms {
            l: None,
            r: Some(Lt),
            u: None,
            d: Some(Db),
            dash: None,
        },
        '\u{2554}' => Arms {
            l: None,
            r: Some(Db),
            u: None,
            d: Some(Db),
            dash: None,
        },
        // -- Corners: down and left -----------------------------------------
        '\u{2510}' => Arms {
            l: Some(Lt),
            r: None,
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{2511}' => Arms {
            l: Some(Hv),
            r: None,
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{2512}' => Arms {
            l: Some(Lt),
            r: None,
            u: None,
            d: Some(Hv),
            dash: None,
        },
        '\u{2513}' => Arms {
            l: Some(Hv),
            r: None,
            u: None,
            d: Some(Hv),
            dash: None,
        },
        '\u{2555}' => Arms {
            l: Some(Db),
            r: None,
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{2556}' => Arms {
            l: Some(Lt),
            r: None,
            u: None,
            d: Some(Db),
            dash: None,
        },
        '\u{2557}' => Arms {
            l: Some(Db),
            r: None,
            u: None,
            d: Some(Db),
            dash: None,
        },
        // -- Corners: up and right ------------------------------------------
        '\u{2514}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{2515}' => Arms {
            l: None,
            r: Some(Hv),
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{2516}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Hv),
            d: None,
            dash: None,
        },
        '\u{2517}' => Arms {
            l: None,
            r: Some(Hv),
            u: Some(Hv),
            d: None,
            dash: None,
        },
        '\u{2558}' => Arms {
            l: None,
            r: Some(Db),
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{2559}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Db),
            d: None,
            dash: None,
        },
        '\u{255A}' => Arms {
            l: None,
            r: Some(Db),
            u: Some(Db),
            d: None,
            dash: None,
        },
        // -- Corners: up and left -------------------------------------------
        '\u{2518}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{2519}' => Arms {
            l: Some(Hv),
            r: None,
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{251A}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Hv),
            d: None,
            dash: None,
        },
        '\u{251B}' => Arms {
            l: Some(Hv),
            r: None,
            u: Some(Hv),
            d: None,
            dash: None,
        },
        '\u{255B}' => Arms {
            l: Some(Db),
            r: None,
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{255C}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Db),
            d: None,
            dash: None,
        },
        '\u{255D}' => Arms {
            l: Some(Db),
            r: None,
            u: Some(Db),
            d: None,
            dash: None,
        },
        // -- Rounded corners --------------------------------------------------
        '\u{256D}' => Arms {
            l: None,
            r: Some(Lt),
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{256E}' => Arms {
            l: Some(Lt),
            r: None,
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{2570}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{256F}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Lt),
            d: None,
            dash: None,
        },
        // -- Tees: vertical and right ----------------------------------------
        '\u{251C}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{251D}' => Arms {
            l: None,
            r: Some(Hv),
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{251E}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Hv),
            d: Some(Lt),
            dash: None,
        },
        '\u{251F}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Lt),
            d: Some(Hv),
            dash: None,
        },
        '\u{2520}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Hv),
            d: Some(Hv),
            dash: None,
        },
        '\u{2521}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Hv),
            d: Some(Lt),
            dash: None,
        },
        '\u{2522}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Lt),
            d: Some(Hv),
            dash: None,
        },
        '\u{2523}' => Arms {
            l: None,
            r: Some(Hv),
            u: Some(Hv),
            d: Some(Hv),
            dash: None,
        },
        '\u{255E}' => Arms {
            l: None,
            r: Some(Db),
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{255F}' => Arms {
            l: None,
            r: Some(Lt),
            u: Some(Db),
            d: Some(Db),
            dash: None,
        },
        '\u{2560}' => Arms {
            l: None,
            r: Some(Db),
            u: Some(Db),
            d: Some(Db),
            dash: None,
        },
        // -- Tees: vertical and left -----------------------------------------
        '\u{2524}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{2525}' => Arms {
            l: Some(Hv),
            r: None,
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{2526}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Hv),
            d: Some(Lt),
            dash: None,
        },
        '\u{2527}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Lt),
            d: Some(Hv),
            dash: None,
        },
        '\u{2528}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Hv),
            d: Some(Hv),
            dash: None,
        },
        '\u{2529}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Hv),
            d: Some(Lt),
            dash: None,
        },
        '\u{252A}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Lt),
            d: Some(Hv),
            dash: None,
        },
        '\u{252B}' => Arms {
            l: Some(Hv),
            r: None,
            u: Some(Hv),
            d: Some(Hv),
            dash: None,
        },
        '\u{2561}' => Arms {
            l: Some(Db),
            r: None,
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{2562}' => Arms {
            l: Some(Lt),
            r: None,
            u: Some(Db),
            d: Some(Db),
            dash: None,
        },
        '\u{2563}' => Arms {
            l: Some(Db),
            r: None,
            u: Some(Db),
            d: Some(Db),
            dash: None,
        },
        // -- Tees: down and horizontal ---------------------------------------
        '\u{252C}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{252D}' => Arms {
            l: Some(Hv),
            r: Some(Lt),
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{252E}' => Arms {
            l: Some(Lt),
            r: Some(Hv),
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{252F}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{2530}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: None,
            d: Some(Hv),
            dash: None,
        },
        '\u{2531}' => Arms {
            l: Some(Hv),
            r: Some(Lt),
            u: None,
            d: Some(Hv),
            dash: None,
        },
        '\u{2532}' => Arms {
            l: Some(Lt),
            r: Some(Hv),
            u: None,
            d: Some(Hv),
            dash: None,
        },
        '\u{2533}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: None,
            d: Some(Hv),
            dash: None,
        },
        '\u{2564}' => Arms {
            l: Some(Db),
            r: Some(Db),
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{2565}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: None,
            d: Some(Db),
            dash: None,
        },
        '\u{2566}' => Arms {
            l: Some(Db),
            r: Some(Db),
            u: None,
            d: Some(Db),
            dash: None,
        },
        // -- Tees: up and horizontal -----------------------------------------
        '\u{2534}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{2535}' => Arms {
            l: Some(Hv),
            r: Some(Lt),
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{2536}' => Arms {
            l: Some(Lt),
            r: Some(Hv),
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{2537}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{2538}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: Some(Hv),
            d: None,
            dash: None,
        },
        '\u{2539}' => Arms {
            l: Some(Hv),
            r: Some(Lt),
            u: Some(Hv),
            d: None,
            dash: None,
        },
        '\u{253A}' => Arms {
            l: Some(Lt),
            r: Some(Hv),
            u: Some(Hv),
            d: None,
            dash: None,
        },
        '\u{253B}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: Some(Hv),
            d: None,
            dash: None,
        },
        '\u{2567}' => Arms {
            l: Some(Db),
            r: Some(Db),
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{2568}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: Some(Db),
            d: None,
            dash: None,
        },
        '\u{2569}' => Arms {
            l: Some(Db),
            r: Some(Db),
            u: Some(Db),
            d: None,
            dash: None,
        },
        // -- Crosses ----------------------------------------------------------
        '\u{253C}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{253D}' => Arms {
            l: Some(Hv),
            r: Some(Lt),
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{253E}' => Arms {
            l: Some(Lt),
            r: Some(Hv),
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{253F}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{2540}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: Some(Hv),
            d: Some(Lt),
            dash: None,
        },
        '\u{2541}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: Some(Lt),
            d: Some(Hv),
            dash: None,
        },
        '\u{2542}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: Some(Hv),
            d: Some(Hv),
            dash: None,
        },
        '\u{2543}' => Arms {
            l: Some(Hv),
            r: Some(Lt),
            u: Some(Hv),
            d: Some(Lt),
            dash: None,
        },
        '\u{2544}' => Arms {
            l: Some(Lt),
            r: Some(Hv),
            u: Some(Hv),
            d: Some(Lt),
            dash: None,
        },
        '\u{2545}' => Arms {
            l: Some(Hv),
            r: Some(Lt),
            u: Some(Lt),
            d: Some(Hv),
            dash: None,
        },
        '\u{2546}' => Arms {
            l: Some(Lt),
            r: Some(Hv),
            u: Some(Lt),
            d: Some(Hv),
            dash: None,
        },
        '\u{2547}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: Some(Hv),
            d: Some(Lt),
            dash: None,
        },
        '\u{2548}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: Some(Lt),
            d: Some(Hv),
            dash: None,
        },
        '\u{2549}' => Arms {
            l: Some(Hv),
            r: Some(Lt),
            u: Some(Hv),
            d: Some(Hv),
            dash: None,
        },
        '\u{254A}' => Arms {
            l: Some(Lt),
            r: Some(Hv),
            u: Some(Hv),
            d: Some(Hv),
            dash: None,
        },
        '\u{254B}' => Arms {
            l: Some(Hv),
            r: Some(Hv),
            u: Some(Hv),
            d: Some(Hv),
            dash: None,
        },
        '\u{256A}' => Arms {
            l: Some(Db),
            r: Some(Db),
            u: Some(Lt),
            d: Some(Lt),
            dash: None,
        },
        '\u{256B}' => Arms {
            l: Some(Lt),
            r: Some(Lt),
            u: Some(Db),
            d: Some(Db),
            dash: None,
        },
        '\u{256C}' => Arms {
            l: Some(Db),
            r: Some(Db),
            u: Some(Db),
            d: Some(Db),
            dash: None,
        },
        // -- Half lines --------------------------------------------------------
        '\u{2574}' => Arms {
            l: Some(Lt),
            r: None,
            u: None,
            d: None,
            dash: None,
        },
        '\u{2575}' => Arms {
            l: None,
            r: None,
            u: Some(Lt),
            d: None,
            dash: None,
        },
        '\u{2576}' => Arms {
            l: None,
            r: Some(Lt),
            u: None,
            d: None,
            dash: None,
        },
        '\u{2577}' => Arms {
            l: None,
            r: None,
            u: None,
            d: Some(Lt),
            dash: None,
        },
        '\u{2578}' => Arms {
            l: Some(Hv),
            r: None,
            u: None,
            d: None,
            dash: None,
        },
        '\u{2579}' => Arms {
            l: None,
            r: None,
            u: Some(Hv),
            d: None,
            dash: None,
        },
        '\u{257A}' => Arms {
            l: None,
            r: Some(Hv),
            u: None,
            d: None,
            dash: None,
        },
        '\u{257B}' => Arms {
            l: None,
            r: None,
            u: None,
            d: Some(Hv),
            dash: None,
        },
        '\u{257C}' => Arms {
            l: Some(Lt),
            r: Some(Hv),
            u: None,
            d: None,
            dash: None,
        },
        '\u{257D}' => Arms {
            l: None,
            r: None,
            u: Some(Lt),
            d: Some(Hv),
            dash: None,
        },
        '\u{257E}' => Arms {
            l: Some(Hv),
            r: Some(Lt),
            u: None,
            d: None,
            dash: None,
        },
        '\u{257F}' => Arms {
            l: None,
            r: None,
            u: Some(Hv),
            d: Some(Lt),
            dash: None,
        },
        // -- Diagonals and shades stay with the font ---------------------------
        '\u{2571}' | '\u{2572}' | '\u{2573}' | '\u{2591}' | '\u{2592}' | '\u{2593}' => {
            return None;
        }
        // -- Blocks -------------------------------------------------------------
        '\u{2580}' => Shape::Block {
            rects: [(0.0, 0.0, 1.0, 0.5), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2581}' => Shape::Block {
            rects: [(0.0, 0.875, 1.0, 0.125), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2582}' => Shape::Block {
            rects: [(0.0, 0.75, 1.0, 0.25), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2583}' => Shape::Block {
            rects: [(0.0, 0.625, 1.0, 0.375), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2584}' => Shape::Block {
            rects: [(0.0, 0.5, 1.0, 0.5), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2585}' => Shape::Block {
            rects: [(0.0, 0.375, 1.0, 0.625), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2586}' => Shape::Block {
            rects: [(0.0, 0.25, 1.0, 0.75), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2587}' => Shape::Block {
            rects: [(0.0, 0.125, 1.0, 0.875), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2588}' => Shape::Block {
            rects: [(0.0, 0.0, 1.0, 1.0), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2589}' => Shape::Block {
            rects: [(0.0, 0.0, 0.875, 1.0), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{258A}' => Shape::Block {
            rects: [(0.0, 0.0, 0.75, 1.0), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{258B}' => Shape::Block {
            rects: [(0.0, 0.0, 0.625, 1.0), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{258C}' => Shape::Block {
            rects: [(0.0, 0.0, 0.5, 1.0), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{258D}' => Shape::Block {
            rects: [(0.0, 0.0, 0.375, 1.0), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{258E}' => Shape::Block {
            rects: [(0.0, 0.0, 0.25, 1.0), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{258F}' => Shape::Block {
            rects: [(0.0, 0.0, 0.125, 1.0), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2590}' => Shape::Block {
            rects: [(0.5, 0.0, 0.5, 1.0), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2594}' => Shape::Block {
            rects: [(0.0, 0.0, 1.0, 0.125), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2595}' => Shape::Block {
            rects: [(0.875, 0.0, 0.125, 1.0), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2596}' => Shape::Block {
            rects: [(0.0, 0.5, 0.5, 0.5), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2597}' => Shape::Block {
            rects: [(0.5, 0.5, 0.5, 0.5), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2598}' => Shape::Block {
            rects: [(0.0, 0.0, 0.5, 0.5), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{2599}' => Shape::Block {
            rects: [(0.0, 0.0, 0.5, 0.5), (0.0, 0.5, 1.0, 0.5)],
            n: 2,
        },
        '\u{259A}' => Shape::Block {
            rects: [(0.0, 0.0, 0.5, 0.5), (0.5, 0.5, 0.5, 0.5)],
            n: 2,
        },
        '\u{259B}' => Shape::Block {
            rects: [(0.0, 0.0, 1.0, 0.5), (0.0, 0.5, 0.5, 0.5)],
            n: 2,
        },
        '\u{259C}' => Shape::Block {
            rects: [(0.0, 0.0, 1.0, 0.5), (0.5, 0.5, 0.5, 0.5)],
            n: 2,
        },
        '\u{259D}' => Shape::Block {
            rects: [(0.5, 0.0, 0.5, 0.5), (0.0, 0.0, 0.0, 0.0)],
            n: 1,
        },
        '\u{259E}' => Shape::Block {
            rects: [(0.5, 0.0, 0.5, 0.5), (0.0, 0.5, 0.5, 0.5)],
            n: 2,
        },
        '\u{259F}' => Shape::Block {
            rects: [(0.5, 0.0, 0.5, 0.5), (0.0, 0.5, 1.0, 0.5)],
            n: 2,
        },
        _ => return None,
    };
    Some(s)
}

/// Does this character get drawn as cell geometry (rather than a font glyph)?
pub(crate) fn is_drawn(c: char) -> bool {
    shape(c).is_some()
}

/// The rounded-corner glyph of `c`, if it is one.
pub(crate) fn arc_kind(c: char) -> Option<ArcKind> {
    match c {
        '\u{256D}' => Some(ArcKind::DownRight),
        '\u{256E}' => Some(ArcKind::DownLeft),
        '\u{2570}' => Some(ArcKind::UpRight),
        '\u{256F}' => Some(ArcKind::UpLeft),
        _ => None,
    }
}

/// Stroke thickness of a light line in a cell `ch` pixels tall.
fn light_thickness(ch: f32) -> f32 {
    (ch * 0.10).round().clamp(1.0, ch * 0.20)
}

/// Stroke thickness of a heavy line, given the light one.
fn heavy_thickness(t: f32) -> f32 {
    (t * 1.5).round().max(t + 1.0)
}

/// Radius of a rounded corner's arc.
fn arc_radius(cw: f32, ch: f32, t: f32) -> f32 {
    // The arc must leave room for its two arms inside the cell; at typical
    // cell ratios the width is the binding constraint.
    let r = (cw * 0.5).min(ch * 0.33);
    r.max(t + 1.0).min(cw * 0.5)
}

/// A small fixed set of rects, so per-cell drawing stays allocation-free.
pub(crate) struct Rects {
    items: [[f32; 4]; 12],
    n: usize,
}

impl Rects {
    fn push(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if self.n < self.items.len() && w > 0.0 && h > 0.0 {
            self.items[self.n] = [x, y, w, h];
            self.n += 1;
        }
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, [f32; 4]> {
        self.items[..self.n].iter()
    }
}

/// The rects a box/block character fills in the cell at `(x, y)`, sized
/// `cw x ch`. Empty when the character is left to the font.
pub(crate) fn rects(c: char, x: f32, y: f32, cw: f32, ch: f32) -> Rects {
    let mut out = Rects {
        items: [[0.0; 4]; 12],
        n: 0,
    };
    let Some(s) = shape(c) else {
        return out;
    };
    // Fractional mids, matching `paint_arc`, so the arcs and the straight
    // arms meet; the stroke edges are integer-aligned in `push_horiz`/`push_vert`.
    let mid_x = x + cw * 0.5;
    let mid_y = y + ch * 0.5;
    let t = light_thickness(ch);
    let th = heavy_thickness(t);

    match s {
        Shape::Block { rects: fr, n } => {
            for (fx, fy, fw, fh) in fr.iter().take(n) {
                out.push(x + fx * cw, y + fy * ch, fw * cw, fh * ch);
            }
        }
        Shape::Arms { l, r, u, d, dash } => {
            // Dashed lines replace both arms along their axis.
            if let Some(dash) = dash {
                let (horizontal, weight) = if l.is_some() { (true, l) } else { (false, u) };
                let weight = weight.unwrap_or(Weight::Light);
                let span = if horizontal { cw } else { ch };
                let count = match dash {
                    Dash::Double => 2,
                    Dash::Triple => 3,
                    Dash::Quadruple => 4,
                } as f32;
                let seg = span / count;
                for k in 0..count as usize {
                    let a = k as f32 * seg;
                    if horizontal {
                        push_horiz(&mut out, x + a, x + a + seg * 0.5, weight, mid_y, t, th);
                    } else {
                        push_vert(&mut out, y + a, y + a + seg * 0.5, weight, mid_x, t, th);
                    }
                }
                return out;
            }
            // Arms reach half a stroke into the center — or, for the rounded
            // corners, into the arc — so opposing arms chain with no seam
            // whatever the rasterizer's half-open edge convention does.
            let arc = arc_kind(c);
            if let Some(w) = l {
                let inset = arc.map_or(mid_x, |k| arc_inset_x(k, mid_x, cw, ch, t));
                push_horiz(
                    &mut out,
                    x,
                    inset + arm_thickness(w, t, th) * 0.5,
                    w,
                    mid_y,
                    t,
                    th,
                );
            }
            if let Some(w) = r {
                let inset = arc.map_or(mid_x, |k| arc_inset_x(k, mid_x, cw, ch, t));
                push_horiz(
                    &mut out,
                    inset - arm_thickness(w, t, th) * 0.5,
                    x + cw,
                    w,
                    mid_y,
                    t,
                    th,
                );
            }
            if let Some(w) = u {
                let inset = arc.map_or(mid_y, |k| arc_inset_y(k, mid_y, cw, ch, t));
                push_vert(
                    &mut out,
                    y,
                    inset + arm_thickness(w, t, th) * 0.5,
                    w,
                    mid_x,
                    t,
                    th,
                );
            }
            if let Some(w) = d {
                let inset = arc.map_or(mid_y, |k| arc_inset_y(k, mid_y, cw, ch, t));
                push_vert(
                    &mut out,
                    inset - arm_thickness(w, t, th) * 0.5,
                    y + ch,
                    w,
                    mid_x,
                    t,
                    th,
                );
            }
        }
    }
    out
}

/// Where a rounded corner's horizontal arm meets the arc, on the mid line.
fn arc_inset_x(kind: ArcKind, mid_x: f32, cw: f32, ch: f32, t: f32) -> f32 {
    let r = arc_radius(cw, ch, t);
    match kind {
        ArcKind::DownRight | ArcKind::UpRight => mid_x + r,
        ArcKind::DownLeft | ArcKind::UpLeft => mid_x - r,
    }
}

/// Where a rounded corner's vertical arm meets the arc, on the mid line.
fn arc_inset_y(kind: ArcKind, mid_y: f32, cw: f32, ch: f32, t: f32) -> f32 {
    let r = arc_radius(cw, ch, t);
    match kind {
        ArcKind::DownRight | ArcKind::DownLeft => mid_y + r,
        ArcKind::UpRight | ArcKind::UpLeft => mid_y - r,
    }
}

/// The stroke width of an arm of weight `w`.
fn arm_thickness(w: Weight, t: f32, th: f32) -> f32 {
    match w {
        Weight::Light => t,
        Weight::Heavy => th,
        // A double arm's overlap is its inner stroke's.
        Weight::Double => t,
    }
}

/// A horizontal stroke from `xa` to `xb` along the cell's mid line. The
/// band edges are rounded to whole pixels so the stroke stays crisp.
fn push_horiz(out: &mut Rects, xa: f32, xb: f32, w: Weight, mid_y: f32, t: f32, th: f32) {
    match w {
        Weight::Light => out.push(xa, (mid_y - t * 0.5).round(), xb - xa, t),
        Weight::Heavy => out.push(xa, (mid_y - th * 0.5).round(), xb - xa, th),
        Weight::Double => {
            out.push(xa, (mid_y - 1.5 * t).round(), xb - xa, t);
            out.push(xa, (mid_y + 0.5 * t).round(), xb - xa, t);
        }
    }
}

/// A vertical stroke from `ya` to `yb` along the cell's mid line.
fn push_vert(out: &mut Rects, ya: f32, yb: f32, w: Weight, mid_x: f32, t: f32, th: f32) {
    match w {
        Weight::Light => out.push((mid_x - t * 0.5).round(), ya, t, yb - ya),
        Weight::Heavy => out.push((mid_x - th * 0.5).round(), ya, th, yb - ya),
        Weight::Double => {
            out.push((mid_x - 1.5 * t).round(), ya, t, yb - ya);
            out.push((mid_x + 0.5 * t).round(), ya, t, yb - ya);
        }
    }
}

/// Paint the anti-aliased arc of a rounded-corner glyph into a `cw x ch`
/// coverage bitmap, calling `put(x, y, coverage)` for every touched pixel
/// (coverage 0-255). Only the curve is painted; the straight arms are plain
/// rects drawn by `rects`. Returns false when `c` is not a corner glyph.
pub(crate) fn paint_arc(c: char, cw: f32, ch: f32, put: &mut dyn FnMut(u32, u32, u8)) -> bool {
    let Some(kind) = arc_kind(c) else {
        return false;
    };
    let (uw, uh) = (cw.ceil() as u32, ch.ceil() as u32);
    let t = light_thickness(ch);
    let r = arc_radius(cw, ch, t);
    let (mx, my) = (cw * 0.5, ch * 0.5);
    // The arc's center sits one radius diagonal from the cell mid point,
    // toward the corner opposite the one being rounded.
    let (cx, cy) = match kind {
        ArcKind::DownRight => (mx + r, my + r),
        ArcKind::DownLeft => (mx - r, my + r),
        ArcKind::UpRight => (mx + r, my - r),
        ArcKind::UpLeft => (mx - r, my - r),
    };
    let half = t * 0.5;
    for py in 0..uh {
        for px in 0..uw {
            let (sx, sy) = (px as f32 + 0.5, py as f32 + 0.5);
            let (dx, dy) = (sx - cx, sy - cy);
            // Only the quadrant that bulges toward the rounded corner.
            let in_quad = match kind {
                ArcKind::DownRight => dx <= 0.0 && dy <= 0.0,
                ArcKind::DownLeft => dx >= 0.0 && dy <= 0.0,
                ArcKind::UpRight => dx <= 0.0 && dy >= 0.0,
                ArcKind::UpLeft => dx >= 0.0 && dy >= 0.0,
            };
            if !in_quad {
                continue;
            }
            let d = (dx * dx + dy * dy).sqrt();
            let cov = ((r + half + 0.5 - d).clamp(0.0, 1.0)
                * (d - (r - half) + 0.5).clamp(0.0, 1.0))
            .clamp(0.0, 1.0);
            if cov > 0.008 {
                put(px, py, (cov * 255.0) as u8);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: f32 = 10.0;
    const Y: f32 = 20.0;
    const CW: f32 = 9.0;
    const CH: f32 = 18.0;

    fn all(c: char) -> Vec<[f32; 4]> {
        rects(c, X, Y, CW, CH).iter().copied().collect()
    }

    /// The union of a char's rects, for span checks.
    fn union(rs: &[[f32; 4]]) -> (f32, f32, f32, f32) {
        let min_x = rs.iter().map(|r| r[0]).fold(f32::MAX, f32::min);
        let min_y = rs.iter().map(|r| r[1]).fold(f32::MAX, f32::min);
        let max_x = rs.iter().map(|r| r[0] + r[2]).fold(f32::MIN, f32::max);
        let max_y = rs.iter().map(|r| r[1] + r[3]).fold(f32::MIN, f32::max);
        (min_x, min_y, max_x, max_y)
    }

    #[test]
    fn a_vertical_line_spans_the_full_cell_height() {
        let rs = all('\u{2502}');
        assert_eq!(rs.len(), 2, "an up arm and a down arm");
        let (_, y0, _, y1) = union(&rs);
        assert_eq!((y0, y1), (Y, Y + CH), "spans the full cell height");
        // The arms overlap across the mid line, so there is no seam.
        let sorted = {
            let mut v = rs.clone();
            v.sort_by(|a, b| a[1].total_cmp(&b[1]));
            v
        };
        assert!(
            sorted[0][1] + sorted[0][3] >= sorted[1][1],
            "arms overlap: {sorted:?}"
        );
        // Every piece is the same stroke width.
        for r in &rs {
            assert!((r[2] - rs[0][2]).abs() < 0.001, "uniform stroke: {r:?}");
        }
    }

    #[test]
    fn a_horizontal_line_spans_the_full_cell_width() {
        let rs = all('\u{2500}');
        assert_eq!(rs.len(), 2, "a left arm and a right arm");
        let (x0, _, x1, _) = union(&rs);
        assert_eq!((x0, x1), (X, X + CW), "spans the full cell width");
        // The arms overlap across the mid line, so there is no seam.
        let sorted = {
            let mut v = rs.clone();
            v.sort_by(|a, b| a[0].total_cmp(&b[0]));
            v
        };
        assert!(
            sorted[0][0] + sorted[0][2] >= sorted[1][0],
            "arms overlap: {sorted:?}"
        );
        // Both halves sit on the same band.
        assert_eq!(sorted[0][1], sorted[1][1]);
    }

    #[test]
    fn corners_reach_the_edges_they_must_connect_to() {
        // A corner's arms must reach the edges their neighbors continue on:
        // ┌'s arms go right and down.
        let rs = all('\u{250C}');
        let (_, _, x1, y1) = union(&rs);
        assert_eq!((x1, y1), (X + CW, Y + CH), "┌ reaches right and bottom");
        // ┘'s arms go left and up.
        let rs = all('\u{2518}');
        let (x0, y0, _, _) = union(&rs);
        assert_eq!((x0, y0), (X, Y), "┘ reaches left and top");
        // ┐ and └ cover the other diagonal.
        let rs = all('\u{2510}');
        let (x0, _, _, y1) = union(&rs);
        assert_eq!((x0, y1), (X, Y + CH), "┐ reaches left and bottom");
        let rs = all('\u{2514}');
        let (_, y0, x1, _) = union(&rs);
        assert_eq!((y0, x1), (Y, X + CW), "└ reaches top and right");
    }

    #[test]
    fn rounded_corners_connect_to_their_neighbors() {
        // ╭'s straight arms must still reach the cell edges so the row below
        // and the column to the right continue seamlessly; the arc itself is
        // painted separately as coverage.
        assert_eq!(arc_kind('\u{256D}'), Some(ArcKind::DownRight));
        let rs = all('\u{256D}');
        let (_, _, x1, y1) = union(&rs);
        assert_eq!(
            (x1, y1),
            (X + CW, Y + CH),
            "down and right arms reach the edges"
        );
        // The other three corners reach the edges they connect on.
        let rs = all('\u{2570}');
        let (_, y0, x1, _) = union(&rs);
        assert_eq!((y0, x1), (Y, X + CW), "╰ reaches top and right");
        let rs = all('\u{256E}');
        let (x0, _, _, y1) = union(&rs);
        assert_eq!((x0, y1), (X, Y + CH), "╮ reaches left and bottom");
        let rs = all('\u{256F}');
        let (x0, y0, _, _) = union(&rs);
        assert_eq!((x0, y0), (X, Y), "╯ reaches left and top");
    }

    #[test]
    fn tees_reach_every_edge_they_point_at() {
        // ├ is vertical-and-right: up, down and right, no left arm.
        let rs = all('\u{251C}');
        let (x0, y0, x1, y1) = union(&rs);
        assert_eq!(
            (y0, x1, y1),
            (Y, X + CW, Y + CH),
            "├ reaches top, right and bottom"
        );
        // The vertical stems share one x band, centered on the mid line.
        let stem: Vec<[f32; 4]> = rs.iter().filter(|r| r[3] > r[2]).copied().collect();
        assert_eq!(stem.len(), 2, "an up stem and a down stem");
        assert_eq!(stem[0][0], stem[1][0], "the stems share one x band");
        let stem_center = stem[0][0] + stem[0][2] * 0.5;
        assert!(
            (stem_center - (X + CW * 0.5)).abs() <= 0.51,
            "the stem is centered on the mid line: {stem_center}"
        );
        assert!(
            x0 > X,
            "├ has no left arm, so nothing reaches the left edge"
        );
        // The right arm is the lone horizontal and overlaps the stem.
        let horiz: Vec<[f32; 4]> = rs.iter().filter(|r| r[2] > r[3]).copied().collect();
        assert_eq!(horiz.len(), 1);
        assert_eq!(
            horiz[0][0] + horiz[0][2],
            X + CW,
            "the right arm reaches the right edge"
        );
        assert!(
            horiz[0][0] < stem[0][0] + stem[0][2],
            "the right arm overlaps the stem"
        );
    }

    #[test]
    fn heavy_lines_are_thicker_than_light_ones() {
        let light = all('\u{2500}')[0][3];
        let heavy = all('\u{2501}')[0][3];
        assert!(heavy > light, "heavy {heavy} must beat light {light}");
    }

    #[test]
    fn double_lines_come_in_paired_strokes() {
        // ═: both direction arms are double, so four strokes in two bands,
        // each band chaining across the full width.
        let rs = all('\u{2550}');
        assert_eq!(rs.len(), 4);
        let mut bands: Vec<f32> = rs.iter().map(|r| r[1]).collect();
        bands.sort_by(|a, b| a.total_cmp(b));
        bands.dedup();
        assert_eq!(bands.len(), 2, "exactly two horizontal bands: {bands:?}");
        for band in &bands {
            let strokes: Vec<[f32; 4]> = rs.iter().filter(|r| r[1] == *band).copied().collect();
            assert_eq!(strokes.len(), 2);
            assert_eq!(strokes.iter().map(|r| r[0]).fold(f32::MAX, f32::min), X);
            assert_eq!(
                strokes.iter().map(|r| r[0] + r[2]).fold(f32::MIN, f32::max),
                X + CW
            );
        }
        // ║: the same, vertically.
        let rs = all('\u{2551}');
        assert_eq!(rs.len(), 4);
        let mut bands: Vec<f32> = rs.iter().map(|r| r[0]).collect();
        bands.sort_by(|a, b| a.total_cmp(b));
        bands.dedup();
        assert_eq!(bands.len(), 2, "exactly two vertical bands: {bands:?}");
        for band in &bands {
            let strokes: Vec<[f32; 4]> = rs.iter().filter(|r| r[0] == *band).copied().collect();
            assert_eq!(strokes.len(), 2);
            assert_eq!(strokes.iter().map(|r| r[1]).fold(f32::MAX, f32::min), Y);
            assert_eq!(
                strokes.iter().map(|r| r[1] + r[3]).fold(f32::MIN, f32::max),
                Y + CH
            );
        }
    }

    #[test]
    fn dashes_leave_gaps_and_stay_inside_the_cell() {
        let rs = all('\u{2504}'); // ┄, triple dash
        assert_eq!(rs.len(), 3);
        for r in &rs {
            assert!(
                r[0] >= X && r[0] + r[2] <= X + CW,
                "dash inside the cell: {r:?}"
            );
            assert!(r[2] < CW, "each dash is shorter than the cell");
        }
        // The pattern must end with a gap so it repeats across cells.
        let last_end = rs.iter().map(|r| r[0] + r[2]).fold(f32::MIN, f32::max);
        assert!(last_end < X + CW, "the cell ends on a gap");
        // And start with a dash at the very edge, so neighbors connect.
        assert_eq!(rs[0][0], X);
    }

    #[test]
    fn blocks_fill_the_expected_fractions() {
        let full = all('\u{2588}');
        assert_eq!(full, vec![[X, Y, CW, CH]]);
        let upper = all('\u{2580}');
        assert_eq!(upper, vec![[X, Y, CW, CH * 0.5]]);
        let lower = all('\u{2584}');
        assert_eq!(lower, vec![[X, Y + CH * 0.5, CW, CH * 0.5]]);
        let left = all('\u{258C}');
        assert_eq!(left, vec![[X, Y, CW * 0.5, CH]]);
        // ▙ = upper-left quadrant + the whole lower half.
        let tri = all('\u{2599}');
        assert_eq!(tri.len(), 2);
        assert_eq!(tri[0], [X, Y, CW * 0.5, CH * 0.5]);
        assert_eq!(tri[1], [X, Y + CH * 0.5, CW, CH * 0.5]);
    }

    #[test]
    fn diagonals_and_shades_stay_with_the_font() {
        for c in [
            '\u{2571}', '\u{2572}', '\u{2573}', '\u{2591}', '\u{2592}', '\u{2593}', 'a', ' ', 'Ä',
        ] {
            assert!(!is_drawn(c), "{c:?} must stay with the font");
            assert!(rects(c, X, Y, CW, CH).iter().next().is_none());
        }
    }

    #[test]
    fn the_whole_box_range_is_covered() {
        // Every codepoint in U+2500..=U+257F except the three diagonals has
        // geometry; a typo in the table would break this census.
        for cp in 0x2500u32..=0x257F {
            let c = char::from_u32(cp).unwrap();
            let drawn = is_drawn(c);
            let diagonal = (0x2571..=0x2573).contains(&cp);
            assert_eq!(drawn, !diagonal, "U+{cp:04X} {c} coverage mismatch");
            assert_eq!(arc_kind(c).is_some(), (0x256D..=0x2570).contains(&cp));
        }
        // Every block element except the three shades.
        for cp in 0x2580u32..=0x259F {
            let c = char::from_u32(cp).unwrap();
            let drawn = is_drawn(c);
            let shade = (0x2591..=0x2593).contains(&cp);
            assert_eq!(drawn, !shade, "U+{cp:04X} {c} coverage mismatch");
        }
    }

    #[test]
    fn the_arc_paints_coverage_inside_the_cell_only() {
        let mut count = 0;
        let mut outside = 0;
        let ok = paint_arc('\u{256D}', CW, CH, &mut |x, y, v| {
            assert!(v > 0);
            if x >= CW.ceil() as u32 || y >= CH.ceil() as u32 {
                outside += 1;
            }
            count += 1;
        });
        assert!(ok);
        assert!(count > 8, "the arc has real coverage ({count} px)");
        assert_eq!(outside, 0);
        assert!(!paint_arc('\u{2500}', CW, CH, &mut |_, _, _| {}));
    }

    #[test]
    fn junctions_with_mixed_weights_are_solid() {
        // ┽: heavy left arm, light right/up/down arms. The horizontal arms
        // must chain across the whole width whatever their thicknesses.
        let rs = all('\u{253D}');
        let (x0, _, x1, _) = union(&rs);
        assert_eq!((x0, x1), (X, X + CW), "arms chain across the whole width");
        let horiz: Vec<[f32; 4]> = rs.iter().filter(|r| r[2] > r[3]).copied().collect();
        assert_eq!(horiz.len(), 2, "a heavy and a light horizontal arm");
        let widths: Vec<f32> = horiz.iter().map(|r| r[3]).collect();
        assert!(widths[0] != widths[1], "the weights differ: {widths:?}");
        let mut ends: Vec<(f32, f32)> = horiz.iter().map(|r| (r[0], r[0] + r[2])).collect();
        ends.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!(
            ends[0].1 >= ends[1].0,
            "the arms overlap with no seam: {ends:?}"
        );
    }
}
